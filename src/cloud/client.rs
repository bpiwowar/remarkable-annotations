//! The reMarkable cloud client: rmapi-js's `RawRemarkable` and `Remarkable`,
//! limited to what the Zotero plugin uses, as sans-I/O [`Operation`]s.
//!
//! Every function mirrors its rmapi-js counterpart, in the same order of
//! reads and writes, so that both leave the cloud in the same state. rmapi-js
//! runs some requests concurrently (`Promise.all`); here they run one after
//! the other, so a file read twice concurrently by rmapi-js is read once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::error::CloudError;
use super::exec::{Io, Operation, Output, Request, Response};
use super::js::{
    base64, base64url_decode, crc32c, from_hex, locale_compare, num, number_to_string, parse,
    sha256_hex, stringify,
};
use super::schema;

type Result<T> = std::result::Result<T, CloudError>;

pub const AUTH_HOST: &str = "https://webapp-prod.cloud.remarkable.engineering";
pub const RAW_HOST: &str = "https://eu.tectonic.remarkable.com";
pub const UPLOAD_HOST: &str = "https://internal.cloud.remarkable.com";
/// The root index's id.
const ROOT_LIST: &str = "root";
const TRANSIENT_BASE_MS: f64 = 200.0;
const GENERATION_BASE_MS: f64 = 25.0;
const PRECONDITION_FAILED: &str = "{\"message\":\"precondition failed\"}\n";

/// An item: its id and the hash of the version meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    pub id: String,
    pub hash: String,
}

/// A line of an index file.
#[derive(Debug, Clone, PartialEq)]
pub struct RawEntry {
    pub hash: String,
    /// 0 (file) or 80000000 (index, schema 3).
    pub kind: u32,
    pub id: String,
    pub subfiles: f64,
    pub size: f64,
}

impl RawEntry {
    pub fn to_json(&self) -> Value {
        json!({"hash": self.hash, "type": self.kind, "id": self.id,
               "subfiles": num(self.subfiles), "size": num(self.size)})
    }
}

/// Session settings (rmapi-js's `session` options).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionOptions {
    pub raw_host: String,
    pub upload_host: String,
    pub max_generation_retries: u32,
    pub max_transient_retries: u32,
    /// Files up to this size are cached with their bytes; larger ones only as
    /// known to exist.
    pub max_cached_bytes: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            raw_host: RAW_HOST.into(),
            upload_host: UPLOAD_HOST.into(),
            max_generation_retries: 10,
            max_transient_retries: 3,
            max_cached_bytes: 1024 * 1024,
        }
    }
}

struct State {
    token: String,
    opts: SessionOptions,
    /// Hash → bytes (`None`: exists, too large to keep).
    cache: HashMap<String, Option<Vec<u8>>>,
    last_hash_gen: Option<(String, f64)>,
    schema_version: Option<u32>,
    /// A root update is running (rmapi-js serializes them with a mutex).
    busy: bool,
}

/// A session with the cloud, holding the session token and the cache. Run
/// one operation at a time per session.
#[derive(Clone)]
pub struct Session {
    st: Rc<RefCell<State>>,
    device_id: String,
}

#[derive(Clone)]
struct Ctx {
    io: Rc<Io>,
    st: Rc<RefCell<State>>,
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// rmapi-js's `idReg`: a lowercase uuid, "" or "trash".
fn valid_id(id: &str) -> bool {
    if id.is_empty() || id == "trash" {
        return true;
    }
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn validation(msg: impl Into<String>) -> CloudError {
    CloudError::Validation {
        message: msg.into(),
    }
}

/// `parseInt(s, 10)`.
fn parse_int(s: &str) -> f64 {
    let s = s.trim_start();
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    let n = rest.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        f64::NAN
    } else {
        sign * rest[..n].parse::<f64>().unwrap_or(f64::NAN)
    }
}

fn backoff_ms(attempt: u32, base: f64) -> f64 {
    (base * 2f64.powi(attempt as i32)).min(30_000.0)
}

/// The `device-id` claim of a session token (a JWT).
fn token_device_id(token: &str) -> Result<String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| CloudError::other("token wasn't a jwt, so it had no device id"))?;
    let decoded = base64url_decode(payload).ok_or_else(|| CloudError::other("invalid token"))?;
    let claims = parse(&decoded).map_err(CloudError::other)?;
    claims
        .get("device-id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CloudError::other("token has no device-id"))
}

impl Ctx {
    fn opts(&self) -> SessionOptions {
        self.st.borrow().opts.clone()
    }

    /// rmapi-js's `#authedFetch`: transient retries (5xx, 429, network) with
    /// backoff, except for the root PUT; a precondition failure is a
    /// generation conflict.
    async fn fetch(
        &self,
        method: &str,
        url: String,
        body: Option<Vec<u8>>,
        headers: Vec<(String, String)>,
    ) -> Result<Response> {
        let retries = if method == "PUT" && url.ends_with("/sync/v3/root") {
            0
        } else {
            self.opts().max_transient_retries
        };
        let mut all = vec![(
            "Authorization".to_owned(),
            format!("Bearer {}", self.st.borrow().token),
        )];
        all.extend(headers);
        let mut attempt = 0;
        loop {
            let resp = self
                .io
                .fetch(Request {
                    method: method.into(),
                    url: url.clone(),
                    headers: all.clone(),
                    body: body.clone(),
                })
                .await;
            if let Some(message) = &resp.network_error {
                if attempt < retries {
                    self.io.sleep(backoff_ms(attempt, TRANSIENT_BASE_MS)).await;
                    attempt += 1;
                    continue;
                }
                return Err(CloudError::Network {
                    message: message.clone(),
                });
            }
            if resp.ok() {
                return Ok(resp);
            }
            let msg = String::from_utf8_lossy(&resp.body).into_owned();
            if msg == PRECONDITION_FAILED {
                return Err(CloudError::Generation);
            } else if (resp.status >= 500 || resp.status == 429) && attempt < retries {
                self.io.sleep(backoff_ms(attempt, TRANSIENT_BASE_MS)).await;
                attempt += 1;
            } else {
                return Err(CloudError::Response {
                    status: resp.status,
                    status_text: resp.status_text,
                    message: format!("failed reMarkable request: {msg}"),
                });
            }
        }
    }

