//! WebAssembly bindings, published to npm as `remarkable-annotations`
//! (built by `wasm/build-npm.sh`).

use remarkable_annotations::scene;
use wasm_bindgen::prelude::*;

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
