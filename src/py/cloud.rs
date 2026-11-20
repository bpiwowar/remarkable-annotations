//! Python bindings of the sans-I/O cloud client.
//!
//! ```python
//! def perform(item):
//!     if item["type"] == "sleep":
//!         time.sleep(random.random() * item["ms"] / 1000)
//!         return None
//!     r = httpx.request(item["method"], item["url"], headers=item["headers"], content=item.get("body"))
//!     return {"status": r.status_code, "status_text": r.reason_phrase, "body": r.content}
//!
//! op = session.list_items(False)
//! step = op.start()
//! while step["type"] in ("request", "sleep", "batch"):
//!     if step["type"] == "batch":  # independent: may run concurrently
//!         step = op.resume_batch([perform(i) for i in step["items"]])
//!     else:
//!         step = op.resume(perform(step))
//! # step["type"] is "done" (step["json"] / step["bytes"] / step["text"]) or "error"
//! ```
//!
//! A network failure is `{"network_error": str(e)}` instead of a response.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use crate::cloud::{
    self, ItemRef, Operation, Output, PutPdfOptions, Request, Response, SessionOptions, Step, Wait,
};

fn value_err(e: impl ToString) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn request_dict<'py>(py: Python<'py>, d: &Bound<'py, PyDict>, r: Request) -> PyResult<()> {
    d.set_item("type", "request")?;
    d.set_item("method", r.method)?;
    d.set_item("url", r.url)?;
    d.set_item("headers", PyList::new(py, r.headers)?)?;
    if let Some(b) = r.body {
        d.set_item("body", PyBytes::new(py, &b))?;
    }
    Ok(())
}

fn step_dict<'py>(py: Python<'py>, step: Step) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    match step {
        Step::Request(r) => request_dict(py, &d, r)?,
        Step::Sleep { ms } => {
            d.set_item("type", "sleep")?;
            d.set_item("ms", ms)?;
        }
        Step::Batch(waits) => {
            d.set_item("type", "batch")?;
            let items = PyList::empty(py);
            for w in waits {
                let item = PyDict::new(py);
                match w {
                    Wait::Request(r) => request_dict(py, &item, r)?,
                    Wait::Sleep { ms } => {
                        item.set_item("type", "sleep")?;
                        item.set_item("ms", ms)?;
                    }
                }
                items.append(item)?;
            }
            d.set_item("items", items)?;
        }
        Step::Done(out) => {
            d.set_item("type", "done")?;
            match out {
                Output::Json(v) => d.set_item("json", cloud::to_js_json(&v))?,
                Output::Bytes(b) => d.set_item("bytes", PyBytes::new(py, &b))?,
                Output::Text(t) => d.set_item("text", t)?,
                Output::None => {}
            }
        }
        Step::Failed(e) => {
            d.set_item("type", "error")?;
            d.set_item("error", serde_json::to_string(&e).unwrap_or_default())?;
            d.set_item("message", e.to_string())?;
        }
    }
    Ok(d)
}

/// A running cloud operation.
#[pyclass(name = "CloudOperation", module = "remarkable_annotations", unsendable)]
pub struct PyCloudOperation(Operation);

#[pymethods]
impl PyCloudOperation {
    /// The first step (a dict with a "type").
    fn start<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        step_dict(py, self.0.start())
    }

    /// The next step, after a request (a dict with "status", "status_text",
    /// "body", or "network_error") or a sleep (None).
    #[pyo3(signature = (response=None))]
    fn resume<'py>(
        &mut self,
        py: Python<'py>,
        response: Option<Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let response = response.map(|d| response_of(&d)).transpose()?;
        step_dict(py, self.0.resume(response))
    }

    /// The next step, after a batch: one outcome per item, in order (a
    /// response dict, or None for a sleep).
    fn resume_batch<'py>(
        &mut self,
        py: Python<'py>,
        responses: Vec<Option<Bound<'py, PyDict>>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let responses = responses
            .iter()
            .map(|r| r.as_ref().map(response_of).transpose())
            .collect::<PyResult<Vec<_>>>()?;
        step_dict(py, self.0.resume_batch(responses))
    }
}

fn response_of(d: &Bound<'_, PyDict>) -> PyResult<Response> {
    Ok(Response {
        status: d
            .get_item("status")?
            .map(|v| v.extract())
            .transpose()?
            .unwrap_or(0),
        status_text: d
            .get_item("status_text")?
            .map(|v| v.extract())
            .transpose()?
            .unwrap_or_default(),
        body: d
            .get_item("body")?
            .map(|v| v.extract::<Vec<u8>>())
            .transpose()?
            .unwrap_or_default(),
        network_error: d
            .get_item("network_error")?
            .map(|v| v.extract())
            .transpose()?,
    })
}

fn item(id: String, hash: String) -> ItemRef {
    ItemRef { id, hash }
}

/// Registers a device with a one-time code; done with "text": the device token.
#[pyfunction]
#[pyo3(signature = (code, device_desc, device_id, auth_host=None))]
fn cloud_register(
    code: &str,
    device_desc: &str,
    device_id: &str,
    auth_host: Option<String>,
) -> PyCloudOperation {
    let host = auth_host.unwrap_or_else(|| cloud::AUTH_HOST.into());
    PyCloudOperation(cloud::register(code, device_desc, device_id, &host))
}