    /// `#getRootHash`: the cached root, or the server's.
    async fn root_hash(&self, refresh: bool) -> Result<(String, f64, u32)> {
        let cached = self.st.borrow().last_hash_gen.clone();
        if refresh || cached.is_none() {
            let url = format!("{}/sync/v4/root", self.opts().raw_host);
            let resp = self.fetch("GET", url, None, vec![]).await?;
            let loaded = parse(&resp.body).map_err(CloudError::other)?;
            let root = schema::parse(&loaded, schema::root_hash(), "root").map_err(validation)?;
            let hash = root["hash"].as_str().unwrap().to_owned();
            let generation = root["generation"].as_f64().unwrap();
            let schema_version = root["schemaVersion"].as_f64().unwrap() as u32;
            if schema_version != 3 && schema_version != 4 {
                return Err(CloudError::other(format!(
                    "schema version {schema_version} not supported"
                )));
            }
            if !super::js::is_safe_integer(generation) {
                return Err(CloudError::other(format!(
                    "generation {} was not a safe integer; please file a bug report",
                    number_to_string(generation)
                )));
            }
            let mut st = self.st.borrow_mut();
            // An older read must not regress past a committed root.
            if st
                .last_hash_gen
                .as_ref()
                .is_none_or(|(_, g)| generation >= *g)
            {
                st.last_hash_gen = Some((hash, generation));
                st.schema_version = Some(schema_version);
            }
        }
        let st = self.st.borrow();
        let (hash, generation) = st.last_hash_gen.clone().unwrap();
        Ok((hash, generation, st.schema_version.unwrap_or(4)))
    }

    /// `#putRootHash`: forgets the cached root on a generation conflict.
    async fn put_root_hash(&self, hash: &str, generation: f64) -> Result<()> {
        if !valid_hash(hash) {
            return Err(validation(format!("rootHash was not a valid hash: {hash}")));
        }
        let body =
            stringify(&json!({"hash": hash, "generation": num(generation), "broadcast": true}));
        let url = format!("{}/sync/v3/root", self.opts().raw_host);
        let result = async {
            let resp = self
                .fetch("PUT", url, Some(body.into_bytes()), vec![])
                .await?;
            let loaded = parse(&resp.body).map_err(CloudError::other)?;
            let r =
                schema::parse(&loaded, schema::updated_root_hash(), "root").map_err(validation)?;
            let new_gen = r["generation"].as_f64().unwrap();
            if !super::js::is_safe_integer(new_gen) {
                return Err(CloudError::other("new generation was not a safe integer"));
            }
            Ok((r["hash"].as_str().unwrap().to_owned(), new_gen))
        }
        .await;
        match result {
            Ok(root) => {
                self.st.borrow_mut().last_hash_gen = Some(root);
                Ok(())
            }
            Err(e) => {
                if e == CloudError::Generation {
                    self.st.borrow_mut().last_hash_gen = None;
                }
                Err(e)
            }
        }
    }

    /// `raw.getHash`: the bytes of a file, from the cache when known.
    async fn get_hash(&self, file_name: &str, hash: &str) -> Result<Vec<u8>> {
        if let Some(Some(bytes)) = self.st.borrow().cache.get(hash) {
            return Ok(bytes.clone());
        }
        if !valid_hash(hash) {
            return Err(validation(format!("hash was not a valid hash: {hash}")));
        }
        let url = format!("{}/sync/v3/files/{hash}", self.opts().raw_host);
        let resp = self
            .fetch(
                "GET",
                url,
                None,
                vec![("rm-filename".into(), file_name.into())],
            )
            .await?;
        let mut st = self.st.borrow_mut();
        let keep = resp.body.len() <= st.opts.max_cached_bytes;
        st.cache
            .insert(hash.into(), keep.then(|| resp.body.clone()));
        Ok(resp.body)
    }

