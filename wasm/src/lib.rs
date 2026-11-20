//! WebAssembly bindings, published to npm as `remarkable-annotations`
//! (built by `wasm/build-npm.sh`).
//!
//! Values cross as plain JavaScript objects (serde-wasm-bindgen), with the
//! Rust field names in camelCase; byte arrays are `Uint8Array`s. Ids are
//! `{ author, counter }`.

use remarkable_annotations::{geometry, page, scene, shapes};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let ser = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    Ok(value.serialize(&ser)?)
}

fn from_js<T: for<'de> Deserialize<'de>>(value: JsValue) -> Result<T, JsError> {
    Ok(serde_wasm_bindgen::from_value(value)?)
}

/// A parsed `.rm` v6 scene.
#[wasm_bindgen]
pub struct Scene(scene::Scene);

#[wasm_bindgen]
impl Scene {
    /// Parses a v6 `.rm` file.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Scene, JsError> {
        Ok(Scene(scene::Scene::parse(bytes)?))
    }

    /// Serializes the scene back to `.rm` bytes.
    pub fn serialize(&self) -> Vec<u8> {
        self.0.serialize()
    }

    /// The blocks as JSON, in rmapi-js's shape.
    #[wasm_bindgen(js_name = blocksJson)]
    pub fn blocks_json(&self) -> String {
        scene::view::blocks_json(&self.0).to_string()
    }
}

/// The strokes and highlights of a `.rm` v6 page (a `Page` object).
#[wasm_bindgen(js_name = parsePage)]
pub fn parse_page(bytes: &[u8]) -> Result<JsValue, JsError> {
    to_js(&page::parse_page(bytes)?)
}

#[derive(Serialize)]
struct Rebuilt {
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    ids: Vec<scene::CrdtId>,
}

/// Rewrites a page (see the Rust `page::rebuild_page`): returns
/// `{ bytes, ids }`.
#[wasm_bindgen(js_name = rebuildPage)]
pub fn rebuild_page(
    existing: &[u8],
    strokes: JsValue,
    highlights: JsValue,
    remove: JsValue,
    update: JsValue,
) -> Result<JsValue, JsError> {
    let strokes: Vec<page::Stroke> = from_js(strokes)?;
    let highlights: Vec<page::Highlight> = from_js(highlights)?;
    let remove: Vec<scene::CrdtId> = from_js(remove)?;
    let update: page::PageUpdate = from_js(update)?;
    let (bytes, ids) = page::rebuild_page(existing, &strokes, &highlights, &remove, &update)?;
    to_js(&Rebuilt { bytes, ids })
}

/// A copy of a page without its scene items.
#[wasm_bindgen(js_name = splitStructure)]
pub fn split_structure(bytes: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(page::split_structure(bytes)?)
}

#[derive(Serialize)]
struct Structure {
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    update: page::PageUpdate,
    authors: Vec<(u16, String)>,
}

/// A blank page: `{ bytes, update, authors }`.
#[wasm_bindgen(js_name = blankStructure)]
pub fn blank_structure(author: u8) -> Result<JsValue, JsError> {
    let s = page::blank_structure(author);
    to_js(&Structure {
        bytes: s.bytes,
        update: s.update,
        authors: s.authors,
    })
}

/// The shape (`{ kind, bbox }`) the points describe, or `undefined`.
#[wasm_bindgen(js_name = detectShape)]
pub fn detect_shape(points: JsValue) -> Result<JsValue, JsError> {
    let points: Vec<page::Point> = from_js(points)?;
    to_js(&shapes::detect_shape(&points))
}

/// The visible size of every page of a PDF.
#[wasm_bindgen(js_name = readPdfPageSizes)]
pub fn read_pdf_page_sizes(bytes: &[u8]) -> Result<JsValue, JsError> {
    to_js(&geometry::read_pdf_page_sizes(bytes))
}

/// A page point as a PDF point `[x, y]`.
#[wasm_bindgen(js_name = rmToPdf)]
pub fn rm_to_pdf(x: f64, y: f64, page: JsValue) -> Result<Vec<f64>, JsError> {
    Ok(geometry::rm_to_pdf(x, y, &from_js(page)?).to_vec())
}

/// A PDF point as a page point `[x, y]`.
#[wasm_bindgen(js_name = pdfToRm)]
pub fn pdf_to_rm(x: f64, y: f64, page: JsValue) -> Result<Vec<f64>, JsError> {
    Ok(geometry::pdf_to_rm(x, y, &from_js(page)?).to_vec())
}

/// A page rectangle `{ x, y, w, h }` as a PDF rectangle `[x1, y1, x2, y2]`.
#[wasm_bindgen(js_name = rmRectToPdf)]
pub fn rm_rect_to_pdf(rect: JsValue, page: JsValue) -> Result<Vec<f64>, JsError> {
    Ok(geometry::rm_rect_to_pdf(&from_js(rect)?, &from_js(page)?).to_vec())
}

/// A PDF rectangle `[x1, y1, x2, y2]` as a page rectangle.
#[wasm_bindgen(js_name = pdfRectToRm)]
pub fn pdf_rect_to_rm(rect: Vec<f64>, page: JsValue) -> Result<JsValue, JsError> {
    let r: [f64; 4] = rect
        .try_into()
        .map_err(|_| JsError::new("a PDF rectangle has 4 numbers"))?;
    to_js(&geometry::pdf_rect_to_rm(r, &from_js(page)?))
}

/// A stroke's points as a flat PDF path.
#[wasm_bindgen(js_name = strokeToPath)]
pub fn stroke_to_path(points: JsValue, page: JsValue) -> Result<Vec<f64>, JsError> {
    let points: Vec<page::Point> = from_js(points)?;
    Ok(geometry::stroke_to_path(&points, &from_js(page)?))
}

/// A flat PDF path as page points.
#[wasm_bindgen(js_name = pdfPathToRm)]
pub fn pdf_path_to_rm(path: Vec<f64>, page: JsValue) -> Result<JsValue, JsError> {
    to_js(&geometry::pdf_path_to_rm(&path, &from_js(page)?))
}

/// A pen width (device units) as an ink width in PDF points.
#[wasm_bindgen(js_name = inkWidth)]
pub fn ink_width(rm_point_width: f64) -> f64 {
    geometry::ink_width(rm_point_width)
}

/// The inverse of `inkWidth`.
#[wasm_bindgen(js_name = pdfWidthToRm)]
pub fn pdf_width_to_rm(pdf_width: f64) -> f64 {
    geometry::pdf_width_to_rm(pdf_width)
}
