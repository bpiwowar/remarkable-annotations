//! The cloud client on a minimal in-memory cloud (the comparison with
//! rmapi-js is tests/reference/cloud-check.mjs): index formats, uploads,
//! retries and errors.

use std::collections::HashMap;

use remarkable_annotations::cloud::{
    CloudError, ItemRef, Operation, Output, Request, Response, Session, SessionOptions, Step, Wait,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// A session token whose payload is `{"device-id": "dev"}`.
const TOKEN: &str = "e30.eyJkZXZpY2UtaWQiOiJkZXYifQ.sig";

struct Fake {
    files: HashMap<String, Vec<u8>>,
    root: String,
    generation: u64,
    schema: u32,
    log: Vec<Request>,
    /// statuses to answer the next requests with, before handling them
    fail: Vec<u16>,
    /// conflicts to raise on the next root PUTs
    conflicts: usize,
}

impl Fake {
    fn new(schema: u32, index: &str) -> Self {
        let mut f = Fake {
            files: HashMap::new(),
            root: String::new(),
            generation: 1,
            schema,
            log: Vec::new(),
            fail: Vec::new(),
            conflicts: 0,
        };
        f.root = f.put(index.as_bytes().to_vec());
        f
    }

    fn put(&mut self, b: Vec<u8>) -> String {
        let h = sha(&b);
        self.files.insert(h.clone(), b);
        h
    }

    fn send(&mut self, r: &Request) -> Response {
        self.log.push(r.clone());
        let resp = |status: u16, body: &[u8]| Response {
            status,
            body: body.to_vec(),
            ..Default::default()
        };
        if let Some(status) = self.fail.pop() {
            return resp(status, b"fault");
        }
        let path = r.url.strip_prefix("https://raw.test").unwrap();
        match (r.method.as_str(), path) {
            ("GET", "/sync/v4/root") => resp(
                200,
                json!({"hash": self.root, "generation": self.generation, "schemaVersion": self.schema})
                    .to_string()
                    .as_bytes(),
            ),
            ("PUT", "/sync/v3/root") => {
                let body: Value = serde_json::from_slice(r.body.as_ref().unwrap()).unwrap();
                if self.conflicts > 0 {
                    self.conflicts -= 1;
                    self.generation += 1;
                }
                if body["generation"] != self.generation {
                    return resp(412, b"{\"message\":\"precondition failed\"}\n");
                }
                self.root = body["hash"].as_str().unwrap().into();
                self.generation += 1;
                resp(200, json!({"hash": self.root, "generation": self.generation}).to_string().as_bytes())
            }
            (m, p) if p.starts_with("/sync/v3/files/") => {
                let hash = &p["/sync/v3/files/".len()..];
                if m == "GET" {
                    match self.files.get(hash) {
                        Some(b) => resp(200, b),
                        None => resp(404, b"not found"),
                    }
                } else {
                    self.files.insert(hash.into(), r.body.clone().unwrap());
                    resp(200, b"")
                }
            }
            _ => resp(404, b"?"),
        }
    }
}

fn session() -> Session {
    Session::new(
        TOKEN,
        SessionOptions {
            raw_host: "https://raw.test".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

/// A cloud with one folder, its index in the account's schema.
fn cloud(schema: u32) -> (Fake, String) {
    let mut f = Fake::new(schema, "4\n0:.:0:0\n");
    let content = f.put(br#"{"tags":[]}"#.to_vec());
    let meta = f.put(
        br#"{"visibleName":"Folder","type":"CollectionType","parent":"","pinned":false}"#.to_vec(),
    );
    let id = "10000000-0000-4000-8000-000000000001";
    let lines = format!("{content}:0:{id}.content:0:11\n{meta}:0:{id}.metadata:0:76\n");
    let index = if schema == 4 {
        format!("4\n0:{id}:2:87\n{lines}")
    } else {
        format!("3\n{lines}")
    };
    let index_hash = if schema == 4 {
        f.put(index.into_bytes())
    } else {
        let mut cat = Vec::new();
        for h in [&content, &meta] {
            cat.extend(
                (0..64)
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()),
            );
        }
        let h = sha(&cat);
        f.files.insert(h.clone(), index.into_bytes());
        h
    };
    let kind = if schema == 4 { 0 } else { 80000000 };
    let root = if schema == 4 {
        format!("4\n0:.:1:87\n{index_hash}:0:{id}:2:87\n")
    } else {
        format!("3\n{index_hash}:{kind}:{id}:2:87\n")
    };
    f.root = f.put(root.into_bytes());
    (f, id.into())
}

fn json_of(out: Output) -> Value {
    match out {
        Output::Json(v) => v,
        o => panic!("not JSON: {o:?}"),
    }
}

#[test]
fn lists_items_in_both_schemas() {
    for schema in [3, 4] {
        let (mut f, id) = cloud(schema);
        let items = json_of(session().list_items(false).run(|r| f.send(r)).unwrap());
        assert_eq!(items[0]["id"], id);
        assert_eq!(items[0]["type"], "CollectionType");
        assert_eq!(items[0]["visibleName"], "Folder");
        assert!(f
            .log
            .iter()
            .all(|r| r.headers[0] == ("Authorization".into(), format!("Bearer {TOKEN}"))));
    }
}

#[test]
fn creating_a_folder_writes_a_schema_4_root() {
    let (mut f, parent) = cloud(3);
    let new_id = "20000000-0000-4000-8000-000000000002";
    let r = json_of(
        session()
            .put_folder(
                "Sub".into(),
                parent.clone(),
                new_id.into(),
                1_700_000_000_000.0,
            )
            .run(|r| f.send(r))
            .unwrap(),
    );
    assert_eq!(r["id"], new_id);
    let root = String::from_utf8(f.files[&f.root].clone()).unwrap();
    assert!(root.starts_with("4\n0:.:2:"), "{root}");
    // the folder's own index keeps the account's schema (3)
    let meta_put = f
        .log
        .iter()
        .find(|r| {
            r.headers
                .iter()
                .any(|(k, v)| k == "rm-filename" && v.ends_with(".metadata"))
        })
        .unwrap();
    assert_eq!(
        String::from_utf8(meta_put.body.clone().unwrap()).unwrap(),
        format!(
            r#"{{"lastModified":"1700000000000","createdTime":"1700000000000","parent":"{parent}","pinned":false,"type":"CollectionType","visibleName":"Sub"}}"#
        )
    );
    let crc = meta_put
        .headers
        .iter()
        .find(|(k, _)| k == "x-goog-hash")
        .unwrap();
    assert!(crc.1.starts_with("crc32c=") && crc.1.len() == "crc32c=".len() + 8);
}

#[test]
fn retries_conflicts_and_transient_errors() {
    let (mut f, parent) = cloud(4);
    f.conflicts = 2;
    f.fail = vec![503, 429];
    let mut sleeps = Vec::new();
    let mut op = session().put_folder(
        "X".into(),
        parent,
        "20000000-0000-4000-8000-000000000003".into(),
        0.0,
    );
    let mut step = op.start();
    let result = loop {
        step = match step {
            Step::Request(r) => {
                let resp = f.send(&r);
                op.resume(Some(resp))
            }
            Step::Sleep { ms } => {
                sleeps.push(ms);
                op.resume(None)
            }
            Step::Batch(_) => panic!("a folder creation has no concurrent requests"),
            Step::Done(o) => break Ok(o),
            Step::Failed(e) => break Err(e),
        }
    };
    assert!(result.is_ok(), "{result:?}");
    // two transient retries (200, 400 ms), then two generation retries (25, 50 ms)
    assert_eq!(sleeps, [200.0, 400.0, 25.0, 50.0]);
}

#[test]
fn errors() {
    let (mut f, id) = cloud(4);
    let s = session();
    let stale = ItemRef {
        id: id.clone(),
        hash: "0".repeat(64),
    };
    assert_eq!(
        s.purge(stale.clone()).run(|r| f.send(r)),
        Err(CloudError::HashNotFound {
            hash: "0".repeat(64)
        })
    );
    assert!(matches!(
        s.move_item(stale, "nope".into()).run(|r| f.send(r)),
        Err(CloudError::Validation { .. })
    ));
    f.fail = vec![500; 4];
    assert!(matches!(
        session().list_refs(true).run(|r| f.send(r)),
        Err(CloudError::Response { status: 500, .. })
    ));
    assert!(Session::new("not-a-jwt", SessionOptions::default()).is_err());
    assert_eq!(session().device_id(), "dev");
}

/// A schema 4 cloud with `n` folders.
fn cloud_with(n: usize) -> Fake {
    let mut f = Fake::new(4, "4\n0:.:0:0\n");
    let mut root = format!("4\n0:.:{n}:0\n");
    for i in 0..n {
        let content = f.put(br#"{"tags":[]}"#.to_vec());
        let meta = f.put(
            format!(
                r#"{{"visibleName":"F{i}","type":"CollectionType","parent":"","pinned":false}}"#
            )
            .into_bytes(),
        );
        let id = format!("10000000-0000-4000-8000-{i:012}");
        let index =
            format!("4\n0:{id}:2:0\n{content}:0:{id}.content:0:0\n{meta}:0:{id}.metadata:0:0\n");
        let h = f.put(index.into_bytes());
        root.push_str(&format!("{h}:0:{id}:2:0\n"));
    }
    f.root = f.put(root.into_bytes());
    f
}

/// Drives an operation, answering every request of a batch (in order) and
/// counting the round trips (steps that wait on the host).
fn drive(mut op: Operation, f: &mut Fake) -> (Result<Output, CloudError>, usize, usize) {
    let (mut trips, mut widest) = (0, 0);
    let mut step = op.start();
    loop {
        step = match step {
            Step::Request(r) => {
                trips += 1;
                widest = widest.max(1);
                let resp = f.send(&r);
                op.resume(Some(resp))
            }
            Step::Sleep { .. } => {
                trips += 1;
                op.resume(None)
            }
            Step::Batch(waits) => {
                trips += 1;
                widest = widest.max(waits.len());
                let outcomes = waits
                    .iter()
                    .map(|w| match w {
                        Wait::Request(r) => Some(f.send(r)),
                        Wait::Sleep { .. } => None,
                    })
                    .collect();
                op.resume_batch(outcomes)
            }
            Step::Done(o) => return (Ok(o), trips, widest),
            Step::Failed(e) => return (Err(e), trips, widest),
        }
    }
}

/// Listing runs the documents concurrently, as rmapi-js: a constant number
/// of round trips, not one or more per document.
#[test]
fn listing_is_concurrent() {
    let mut f = cloud_with(50);
    let (result, trips, widest) = drive(session().list_items(true), &mut f);
    let items = json_of(result.unwrap());
    assert_eq!(items.as_array().unwrap().len(), 50);
    assert_eq!(items[7]["visibleName"], "F7");
    // root, root index, the 50 indexes, then the 100 metadata and contents
    assert_eq!(trips, 4, "round trips");
    assert_eq!(widest, 100);
    assert_eq!(f.log.len(), 2 + 50 + 100);
    // A second listing reads the root again; the rest is cached.
    let s = session();
    drive(s.list_items(true), &mut f).0.unwrap();
    let before = f.log.len();
    let (again, trips, _) = drive(s.list_items(true), &mut f);
    assert_eq!(json_of(again.unwrap()), items);
    assert_eq!((f.log.len() - before, trips), (1, 1));
}

/// Concurrent requests retry on their own: failures inside a batch give
/// sleeps and new requests in the next batches, and the same result.
#[test]
fn concurrent_requests_retry() {
    let mut reference = cloud_with(5);
    let expected = json_of(
        session()
            .list_items(true)
            .run(|r| reference.send(r))
            .unwrap(),
    );
    let mut f = cloud_with(5);
    // The 3rd, 4th and 6th requests fail with a transient error.
    let mut n = 0;
    let mut failing = |f: &mut Fake, r: &Request| {
        n += 1;
        if [3, 4, 6].contains(&n) {
            f.log.push(r.clone());
            Response {
                status: 503,
                body: b"busy".to_vec(),
                ..Default::default()
            }
        } else {
            f.send(r)
        }
    };
    let mut op = session().list_items(true);
    let mut step = op.start();
    let mut saw_sleep_in_batch = false;
    let result = loop {
        step = match step {
            Step::Request(r) => {
                let resp = failing(&mut f, &r);
                op.resume(Some(resp))
            }
            Step::Sleep { .. } => op.resume(None),
            Step::Batch(waits) => {
                saw_sleep_in_batch |= waits.iter().any(|w| matches!(w, Wait::Sleep { .. }));
                let outcomes = waits
                    .iter()
                    .map(|w| match w {
                        Wait::Request(r) => Some(failing(&mut f, r)),
                        Wait::Sleep { .. } => None,
                    })
                    .collect();
                op.resume_batch(outcomes)
            }
            Step::Done(o) => break Ok(o),
            Step::Failed(e) => break Err(e),
        }
    };
    assert_eq!(json_of(result.unwrap()), expected);
    assert!(
        saw_sleep_in_batch,
        "retries of concurrent requests are batched"
    );
}

/// A document that cannot be read fails the listing (as Promise.all).
#[test]
fn concurrent_errors() {
    let mut f = cloud_with(3);
    let broken = sha(br#"{"visibleName":"F1","type":"CollectionType","parent":"","pinned":false}"#);
    f.files.remove(&broken);
    let (result, ..) = drive(session().list_items(true), &mut f);
    assert!(
        matches!(result, Err(CloudError::Response { status: 404, .. })),
        "{result:?}"
    );
}