    /// `raw.getEntries`: the lines of an index (`id` is the item id, or
    /// "root").
    async fn get_entries(&self, id: &str, hash: &str) -> Result<Entries> {
        let raw = self.get_hash(&format!("{id}.docSchema"), hash).await?;
        let mut text = String::from_utf8_lossy(&raw).into_owned();
        // rawFile.slice(0, -1)
        text.pop();
        let mut lines = text.split('\n');
        let version = lines.next().unwrap_or("");
        match version {
            "3" => Ok(Entries {
                entries: lines.map(parse_entry_line).collect::<Result<_>>()?,
                id: None,
                size: None,
            }),
            "4" => {
                let info = lines
                    .next()
                    .ok_or_else(|| CloudError::other("missing info line for schema version 4"))?;
                let parts: Vec<&str> = info.split(':').collect();
                if parts.len() < 4 || parts[0] != "0" {
                    return Err(CloudError::other(format!(
                        "schema 4 info line '{info}' was not formatted correctly"
                    )));
                }
                let entries: Vec<RawEntry> = lines.map(parse_entry_line).collect::<Result<_>>()?;
                let count = parse_int(parts[2]);
                if count != entries.len() as f64 {
                    return Err(CloudError::other(format!(
                        "schema 4 expected {} entries, but found {}",
                        parts[2],
                        entries.len()
                    )));
                }
                Ok(Entries {
                    entries,
                    id: Some(parts[1].into()),
                    size: Some(parse_int(parts[3])),
                })
            }
            v => Err(CloudError::other(format!(
                "schema version {v} not supported"
            ))),
        }
    }

    async fn get_json(&self, entry: &RawEntry, t: &schema::T, what: &str) -> Result<Value> {
        let raw = self.get_hash(&entry.id, &entry.hash).await?;
        let loaded = parse(&raw).map_err(CloudError::other)?;
        schema::parse(&loaded, t, what).map_err(validation)
    }

    /// `raw.putFile`: uploads a file unless its hash is known.
    async fn put_file(&self, file_name: &str, bytes: Vec<u8>) -> Result<RawEntry> {
        let hash = sha256_hex(&bytes);
        self.put_hashed(file_name, &hash, &bytes).await?;
        Ok(RawEntry {
            id: file_name.into(),
            hash,
            kind: 0,
            subfiles: 0.0,
            size: bytes.len() as f64,
        })
    }

    async fn put_hashed(&self, file_name: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        if self.st.borrow().cache.contains_key(hash) {
            return Ok(());
        }
        let crc = base64(&crc32c(bytes).to_be_bytes());
        let url = format!("{}/sync/v3/files/{hash}", self.opts().raw_host);
        self.fetch(
            "PUT",
            url,
            Some(bytes.to_vec()),
            vec![
                ("rm-filename".into(), file_name.into()),
                ("x-goog-hash".into(), format!("crc32c={crc}")),
            ],
        )
        .await?;
        let mut st = self.st.borrow_mut();
        let keep = bytes.len() <= st.opts.max_cached_bytes;
        st.cache.insert(hash.into(), keep.then(|| bytes.to_vec()));
        Ok(())
    }

    async fn put_json(&self, file_name: &str, v: &Value) -> Result<RawEntry> {
        self.put_file(file_name, stringify(v).into_bytes()).await
    }