/// A session token for a device token; done with "text".
#[pyfunction]
#[pyo3(signature = (device_token, auth_host=None))]
fn cloud_auth(device_token: &str, auth_host: Option<String>) -> PyCloudOperation {
    let host = auth_host.unwrap_or_else(|| cloud::AUTH_HOST.into());
    PyCloudOperation(cloud::auth(device_token, &host))
}

/// A cloud session (session token and cache). Run one operation at a time.
#[pyclass(name = "CloudSession", module = "remarkable_annotations", unsendable)]
pub struct PyCloudSession(cloud::Session);

#[pymethods]
impl PyCloudSession {
    /// `options`: JSON `{rawHost, uploadHost, maxGenerationRetries,
    /// maxTransientRetries, maxCachedBytes}`, all optional.
    #[new]
    #[pyo3(signature = (session_token, options=None))]
    fn new(session_token: &str, options: Option<&str>) -> PyResult<Self> {
        let opts: SessionOptions = match options {
            Some(o) => serde_json::from_str(o).map_err(value_err)?,
            None => SessionOptions::default(),
        };
        cloud::Session::new(session_token, opts)
            .map(PyCloudSession)
            .map_err(value_err)
    }

    #[getter]
    fn device_id(&self) -> String {
        self.0.device_id().into()
    }

    #[pyo3(signature = (refresh=false))]
    fn list_items(&self, refresh: bool) -> PyCloudOperation {
        PyCloudOperation(self.0.list_items(refresh))
    }

    #[pyo3(signature = (refresh=false))]
    fn list_refs(&self, refresh: bool) -> PyCloudOperation {
        PyCloudOperation(self.0.list_refs(refresh))
    }

    fn get_content(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.get_content(item(id, hash)))
    }

    fn get_metadata(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.get_metadata(item(id, hash)))
    }

    fn get_entries(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.get_entries(item(id, hash)))
    }

    fn get_hash(&self, file_name: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.get_hash(item(file_name, hash)))
    }

    /// `options`: JSON `{parent, tags, refresh}`.
    fn put_pdf(
        &self,
        name: String,
        data: Vec<u8>,
        options: &str,
        id: String,
        page_id: String,
        now: f64,
    ) -> PyResult<PyCloudOperation> {
        let opts: PutPdfOptions = serde_json::from_str(options).map_err(value_err)?;
        Ok(PyCloudOperation(
            self.0.put_pdf(name, data, opts, id, page_id, now),
        ))
    }

    fn put_folder(&self, name: String, parent: String, id: String, now: f64) -> PyCloudOperation {
        PyCloudOperation(self.0.put_folder(name, parent, id, now))
    }

    /// `update`: a JSON object merged into the `.content`.
    fn update_document(
        &self,
        id: String,
        hash: String,
        update: &str,
    ) -> PyResult<PyCloudOperation> {
        let update = match serde_json::from_str(update).map_err(value_err)? {
            serde_json::Value::Object(m) => m,
            _ => return Err(value_err("update must be a JSON object")),
        };
        Ok(PyCloudOperation(
            self.0.update_document(item(id, hash), update),
        ))
    }

    #[pyo3(name = "move")]
    fn move_item(&self, id: String, hash: String, parent: String) -> PyCloudOperation {
        PyCloudOperation(self.0.move_item(item(id, hash), parent))
    }

    fn purge(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.purge(item(id, hash)))
    }

    /// `pages`: (page id, `.rm` bytes) pairs.
    fn put_rm_pages(
        &self,
        id: String,
        hash: String,
        pages: Vec<(String, Vec<u8>)>,
    ) -> PyCloudOperation {
        PyCloudOperation(self.0.put_rm_pages(item(id, hash), pages))
    }

    /// `page_tags`: JSON `{"<pdf page index>": [names]}`.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (name, data, folder_id, tags, page_count, page_tags, uuids, now))]
    fn upload_pdf(
        &self,
        name: String,
        data: Vec<u8>,
        folder_id: String,
        tags: Vec<String>,
        page_count: Option<u32>,
        page_tags: &str,
        uuids: Vec<String>,
        now: f64,
    ) -> PyResult<PyCloudOperation> {
        let page_tags = match serde_json::from_str(page_tags).map_err(value_err)? {
            serde_json::Value::Object(m) => m,
            _ => return Err(value_err("page_tags must be a JSON object")),
        };
        Ok(PyCloudOperation(self.0.upload_pdf(
            name, data, folder_id, tags, page_count, page_tags, uuids, now,
        )))
    }

    fn ensure_folder(&self, path: String, uuids: Vec<String>, now: f64) -> PyCloudOperation {
        PyCloudOperation(self.0.ensure_folder(path, uuids, now))
    }

    fn fetch_annotations(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.fetch_annotations(item(id, hash)))
    }

    fn map_pdf_pages(&self, id: String, hash: String) -> PyCloudOperation {
        PyCloudOperation(self.0.map_pdf_pages(item(id, hash)))
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyCloudSession>()?;
    m.add_class::<PyCloudOperation>()?;
    m.add_function(wrap_pyfunction!(cloud_register, m)?)?;
    m.add_function(wrap_pyfunction!(cloud_auth, m)?)?;
    Ok(())
}
