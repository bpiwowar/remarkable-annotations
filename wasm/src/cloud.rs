//! Bindings of the sans-I/O cloud client (`remarkable_annotations::cloud`).
//!
//! An operation is driven by the host:
//!
//! ```js
//! const op = session.listItems(false);
//! for (let step = op.start(); ; ) {
//!   if (step.type === "request") {
//!     const r = await fetch(step.url, { method: step.method, headers: step.headers, body: step.body });
//!     step = op.resume({ status: r.status, statusText: r.statusText, body: new Uint8Array(await r.arrayBuffer()) });
//!   } else if (step.type === "sleep") {
//!     await new Promise((ok) => setTimeout(ok, Math.random() * step.ms));
//!     step = op.resume();
//!   } else if (step.type === "done") return step;   // step.json / step.bytes / step.text
//!   else throw step.error;                             // {kind, message, ...}
//! }
//! ```
//!
//! A network failure is `op.resume({ networkError: String(e) })`.

use js_sys::{Array, Object, Reflect, Uint8Array};
use remarkable_annotations::cloud::{
    self, CloudError, ItemRef, Operation, Output, PutPdfOptions, Response, SessionOptions, Step,
};
use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

fn set(o: &Object, k: &str, v: impl Into<JsValue>) {
    Reflect::set(o, &JsValue::from_str(k), &v.into()).unwrap();
}

fn error_value(e: &CloudError) -> JsValue {
    let o = Object::new();
    if let Ok(Value::Object(m)) = serde_json::to_value(e) {
        for (k, v) in m {
            match v {
                Value::String(s) => set(&o, &k, s),
                Value::Number(n) => set(&o, &k, n.as_f64().unwrap_or(f64::NAN)),
                other => set(&o, &k, other.to_string()),
            }
        }
    }
    set(&o, "message", e.to_string());
    o.into()
}

fn step_value(step: Step) -> JsValue {
    let o = Object::new();
    match step {
        Step::Request(r) => {
            set(&o, "type", "request");
            set(&o, "method", r.method);
            set(&o, "url", r.url);
            let headers = Array::new();
            for (k, v) in r.headers {
                headers.push(&Array::of2(&k.into(), &v.into()));
            }
            set(&o, "headers", headers);
            if let Some(b) = r.body {
                set(&o, "body", Uint8Array::from(b.as_slice()));
            }
        }
        Step::Sleep { ms } => {
            set(&o, "type", "sleep");
            set(&o, "ms", ms);
        }
        Step::Done(out) => {
            set(&o, "type", "done");
            match out {
                Output::Json(v) => set(&o, "json", cloud::to_js_json(&v)),
                Output::Bytes(b) => set(&o, "bytes", Uint8Array::from(b.as_slice())),
                Output::Text(t) => set(&o, "text", t),
                Output::None => {}
            }
        }
        Step::Failed(e) => {
            set(&o, "type", "error");
            set(&o, "error", error_value(&e));
        }
    }
    o.into()
}

fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
}

/// A running cloud operation.
#[wasm_bindgen]
pub struct CloudOperation(Operation);

#[wasm_bindgen]
impl CloudOperation {
    /// The first step.
    pub fn start(&mut self) -> JsValue {
        step_value(self.0.start())
    }

    /// The next step, after a request (`{status, statusText, body}` or
    /// `{networkError}`) or a sleep (no argument).
    pub fn resume(&mut self, response: JsValue) -> JsValue {
        let response = (!response.is_undefined() && !response.is_null()).then(|| Response {
            status: get(&response, "status").as_f64().unwrap_or(0.0) as u16,
            status_text: get(&response, "statusText").as_string().unwrap_or_default(),
            body: get(&response, "body")
                .dyn_into::<Uint8Array>()
                .map(|b| b.to_vec())
                .unwrap_or_default(),
            network_error: get(&response, "networkError").as_string(),
        });
        step_value(self.0.resume(response))
    }
}

fn json_arg(s: &str) -> Result<Value, JsError> {
    serde_json::from_str(s).map_err(|e| JsError::new(&format!("invalid JSON argument: {e}")))
}

fn json_object(s: &str) -> Result<Map<String, Value>, JsError> {
    match json_arg(s)? {
        Value::Object(m) => Ok(m),
        _ => Err(JsError::new("expected a JSON object")),
    }
}

fn item(id: String, hash: String) -> ItemRef {
    ItemRef { id, hash }
}

/// Registers a device with a one-time code; done with `text`: the device
/// token. `deviceId` is a fresh uuid.
#[wasm_bindgen(js_name = cloudRegister)]
pub fn cloud_register(
    code: &str,
    device_desc: &str,
    device_id: &str,
    auth_host: Option<String>,
) -> CloudOperation {
    let host = auth_host.unwrap_or_else(|| cloud::AUTH_HOST.into());
    CloudOperation(cloud::register(code, device_desc, device_id, &host))
}

/// A session token for a device token; done with `text`.
#[wasm_bindgen(js_name = cloudAuth)]
pub fn cloud_auth(device_token: &str, auth_host: Option<String>) -> CloudOperation {
    let host = auth_host.unwrap_or_else(|| cloud::AUTH_HOST.into());
    CloudOperation(cloud::auth(device_token, &host))
}

/// A cloud session (session token and cache). Run one operation at a time.
#[wasm_bindgen]
pub struct CloudSession(cloud::Session);