    /// `raw.putEntries`: writes an index (schema 3 or 4).
    async fn put_entries(&self, id: &str, entries: &[RawEntry], schema: u32) -> Result<RawEntry> {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| locale_compare(&a.id, &b.id));
        let size: f64 = sorted.iter().map(|e| e.size).sum();
        let mut records = format!("{schema}\n");
        if schema == 4 {
            let name = if id == ROOT_LIST { "." } else { id };
            records += &format!("0:{name}:{}:{}\n", sorted.len(), number_to_string(size));
        }
        for e in &sorted {
            let kind = if schema == 4 { 0 } else { e.kind };
            records += &format!(
                "{}:{kind}:{}:{}:{}\n",
                e.hash,
                e.id,
                number_to_string(e.subfiles),
                number_to_string(e.size)
            );
        }
        let buff = records.into_bytes();
        let hash = match schema {
            3 => {
                let mut cat = Vec::new();
                for e in &sorted {
                    cat.extend(
                        from_hex(&e.hash)
                            .ok_or_else(|| CloudError::other(format!("invalid hash {}", e.hash)))?,
                    );
                }
                sha256_hex(&cat)
            }
            4 => sha256_hex(&buff),
            v => return Err(CloudError::other(format!("unsupported schema version {v}"))),
        };
        self.put_hashed(&format!("{id}.docSchema"), &hash, &buff)
            .await?;
        Ok(RawEntry {
            id: id.into(),
            hash,
            kind: if schema > 3 { 0 } else { 80_000_000 },
            subfiles: sorted.len() as f64,
            size,
        })
    }

    /// `#withRetry`: re-runs a root update on generation conflicts.
    async fn with_retry<T, F, Fut>(&self, mut op: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if std::mem::replace(&mut self.st.borrow_mut().busy, true) {
            return Err(CloudError::other(
                "another root update is running on this session",
            ));
        }
        let max = self.opts().max_generation_retries;
        let mut attempt = 0;
        let result = loop {
            match op().await {
                Err(CloudError::Generation) if attempt < max => {
                    self.io.sleep(backoff_ms(attempt, GENERATION_BASE_MS)).await;
                    attempt += 1;
                }
                other => break other,
            }
        };
        self.st.borrow_mut().busy = false;
        result
    }

    /// `#writeRoot`
    async fn write_root(&self, entries: &[RawEntry], generation: f64) -> Result<()> {
        let root = self.put_entries(ROOT_LIST, entries, 4).await?;
        self.put_root_hash(&root.hash, generation).await
    }

    /// `#commit`: adds an uploaded item to the root.
    async fn commit(&self, entry: &RawEntry) -> Result<()> {
        self.with_retry(|| async {
            let (root, generation, _) = self.root_hash(false).await?;
            let mut entries = self.get_entries(ROOT_LIST, &root).await?.entries;
            entries.push(entry.clone());
            self.write_root(&entries, generation).await
        })
        .await
    }

    /// `#editEntry`: replaces an item (found by id and hash) with `edit`'s
    /// result, in one root update.
    async fn edit_entry<F, Fut>(&self, r: &ItemRef, refresh: bool, edit: F) -> Result<ItemRef>
    where
        F: Fn(ItemRef, u32) -> Fut,
        Fut: Future<Output = Result<RawEntry>>,
    {
        self.with_retry(|| async {
            let (root, generation, schema) = self.root_hash(refresh).await?;
            let mut entries = self.get_entries(ROOT_LIST, &root).await?.entries;
            let i = entries
                .iter()
                .position(|e| e.id == r.id && e.hash == r.hash)
                .ok_or_else(|| CloudError::HashNotFound {
                    hash: r.hash.clone(),
                })?;
            let item = ItemRef {
                id: entries[i].id.clone(),
                hash: r.hash.clone(),
            };
            let new_entry = edit(item.clone(), schema).await?;
            let new_hash = new_entry.hash.clone();
            entries[i] = new_entry;
            self.write_root(&entries, generation).await?;
            Ok(ItemRef {
                id: item.id,
                hash: new_hash,
            })
        })
        .await
    }

    async fn find_entry(
        &self,
        r: &ItemRef,
        suffix: &str,
        what: &str,
    ) -> Result<(Vec<RawEntry>, usize)> {
        let entries = self.get_entries(&r.id, &r.hash).await?.entries;
        let i = entries
            .iter()
            .position(|e| e.id.ends_with(suffix))
            .ok_or_else(|| {
                CloudError::other(format!("couldn't find {what} for hash {}", r.hash))
            })?;
        Ok((entries, i))
    }

    /// `getContent`
    async fn get_content(&self, r: &ItemRef) -> Result<Value> {
        let (entries, i) = self.find_entry(r, ".content", "contents").await?;
        self.get_json(&entries[i], schema::content(), "content")
            .await
    }

    /// `getMetadata`
    async fn get_metadata(&self, r: &ItemRef) -> Result<Value> {
        let (entries, i) = self.find_entry(r, ".metadata", "metadata").await?;
        self.get_json(&entries[i], schema::metadata(), "metadata")
            .await
    }

    /// `listRefs`
    async fn list_refs(&self, refresh: bool) -> Result<Vec<ItemRef>> {
        let (root, _, _) = self.root_hash(refresh).await?;
        Ok(self
            .get_entries(ROOT_LIST, &root)
            .await?
            .entries
            .into_iter()
            .map(|e| ItemRef {
                id: e.id,
                hash: e.hash,
            })
            .collect())
    }

    /// `#convertEntry`: an item as `listItems` returns it.
    async fn convert_entry(&self, r: &ItemRef) -> Result<Value> {
        let entries = self.get_entries(&r.id, &r.hash).await?.entries;
        let meta_entry = entries
            .iter()
            .find(|e| e.id.ends_with(".metadata"))
            .ok_or_else(|| {
                CloudError::other(format!("couldn't find metadata for hash {}", r.hash))
            })?;
        let meta = self
            .get_json(meta_entry, schema::metadata(), "metadata")
            .await?;
        let content = match entries.iter().find(|e| e.id.ends_with(".content")) {
            Some(e) => self.get_json(e, schema::content(), "content").await?,
            None => json!({}),
        };
        let mut out = Map::new();
        let mut put = |k: &str, v: Option<&Value>| {
            if let Some(v) = v {
                out.insert(k.into(), v.clone());
            }
        };
        let id = json!(r.id);
        let hash = json!(r.hash);
        put("id", Some(&id));
        put("hash", Some(&hash));
        put("visibleName", meta.get("visibleName"));
        put("lastModified", meta.get("lastModified"));
        let ty;
        if content.get("templateVersion").is_some() {
            put("new", meta.get("new"));
            put("pinned", meta.get("pinned"));
            put("source", meta.get("source"));
            put("parent", meta.get("parent"));
            put("createdTime", meta.get("createdTime"));
            ty = json!("TemplateType");
        } else if content.get("fileType").is_none() {
            put("pinned", meta.get("pinned"));
            put("parent", meta.get("parent"));
            put("tags", content.get("tags"));
            ty = json!("CollectionType");
        } else {
            put("pinned", meta.get("pinned"));
            put("parent", meta.get("parent"));
            put("tags", content.get("tags"));
            let last_opened = meta.get("lastOpened").cloned().unwrap_or(json!(""));
            put("lastOpened", Some(&last_opened));
            put("fileType", content.get("fileType"));
            ty = json!("DocumentType");
        }
        put("type", Some(&ty));
        Ok(Value::Object(out))
    }

    /// `#editMetaRaw`
    async fn edit_meta_raw(
        &self,
        item: &ItemRef,
        update: &Map<String, Value>,
        schema: u32,
    ) -> Result<RawEntry> {
        let (mut entries, i) = self
            .find_entry(item, ".metadata", "metadata in entry hash")
            .await?;
        let mut meta = self
            .get_json(&entries[i], schema::metadata(), "metadata")
            .await?;
        let m = meta.as_object_mut().unwrap();
        assign(m, update);
        let version = m.get("version").and_then(Value::as_f64).unwrap_or(0.0) + 1.0;
        m.insert("version".into(), num(version));
        m.insert("metadatamodified".into(), json!(true));
        entries[i] = self.put_json(&entries[i].id.clone(), &meta).await?;
        self.put_entries(&item.id, &entries, schema).await
    }

    /// `#editContentRaw`
    async fn edit_content_raw(
        &self,
        item: &ItemRef,
        update: &Map<String, Value>,
        schema: u32,
    ) -> Result<RawEntry> {
        let (mut entries, i) = self
            .find_entry(item, ".content", "content in entry hash")
            .await?;
        let mut content = self
            .get_json(&entries[i], schema::content(), "content")
            .await?;
        assign(content.as_object_mut().unwrap(), update);
        entries[i] = self.put_json(&entries[i].id.clone(), &content).await?;
        self.put_entries(&item.id, &entries, schema).await
    }
}

