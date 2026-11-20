//! Runs an operation's `async` code without a runtime: each time it needs
//! I/O, it stops and hands the host a [`Step`]; the host performs it and
//! resumes the operation with the outcome.

use std::cell::RefCell;
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
    Done(Output),
    Failed(CloudError),
}

#[derive(Default)]
enum Slot {
    #[default]
    Empty,
    Request(Request),
    Sleep(f64),
    Response(Response),
    Woken,
}

/// The channel between the operation's futures and the host.
#[derive(Default)]
pub(crate) struct Io {
    slot: RefCell<Slot>,
}

pub(crate) struct Fetch {
    io: Rc<Io>,
    request: Option<Request>,
}

impl Future for Fetch {
    type Output = Response;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Response> {
        if let Some(r) = self.request.take() {
            *self.io.slot.borrow_mut() = Slot::Request(r);
            return Poll::Pending;
        }
        match std::mem::take(&mut *self.io.slot.borrow_mut()) {
            Slot::Response(r) => Poll::Ready(r),
            other => {
                *self.io.slot.borrow_mut() = other;
                Poll::Pending
            }
        }
    }
}

pub(crate) struct Sleep {
    io: Rc<Io>,
    ms: Option<f64>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if let Some(ms) = self.ms.take() {
            *self.io.slot.borrow_mut() = Slot::Sleep(ms);
            return Poll::Pending;
        }
        match std::mem::take(&mut *self.io.slot.borrow_mut()) {
            Slot::Woken => Poll::Ready(()),
            other => {
                *self.io.slot.borrow_mut() = other;
                Poll::Pending
            }
        }
    }
}

impl Io {
    pub(crate) fn fetch(self: &Rc<Self>, request: Request) -> Fetch {
        Fetch {
            io: self.clone(),
            request: Some(request),
        }
    }

    pub(crate) fn sleep(self: &Rc<Self>, ms: f64) -> Sleep {
        Sleep {
            io: self.clone(),
            ms: Some(ms),
        }
    }
}

type Task = Pin<Box<dyn Future<Output = Result<Output, CloudError>>>>;

/// A running operation.
pub struct Operation {
    io: Rc<Io>,
    task: Option<Task>,
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
            Poll::Pending => match std::mem::take(&mut *self.io.slot.borrow_mut()) {
                Slot::Request(r) => Step::Request(r),
                Slot::Sleep(ms) => Step::Sleep { ms },
                _ => {
                    self.task = None;
                    Step::Failed(CloudError::other("operation stalled"))
                }
            },
        }
    }

    /// Starts the operation.
    pub fn start(&mut self) -> Step {
        self.poll()
    }

    /// Resumes after a request (with its response) or a sleep (with `None`).
    pub fn resume(&mut self, response: Option<Response>) -> Step {
        *self.io.slot.borrow_mut() = match response {
            Some(r) => Slot::Response(r),
            None => Slot::Woken,
        };
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
                Step::Done(out) => return Ok(out),
                Step::Failed(e) => return Err(e),
            }
        }
    }
}