#[wasm_bindgen]
impl CloudSession {
    /// `options`: JSON `{rawHost, uploadHost, maxGenerationRetries,
    /// maxTransientRetries, maxCachedBytes}`, all optional.
    #[wasm_bindgen(constructor)]
    pub fn new(session_token: &str, options: Option<String>) -> Result<CloudSession, JsError> {
        let opts: SessionOptions = match options {
            Some(o) => serde_json::from_str(&o).map_err(|e| JsError::new(&e.to_string()))?,
            None => SessionOptions::default(),
        };
        cloud::Session::new(session_token, opts)
            .map(CloudSession)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(getter, js_name = deviceId)]
    pub fn device_id(&self) -> String {
        self.0.device_id().into()
    }

    /// Every item; `json`: an array of entries as rmapi-js's `listItems`.
    #[wasm_bindgen(js_name = listItems)]
    pub fn list_items(&self, refresh: bool) -> CloudOperation {
        CloudOperation(self.0.list_items(refresh))
    }

    #[wasm_bindgen(js_name = listRefs)]
    pub fn list_refs(&self, refresh: bool) -> CloudOperation {
        CloudOperation(self.0.list_refs(refresh))
    }

    #[wasm_bindgen(js_name = getContent)]
    pub fn get_content(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.get_content(item(id, hash)))
    }

    #[wasm_bindgen(js_name = getMetadata)]
    pub fn get_metadata(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.get_metadata(item(id, hash)))
    }

    /// An item's files (`id` "root" for the root index).
    #[wasm_bindgen(js_name = getEntries)]
    pub fn get_entries(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.get_entries(item(id, hash)))
    }

    /// A file's bytes (`fileName` as in its entry).
    #[wasm_bindgen(js_name = getHash)]
    pub fn get_hash(&self, file_name: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.get_hash(item(file_name, hash)))
    }

    /// `options`: JSON `{parent, tags, refresh}`; done with `json` `{id, hash}`.
    #[wasm_bindgen(js_name = putPdf)]
    pub fn put_pdf(
        &self,
        name: String,
        bytes: Vec<u8>,
        options: &str,
        id: String,
        page_id: String,
        now: f64,
    ) -> Result<CloudOperation, JsError> {
        let opts: PutPdfOptions =
            serde_json::from_str(options).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(CloudOperation(
            self.0.put_pdf(name, bytes, opts, id, page_id, now),
        ))
    }

    #[wasm_bindgen(js_name = putFolder)]
    pub fn put_folder(&self, name: String, parent: String, id: String, now: f64) -> CloudOperation {
        CloudOperation(self.0.put_folder(name, parent, id, now))
    }

    /// `update`: a JSON object merged into the `.content`.
    #[wasm_bindgen(js_name = updateDocument)]
    pub fn update_document(
        &self,
        id: String,
        hash: String,
        update: &str,
    ) -> Result<CloudOperation, JsError> {
        Ok(CloudOperation(
            self.0.update_document(item(id, hash), json_object(update)?),
        ))
    }

    /// Moves an item (`parent` "trash" for the trash).
    #[wasm_bindgen(js_name = move)]
    pub fn move_item(&self, id: String, hash: String, parent: String) -> CloudOperation {
        CloudOperation(self.0.move_item(item(id, hash), parent))
    }

    pub fn purge(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.purge(item(id, hash)))
    }

    /// Writes pages' `.rm` files in one commit: `pageIds[i]` gets `pages[i]`
    /// (Uint8Arrays). Fails with `hashNotFound` if the document changed.
    #[wasm_bindgen(js_name = putRmPages)]
    pub fn put_rm_pages(
        &self,
        id: String,
        hash: String,
        page_ids: Vec<String>,
        pages: Array,
    ) -> CloudOperation {
        let pages = page_ids
            .into_iter()
            .zip(pages.iter())
            .map(|(p, b)| (p, Uint8Array::new(&b).to_vec()))
            .collect();
        CloudOperation(self.0.put_rm_pages(item(id, hash), pages))
    }

    /// Uploads a PDF with its page list. `pageTags`: JSON `{"<pdf page
    /// index>": [names]}`; `uuids`: document id, placeholder page, then one
    /// per page.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = uploadPdf)]
    pub fn upload_pdf(
        &self,
        name: String,
        bytes: Vec<u8>,
        folder_id: String,
        tags: Vec<String>,
        page_count: Option<u32>,
        page_tags: &str,
        uuids: Vec<String>,
        now: f64,
    ) -> Result<CloudOperation, JsError> {
        Ok(CloudOperation(self.0.upload_pdf(
            name,
            bytes,
            folder_id,
            tags,
            page_count,
            json_object(page_tags)?,
            uuids,
            now,
        )))
    }

    /// The id of a folder path, creating missing folders with `uuids`.
    #[wasm_bindgen(js_name = ensureFolder)]
    pub fn ensure_folder(&self, path: String, uuids: Vec<String>, now: f64) -> CloudOperation {
        CloudOperation(self.0.ensure_folder(path, uuids, now))
    }

    /// `json`: `{pages: [{pdfPageIndex, pageUuid, page}], pageTags, content}`.
    #[wasm_bindgen(js_name = fetchAnnotations)]
    pub fn fetch_annotations(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.fetch_annotations(item(id, hash)))
    }

    /// `json`: `[[pdfIndex, {pageUuid, rmEntry?}], …]`.
    #[wasm_bindgen(js_name = mapPdfPages)]
    pub fn map_pdf_pages(&self, id: String, hash: String) -> CloudOperation {
        CloudOperation(self.0.map_pdf_pages(item(id, hash)))
    }
}

/// A `.rm` page's annotations as JSON (the page format of
/// `fetchAnnotations`), for hosts comparing or post-processing pages.
#[wasm_bindgen(js_name = cloudParsePage)]
pub fn cloud_parse_page(bytes: &[u8]) -> Result<String, JsError> {
    let page = remarkable_annotations::page::parse_page(bytes)
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(cloud::to_js_json(&serde_json::to_value(page).unwrap()))
}