/// `Object.assign(target, update)`: existing keys keep their place.
fn assign(target: &mut Map<String, Value>, update: &Map<String, Value>) {
    for k in super::js::js_key_order(update) {
        target.insert(k.clone(), update[k].clone());
    }
}

/// The lines of an index file.
pub struct Entries {
    pub entries: Vec<RawEntry>,
    /// Schema 4: the index's own id and total size.
    pub id: Option<String>,
    pub size: Option<f64>,
}

impl Entries {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "entries".into(),
            Value::Array(self.entries.iter().map(RawEntry::to_json).collect()),
        );
        if let Some(id) = &self.id {
            m.insert("id".into(), json!(id));
        }
        if let Some(size) = self.size {
            m.insert("size".into(), num(size));
        }
        Value::Object(m)
    }
}

fn parse_entry_line(line: &str) -> Result<RawEntry> {
    let parts: Vec<&str> = line.split(':').collect();
    let bad = || CloudError::other(format!("line '{line}' was not formatted correctly"));
    if parts.len() < 5 {
        return Err(bad());
    }
    let kind = match parts[1] {
        "0" => 0,
        "80000000" => 80_000_000,
        _ => return Err(bad()),
    };
    Ok(RawEntry {
        hash: parts[0].into(),
        kind,
        id: parts[2].into(),
        subfiles: parse_int(parts[3]),
        size: parse_int(parts[4]),
    })
}

/// Options of [`Session::put_pdf`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PutPdfOptions {
    pub parent: String,
    pub tags: Option<Vec<String>>,
    pub refresh: bool,
}

fn ref_json(r: &ItemRef) -> Output {
    Output::Json(json!({"id": r.id, "hash": r.hash}))
}

impl Session {
    /// A session from a session token (see [`auth`]).
    pub fn new(session_token: &str, opts: SessionOptions) -> Result<Session> {
        let device_id = token_device_id(session_token)?;
        Ok(Session {
            device_id,
            st: Rc::new(RefCell::new(State {
                token: session_token.into(),
                opts,
                cache: HashMap::new(),
                last_hash_gen: None,
                schema_version: None,
                busy: false,
            })),
        })
    }

    /// The device id the session token was issued to.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    fn op<F, Fut>(&self, f: F) -> Operation
    where
        F: FnOnce(Ctx) -> Fut,
        Fut: Future<Output = Result<Output>> + 'static,
    {
        let io = Rc::new(Io::default());
        let ctx = Ctx {
            io: io.clone(),
            st: self.st.clone(),
        };
        Operation::new(io, f(ctx))
    }

    /// Every item (documents, folders, templates) with its metadata, as
    /// rmapi-js's `listItems`.
    pub fn list_items(&self, refresh: bool) -> Operation {
        self.op(move |c| async move {
            let refs = c.list_refs(refresh).await?;
            let mut items = Vec::new();
            for r in &refs {
                items.push(c.convert_entry(r).await?);
            }
            Ok(Output::Json(Value::Array(items)))
        })
    }

    /// The item ids and hashes of the root.
    pub fn list_refs(&self, refresh: bool) -> Operation {
        self.op(move |c| async move {
            let refs = c.list_refs(refresh).await?;
            Ok(Output::Json(serde_json::to_value(refs).unwrap()))
        })
    }

