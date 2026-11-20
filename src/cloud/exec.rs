//! Runs an operation's `async` code without a runtime: each time it needs
//! I/O, it stops and hands the host a [`Step`]; the host performs it and
//! resumes the operation with the outcome.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use serde::{Deserialize, Serialize};

use super::error::CloudError;

/// An HTTP request for the host to perform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub method: String,
    pub url: String,
    /// Header names and values, in order.
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<u8>>,
}

/// What the host got for a [`Request`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// HTTP status; ignored when `network_error` is set.
    pub status: u16,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub body: Vec<u8>,
    /// Set when the request failed without a response (`fetch` threw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_error: Option<String>,
}

impl Response {
    pub fn ok(&self) -> bool {
        self.network_error.is_none() && (200..300).contains(&self.status)
    }
}

/// The result of an operation.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
    Text(String),
    None,
}

/// One thing an operation waits for.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    /// Perform this request.
    Request(Request),
    /// Wait (backoff before a retry). rmapi-js waits a random time below
    /// `ms` (full jitter).
    Sleep { ms: f64 },
}

/// What the operation needs next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Perform this request, then call [`Operation::resume`] with the response.
    Request(Request),
    /// Wait (backoff before a retry), then call [`Operation::resume`] with
    /// no response. rmapi-js waits a random time below `ms` (full jitter).
    Sleep {
        ms: f64,
    },
    /// Several independent requests or waits, which the host may run
    /// concurrently; then call [`Operation::resume_batch`] with one outcome
    /// per item, in order (`None` for a sleep).
    Batch(Vec<Wait>),
    Done(Output),
    Failed(CloudError),
}

enum Completion {
    Response(Response),
    Woken,
}

/// The channel between the operation's futures and the host: what they wait
/// for, and what the host gave back, by wait id.
#[derive(Default)]
pub(crate) struct Io {
    next_id: Cell<u64>,
    queued: RefCell<Vec<(u64, Wait)>>,
    done: RefCell<HashMap<u64, Completion>>,
}

/// A request or a sleep, as a future.
pub(crate) struct Waiting {
    io: Rc<Io>,
    id: u64,
    wait: Option<Wait>,
}

impl Future for Waiting {
    type Output = Option<Response>;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Response>> {
        if let Some(w) = self.wait.take() {
            self.io.queued.borrow_mut().push((self.id, w));
            return Poll::Pending;
        }
        match self.io.done.borrow_mut().remove(&self.id) {
            Some(Completion::Response(r)) => Poll::Ready(Some(r)),
            Some(Completion::Woken) => Poll::Ready(None),
            None => Poll::Pending,
        }
    }
}

impl Io {
    fn wait(self: &Rc<Self>, wait: Wait) -> Waiting {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        Waiting {
            io: self.clone(),
            id,
            wait: Some(wait),
        }
    }

    /// A request; the future owns what it needs (no borrow of `self`).
    pub(crate) fn fetch(
        self: &Rc<Self>,
        request: Request,
    ) -> impl Future<Output = Response> + 'static {
        let w = self.wait(Wait::Request(request));
        async move { w.await.unwrap_or_default() }
    }

    /// A sleep of up to `ms` (the host applies the jitter).
    pub(crate) fn sleep(self: &Rc<Self>, ms: f64) -> impl Future<Output = ()> + 'static {
        let w = self.wait(Wait::Sleep { ms });
        async move {
            w.await;
        }
    }
}

/// Runs futures concurrently (their requests go to the host together) and
/// gives their outputs in order, like `Promise.all` once all have settled.
pub(crate) async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    struct JoinAll<F: Future> {
        futures: Vec<Option<Pin<Box<F>>>>,
        out: Vec<Option<F::Output>>,
    }
    // The futures are boxed and the outputs never pinned: moving the
    // struct is fine.
    impl<F: Future> Unpin for JoinAll<F> {}
    impl<F: Future> Future for JoinAll<F> {
        type Output = Vec<F::Output>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            for (slot, out) in this.futures.iter_mut().zip(this.out.iter_mut()) {
                if let Some(f) = slot {
                    if let Poll::Ready(v) = f.as_mut().poll(cx) {
                        *out = Some(v);
                        *slot = None;
                    }
                }
            }
            if this.futures.iter().all(Option::is_none) {
                Poll::Ready(this.out.iter_mut().map(|o| o.take().unwrap()).collect())
            } else {
                Poll::Pending
            }
        }
    }
    let n = futures.len();
    JoinAll {
        futures: futures.into_iter().map(|f| Some(Box::pin(f))).collect(),
        out: (0..n).map(|_| None).collect(),
    }
    .await
}

