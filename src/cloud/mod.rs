//! A client for the reMarkable cloud that does no I/O ("sans-I/O").
//!
//! Each operation is an [`Operation`]: [`Operation::start`] it, and it
//! returns a [`Step`]: a [`Request`] for the host to perform (then
//! [`Operation::resume`] with the [`Response`]), a sleep (backoff before a
//! retry; resume with `None`), a batch of independent requests and sleeps
//! the host may run concurrently (where rmapi-js uses `Promise.all`; then
//! [`Operation::resume_batch`]), or the result. The host owns the HTTP stack,
//! the clock and the randomness (uuids, backoff jitter), so the same code runs
//! in a browser, privileged Firefox code, Node and Python.
//!
//! The protocol is rmapi-js's (MIT), ported operation by operation and
//! tested against it on a fake cloud (`tests/reference/cloud-check.mjs`).

mod client;
pub mod doc;
mod error;
mod exec;
mod js;
mod schema;

pub use client::{
    auth, register, Entries, ItemRef, PutPdfOptions, RawEntry, Session, SessionOptions, AUTH_HOST,
    RAW_HOST, UPLOAD_HOST,
};
pub use error::CloudError;
pub use exec::{Operation, Output, Request, Response, Step, Wait};

/// `JSON.stringify` of a result, as rmapi-js would write it.
pub fn to_js_json(v: &serde_json::Value) -> String {
    js::stringify(v)
}