    /// An item's `.content`, validated (`getContent`).
    pub fn get_content(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move { c.get_content(&r).await.map(Output::Json) })
    }

    /// An item's `.metadata`, validated (`getMetadata`).
    pub fn get_metadata(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move { c.get_metadata(&r).await.map(Output::Json) })
    }

    /// The files of an item (`raw.getEntries`; `id` "root" for the root).
    pub fn get_entries(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move {
            Ok(Output::Json(c.get_entries(&r.id, &r.hash).await?.to_json()))
        })
    }

    /// A file's bytes (`raw.getHash`; `id` is the file name).
    pub fn get_hash(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move { c.get_hash(&r.id, &r.hash).await.map(Output::Bytes) })
    }

    /// Uploads a PDF (`putPdf`). `id` and `page_id` are fresh uuids, `now`
    /// the time in ms.
    pub fn put_pdf(
        &self,
        name: String,
        bytes: Vec<u8>,
        opts: PutPdfOptions,
        id: String,
        page_id: String,
        now: f64,
    ) -> Operation {
        self.op(move |c| async move {
            c.put_pdf(&name, bytes, &opts, &id, &page_id, now)
                .await
                .map(|r| ref_json(&r))
        })
    }

    /// Creates a folder (`putFolder`).
    pub fn put_folder(&self, name: String, parent: String, id: String, now: f64) -> Operation {
        self.op(move |c| async move {
            c.put_folder(&name, &parent, &id, now, false)
                .await
                .map(|r| ref_json(&r))
        })
    }

    /// Merges `update` into a document's `.content` (`updateDocument`).
    pub fn update_document(&self, r: ItemRef, update: Map<String, Value>) -> Operation {
        self.op(move |c| async move {
            c.update_document(&r, &update, false)
                .await
                .map(|r| ref_json(&r))
        })
    }

    /// Moves an item (`move`); `parent` "trash" for the trash.
    pub fn move_item(&self, r: ItemRef, parent: String) -> Operation {
        self.op(move |c| async move {
            if !valid_id(&parent) {
                return Err(validation(format!(
                    "parent must be a valid document id: {parent}"
                )));
            }
            let mut update = Map::new();
            update.insert("parent".into(), json!(parent));
            c.edit_entry(&r, false, |item, schema| {
                let (c, update) = (c.clone(), update.clone());
                async move { c.edit_meta_raw(&item, &update, schema).await }
            })
            .await
            .map(|r| ref_json(&r))
        })
    }

    /// Removes an item from the root, for good (`purge`).
    pub fn purge(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move {
            c.with_retry(|| async {
                let (root, generation, _) = c.root_hash(false).await?;
                let entries = c.get_entries(ROOT_LIST, &root).await?.entries;
                let n = entries.len();
                let kept: Vec<RawEntry> = entries
                    .into_iter()
                    .filter(|e| !(e.id == r.id && e.hash == r.hash))
                    .collect();
                if kept.len() == n {
                    return Err(CloudError::HashNotFound {
                        hash: r.hash.clone(),
                    });
                }
                c.write_root(&kept, generation).await
            })
            .await?;
            Ok(Output::None)
        })
    }

    /// Replaces the `.rm` files of some pages in one commit (`putRmPages`).
    /// Fails with [`CloudError::HashNotFound`] if the document changed.
    pub fn put_rm_pages(&self, r: ItemRef, pages: Vec<(String, Vec<u8>)>) -> Operation {
        self.op(move |c| async move {
            if pages.is_empty() {
                return Ok(ref_json(&r));
            }
            c.edit_entry(&r, false, |item, schema| {
                let (c, pages) = (c.clone(), pages.clone());
                async move { c.put_page_files(&item, &pages, schema).await }
            })
            .await
            .map(|r| ref_json(&r))
        })
    }

    /// Uploads a PDF with its full page list (the plugin's `uploadPdf`).
    /// `uuids`: the document id, the placeholder page id, then one per page.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_pdf(
        &self,
        name: String,
        bytes: Vec<u8>,
        folder_id: String,
        tags: Vec<String>,
        page_count: Option<u32>,
        page_tags: Map<String, Value>,
        uuids: Vec<String>,
        now: f64,
    ) -> Operation {
        self.op(move |c| async move {
            let need = 2 + page_count.unwrap_or(0) as usize;
            if uuids.len() < need {
                return Err(validation(format!("uploadPdf needs {need} uuids")));
            }
            let opts = PutPdfOptions {
                parent: folder_id,
                tags: Some(tags),
                refresh: false,
            };
            let r = c
                .put_pdf(&name, bytes, &opts, &uuids[0], &uuids[1], now)
                .await?;
            let Some(n) = page_count.filter(|&n| n >= 1) else {
                return Ok(ref_json(&r));
            };
            let pages: Vec<String> = uuids[2..2 + n as usize].to_vec();
            let mut tags_out = Vec::new();
            for k in super::js::js_key_order(&page_tags) {
                let idx = super::js::js_number(k);
                let page_id = (idx >= 0.0 && idx.fract() == 0.0)
                    .then(|| pages.get(idx as usize))
                    .flatten();
                if let (Some(page_id), Some(names)) = (page_id, page_tags[k].as_array()) {
                    for name in names {
                        tags_out
                            .push(json!({"name": name, "pageId": page_id, "timestamp": num(now)}));
                    }
                }
            }
            let mut update = Map::new();
            update.insert("originalPageCount".into(), json!(n));
            update.insert("pageCount".into(), json!(n));
            update.insert("pages".into(), json!(pages));
            update.insert(
                "redirectionPageMap".into(),
                Value::Array((0..n).map(|i| json!(i)).collect()),
            );
            update.insert("pageTags".into(), Value::Array(tags_out));
            c.update_document(&r, &update, false)
                .await
                .map(|r| ref_json(&r))
        })
    }

    /// Resolves a "/"-separated folder path to a folder id, creating the
    /// missing folders (the plugin's `ensureFolder`). `uuids`: ids for the
    /// folders to create, used in order.
    pub fn ensure_folder(&self, path: String, uuids: Vec<String>, now: f64) -> Operation {
        self.op(move |c| async move {
            let parts: Vec<String> = path
                .split('/')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() {
                return Ok(Output::Json(json!("")));
            }
            let refs = c.list_refs(true).await?;
            let mut items = Vec::new();
            for r in &refs {
                items.push(c.convert_entry(r).await?);
            }
            let mut uuids = uuids.into_iter();
            let mut parent = String::new();
            for name in parts {
                let found = items.iter().find(|e| {
                    e["type"] == "CollectionType"
                        && e["visibleName"] == name.as_str()
                        && e.get("parent").and_then(Value::as_str).unwrap_or("") == parent
                });
                parent = match found {
                    Some(e) => e["id"].as_str().unwrap().to_owned(),
                    None => {
                        let id = uuids
                            .next()
                            .ok_or_else(|| validation("ensureFolder: not enough uuids"))?;
                        c.put_folder(&name, &parent, &id, now, false).await?.id
                    }
                };
            }
            Ok(Output::Json(json!(parent)))
        })
    }

    /// The annotated pages of a document, parsed, with its page tags (the
    /// plugin's `fetchAnnotations`).
    pub fn fetch_annotations(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move {
            let doc = c.read_doc(&r).await?;
            let mut pages = Vec::new();
            for page_id in &doc.page_ids {
                let Some(entry) = doc.rm_by_page.get(page_id) else {
                    continue;
                };
                let Ok(bytes) = c.get_hash(&entry.id, &entry.hash).await else {
                    // rmapi-js: a failed read skips the page
                    continue;
                };
                let Ok(page) = crate::page::parse_page(&bytes) else {
                    continue;
                };
                if page.strokes.is_empty() && page.highlights.is_empty() {
                    continue;
                }
                pages.push(json!({
                    "pdfPageIndex": doc.pdf_index[page_id],
                    "pageUuid": page_id,
                    "page": page,
                }));
            }
            let tags = super::doc::page_tags(doc.content.as_ref(), &doc.page_ids);
            Ok(Output::Json(json!({
                "pages": pages,
                "pageTags": tags,
                "content": doc.content,
            })))
        })
    }

    /// For each PDF page, its page uuid and `.rm` file (the plugin's
    /// `mapPdfPages`), as `[pdfIndex, {pageUuid, rmEntry?}]` pairs.
    pub fn map_pdf_pages(&self, r: ItemRef) -> Operation {
        self.op(move |c| async move {
            let doc = c.read_doc(&r).await?;
            let mut out: Vec<(f64, Value)> = Vec::new();
            for page_id in &doc.page_ids {
                let mut v = Map::new();
                v.insert("pageUuid".into(), json!(page_id));
                if let Some(e) = doc.rm_by_page.get(page_id) {
                    v.insert("rmEntry".into(), json!({"id": e.id, "hash": e.hash}));
                }
                let idx = doc.pdf_index[page_id];
                // A Map: a later page with the same index replaces the value
                match out.iter_mut().find(|(i, _)| *i == idx) {
                    Some(slot) => slot.1 = Value::Object(v),
                    None => out.push((idx, Value::Object(v))),
                }
            }
            Ok(Output::Json(Value::Array(
                out.into_iter().map(|(i, v)| json!([num(i), v])).collect(),
            )))
        })
    }
}