/// Runs two futures concurrently, like `Promise.all([a, b])` once both have
/// settled.
pub(crate) async fn join2<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    enum Either<X, Y> {
        A(X),
        B(Y),
    }
    type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
    let futures: Vec<Boxed<'_, Either<A::Output, B::Output>>> = vec![
        Box::pin(async move { Either::A(a.await) }),
        Box::pin(async move { Either::B(b.await) }),
    ];
    let mut out = join_all(futures).await.into_iter();
    match (out.next(), out.next()) {
        (Some(Either::A(x)), Some(Either::B(y))) => (x, y),
        _ => unreachable!(),
    }
}

type Task = Pin<Box<dyn Future<Output = Result<Output, CloudError>>>>;

/// A running operation.
pub struct Operation {
    io: Rc<Io>,
    task: Option<Task>,
    /// The waits handed to the host, awaiting their outcome.
    waiting: Vec<u64>,
}

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    // SAFETY: the vtable functions do nothing and never use the data pointer.
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

impl Operation {
    pub(crate) fn new(
        io: Rc<Io>,
        task: impl Future<Output = Result<Output, CloudError>> + 'static,
    ) -> Self {
        Self {
            io,
            task: Some(Box::pin(task)),
            waiting: Vec::new(),
        }
    }

    /// An operation that ends at once (no I/O needed, or invalid input).
    pub(crate) fn ready(result: Result<Output, CloudError>) -> Self {
        Self::new(Rc::new(Io::default()), async move { result })
    }