struct Doc {
    content: Option<Value>,
    page_ids: Vec<String>,
    pdf_index: HashMap<String, f64>,
    rm_by_page: HashMap<String, RawEntry>,
}

impl Ctx {
    /// `#putFile`: a new document from a file.
    async fn put_pdf(
        &self,
        name: &str,
        bytes: Vec<u8>,
        opts: &PutPdfOptions,
        id: &str,
        page_id: &str,
        now: f64,
    ) -> Result<ItemRef> {
        let parent = &opts.parent;
        if !parent.is_empty() && !valid_id(parent) {
            return Err(validation(format!(
                "parent must be a valid document id: {parent}"
            )));
        }
        let now_s = number_to_string(now.trunc());
        let metadata = json!({
            "parent": parent, "pinned": false, "lastModified": now_s, "createdTime": now_s,
            "type": "DocumentType", "visibleName": name, "lastOpened": "0", "lastOpenedPage": 0,
        });
        let tags: Vec<Value> = opts
            .tags
            .iter()
            .flatten()
            .map(|t| json!({"name": t, "timestamp": num(now)}))
            .collect();
        let content = json!({
            "coverPageNumber": -1, "documentMetadata": {}, "extraMetadata": {},
            "lineHeight": -1, "margins": 125, "orientation": "portrait", "fileType": "pdf",
            "formatVersion": 1, "tags": tags, "fontName": "", "textAlignment": "justify",
            "textScale": 1, "zoomMode": "bestFit", "originalPageCount": 1, "pageCount": 1,
            "pageTags": [], "pages": [page_id], "redirectionPageMap": [0],
            "sizeInBytes": bytes.len().to_string(),
        });
        let (_, _, schema) = self.root_hash(opts.refresh).await?;
        let content_entry = self.put_json(&format!("{id}.content"), &content).await?;
        let metadata_entry = self.put_json(&format!("{id}.metadata"), &metadata).await?;
        let pagedata_entry = self
            .put_file(&format!("{id}.pagedata"), b"\n".to_vec())
            .await?;
        let file_entry = self.put_file(&format!("{id}.pdf"), bytes).await?;
        let entry = self
            .put_entries(
                id,
                &[content_entry, metadata_entry, pagedata_entry, file_entry],
                schema,
            )
            .await?;
        self.commit(&entry).await?;
        Ok(ItemRef {
            id: id.into(),
            hash: entry.hash,
        })
    }

    /// `putFolder`
    async fn put_folder(
        &self,
        name: &str,
        parent: &str,
        id: &str,
        now: f64,
        refresh: bool,
    ) -> Result<ItemRef> {
        if !parent.is_empty() && !valid_id(parent) {
            return Err(validation(format!(
                "parent must be a valid document id: {parent}"
            )));
        }
        let now_s = number_to_string(now.trunc());
        let content = json!({"tags": []});
        let metadata = json!({
            "lastModified": now_s, "createdTime": now_s, "parent": parent, "pinned": false,
            "type": "CollectionType", "visibleName": name,
        });
        let (_, _, schema) = self.root_hash(refresh).await?;
        let content_entry = self.put_json(&format!("{id}.content"), &content).await?;
        let metadata_entry = self.put_json(&format!("{id}.metadata"), &metadata).await?;
        let entry = self
            .put_entries(id, &[content_entry, metadata_entry], schema)
            .await?;
        self.commit(&entry).await?;
        Ok(ItemRef {
            id: id.into(),
            hash: entry.hash,
        })
    }

    /// `updateDocument`: the content edit, then the type check (rmapi-js runs
    /// both at once).
    async fn update_document(
        &self,
        r: &ItemRef,
        update: &Map<String, Value>,
        refresh: bool,
    ) -> Result<ItemRef> {
        self.edit_entry(r, refresh, |item, schema| {
            let (c, update) = (self.clone(), update.clone());
            async move {
                let new_entry = c.edit_content_raw(&item, &update, schema).await?;
                let meta = c.get_metadata(&item).await?;
                if meta["type"] != "DocumentType" {
                    return Err(CloudError::other(format!(
                        "expected type DocumentType but got {} for hash {}",
                        meta["type"].as_str().unwrap_or(""),
                        item.hash
                    )));
                }
                Ok(new_entry)
            }
        })
        .await
    }

    /// `#putPageFilesRaw` for `.rm` files: `bytes` are uploaded as given
    /// (rmapi-js re-serializes its parsed scene, the same bytes for pages
    /// that round-trip).
    async fn put_page_files(
        &self,
        item: &ItemRef,
        pages: &[(String, Vec<u8>)],
        schema: u32,
    ) -> Result<RawEntry> {
        let (mut entries, i) = self.find_entry(item, ".content", "contents").await?;
        let content = self
            .get_json(&entries[i], schema::content(), "content")
            .await?;
        let order = super::doc::page_order(&content);
        for (page_id, _) in pages {
            if !order.contains(page_id) {
                return Err(CloudError::other(format!(
                    "document {} has no page {page_id}",
                    item.id
                )));
            }
        }
        for (page_id, bytes) in pages {
            let name = format!("{}/{page_id}.rm", item.id);
            let entry = self.put_file(&name, bytes.clone()).await?;
            match entries.iter().position(|e| e.id == entry.id) {
                Some(j) => entries[j] = entry,
                None => entries.push(entry),
            }
        }
        self.put_entries(&item.id, &entries, schema).await
    }

    /// rmdoc.ts's `readDoc`.
    async fn read_doc(&self, r: &ItemRef) -> Result<Doc> {
        let entries = self.get_entries(&r.id, &r.hash).await?.entries;
        let content = self.get_content(r).await.ok();
        let mut rm_by_page = HashMap::new();
        for e in entries {
            if let Some(stem) = e.id.strip_suffix(".rm") {
                let base = stem.rsplit('/').next().unwrap_or(stem).to_owned();
                rm_by_page.insert(base, e);
            }
        }
        let page_ids = super::doc::ordered_page_ids(content.as_ref());
        let pdf_index = super::doc::pdf_index_by_page_id(content.as_ref(), &page_ids);
        Ok(Doc {
            content,
            page_ids,
            pdf_index,
            rm_by_page,
        })
    }
}

/// Registers a device with a one-time code (rmapi-js's `register`); the
/// result is the device token. `device_id` is a fresh uuid.
pub fn register(code: &str, device_desc: &str, device_id: &str, auth_host: &str) -> Operation {
    if code.encode_utf16().count() != 8 {
        return Operation::ready(Err(CloudError::other(format!(
            "code should be length 8, but was {}",
            code.encode_utf16().count()
        ))));
    }
    let io = Rc::new(Io::default());
    let request = Request {
        method: "POST".into(),
        url: format!("{auth_host}/token/json/2/device/new"),
        headers: vec![
            ("Authorization".into(), "Bearer".into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: Some(
            stringify(&json!({"code": code, "deviceDesc": device_desc, "deviceID": device_id}))
                .into_bytes(),
        ),
    };
    let fetch = io.fetch(request);
    Operation::new(io, async move {
        let resp = fetch.await;
        if let Some(message) = resp.network_error {
            return Err(CloudError::Network { message });
        }
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        if resp.ok() {
            Ok(Output::Text(body))
        } else {
            Err(CloudError::Register {
                status: resp.status,
                status_text: resp.status_text,
                body,
            })
        }
    })
}

/// A session token for a device token (rmapi-js's `auth`).
pub fn auth(device_token: &str, auth_host: &str) -> Operation {
    let io = Rc::new(Io::default());
    let fetch = io.fetch(Request {
        method: "POST".into(),
        url: format!("{auth_host}/token/json/2/user/new"),
        headers: vec![("Authorization".into(), format!("Bearer {device_token}"))],
        body: None,
    });
    Operation::new(io, async move {
        let resp = fetch.await;
        if let Some(message) = resp.network_error {
            return Err(CloudError::Network { message });
        }
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        if resp.ok() {
            Ok(Output::Text(body))
        } else {
            Err(CloudError::Auth {
                status: resp.status,
                status_text: resp.status_text,
                body,
            })
        }
    })
}