    fn poll(&mut self) -> Step {
        let Some(task) = self.task.as_mut() else {
            return Step::Failed(CloudError::other("operation already finished"));
        };
        let waker = noop_waker();
        match task.as_mut().poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(result) => {
                self.task = None;
                match result {
                    Ok(out) => Step::Done(out),
                    Err(e) => Step::Failed(e),
                }
            }
            Poll::Pending => {
                let queued = std::mem::take(&mut *self.io.queued.borrow_mut());
                self.waiting = queued.iter().map(|(id, _)| *id).collect();
                let mut waits: Vec<Wait> = queued.into_iter().map(|(_, w)| w).collect();
                match waits.len() {
                    0 => {
                        self.task = None;
                        Step::Failed(CloudError::other("operation stalled"))
                    }
                    1 => match waits.pop().unwrap() {
                        Wait::Request(r) => Step::Request(r),
                        Wait::Sleep { ms } => Step::Sleep { ms },
                    },
                    _ => Step::Batch(waits),
                }
            }
        }
    }

    /// Starts the operation.
    pub fn start(&mut self) -> Step {
        self.poll()
    }

    /// Resumes after a request (with its response) or a sleep (with `None`).
    pub fn resume(&mut self, response: Option<Response>) -> Step {
        self.resume_batch(vec![response])
    }

    /// Resumes after a batch: one outcome per item, in order (`None` for a
    /// sleep).
    pub fn resume_batch(&mut self, responses: Vec<Option<Response>>) -> Step {
        let ids = std::mem::take(&mut self.waiting);
        if ids.len() != responses.len() {
            self.task = None;
            return Step::Failed(CloudError::other(format!(
                "expected {} outcome(s), got {}",
                ids.len(),
                responses.len()
            )));
        }
        let mut done = self.io.done.borrow_mut();
        for (id, r) in ids.into_iter().zip(responses) {
            done.insert(
                id,
                match r {
                    Some(r) => Completion::Response(r),
                    None => Completion::Woken,
                },
            );
        }
        drop(done);
        self.poll()
    }

    /// Runs the operation with a blocking transport (tests, simple hosts).
    pub fn run(mut self, mut send: impl FnMut(&Request) -> Response) -> Result<Output, CloudError> {
        let mut step = self.start();
        loop {
            step = match step {
                Step::Request(r) => {
                    let resp = send(&r);
                    self.resume(Some(resp))
                }
                Step::Sleep { .. } => self.resume(None),
                Step::Batch(waits) => {
                    let outcomes = waits
                        .iter()
                        .map(|w| match w {
                            Wait::Request(r) => Some(send(r)),
                            Wait::Sleep { .. } => None,
                        })
                        .collect();
                    self.resume_batch(outcomes)
                }
                Step::Done(out) => return Ok(out),
                Step::Failed(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str) -> Request {
        Request {
            method: "GET".into(),
            url: url.into(),
            headers: vec![],
            body: None,
        }
    }

    fn answer(body: &str) -> Option<Response> {
        Some(Response {
            status: 200,
            body: body.as_bytes().to_vec(),
            ..Default::default()
        })
    }

    fn text(r: Response) -> String {
        String::from_utf8(r.body).unwrap()
    }

    /// Outcomes go to the future that asked, whatever the order.
    #[test]
    fn join_all_batches_and_orders() {
        let io = Rc::new(Io::default());
        let (a, b, c) = (io.fetch(get("a")), io.fetch(get("b")), io.sleep(10.0));
        let mut op = Operation::new(io.clone(), async move {
            let (ab, ()) = join2(join_all(vec![a, b]), c).await;
            let ab: Vec<String> = ab.into_iter().map(text).collect();
            Ok(Output::Text(ab.join(",")))
        });
        let Step::Batch(waits) = op.start() else {
            panic!("expected a batch")
        };
        assert_eq!(
            waits,
            vec![
                Wait::Request(get("a")),
                Wait::Request(get("b")),
                Wait::Sleep { ms: 10.0 }
            ]
        );
        let step = op.resume_batch(vec![answer("A"), answer("B"), None]);
        assert_eq!(step, Step::Done(Output::Text("A,B".into())));
    }

    /// A single wait is still a plain request; a batch needs one outcome per
    /// item.
    #[test]
    fn single_waits_and_wrong_counts() {
        let io = Rc::new(Io::default());
        let a = io.fetch(get("a"));
        let mut op = Operation::new(io.clone(), async move { Ok(Output::Text(text(a.await))) });
        assert_eq!(op.start(), Step::Request(get("a")));
        assert_eq!(op.resume(answer("x")), Step::Done(Output::Text("x".into())));

        let io = Rc::new(Io::default());
        let (a, b) = (io.fetch(get("a")), io.fetch(get("b")));
        let mut op = Operation::new(io.clone(), async move {
            join_all(vec![a, b]).await;
            Ok(Output::None)
        });
        assert!(matches!(op.start(), Step::Batch(_)));
        assert!(matches!(op.resume(answer("only one")), Step::Failed(_)));
    }

    /// Futures that finish at different times: each later wait is its own
    /// step, and errors come back in order.
    #[test]
    fn join_all_staggered_with_errors() {
        let io = Rc::new(Io::default());
        let make = |io: &Rc<Io>, url: &'static str, twice: bool| {
            let io = io.clone();
            async move {
                let first = text(io.fetch(get(url)).await);
                if twice {
                    let second = text(io.fetch(get(url)).await);
                    return Err::<String, String>(format!("{first}{second}"));
                }
                Ok(first)
            }
        };
        let futures = vec![
            Box::pin(make(&io, "a", false)) as Pin<Box<dyn Future<Output = _>>>,
            Box::pin(make(&io, "b", true)),
        ];
        let mut op = Operation::new(io.clone(), async move {
            let out = join_all(futures).await;
            Ok(Output::Text(format!("{out:?}")))
        });
        assert!(matches!(op.start(), Step::Batch(w) if w.len() == 2));
        assert_eq!(
            op.resume_batch(vec![answer("1"), answer("2")]),
            Step::Request(get("b"))
        );
        assert_eq!(
            op.resume(answer("3")),
            Step::Done(Output::Text(r#"[Ok("1"), Err("23")]"#.into()))
        );
    }
}
