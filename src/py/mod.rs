//! Python bindings (`remarkable_annotations` on PyPI), built by maturin with
//! the `extension-module` feature.

mod cloud;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::scene;

/// A parsed `.rm` v6 scene.
#[pyclass(name = "Scene", module = "remarkable_annotations")]
struct PyScene(scene::Scene);

#[pymethods]
impl PyScene {
    /// Parses a v6 `.rm` file.
    #[new]
    fn new(data: &[u8]) -> PyResult<Self> {
        scene::Scene::parse(data)
            .map(PyScene)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Serializes the scene back to `.rm` bytes.
    fn serialize<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.0.serialize())
    }

    /// The blocks as JSON, in rmapi-js's shape.
    fn blocks_json(&self) -> String {
        scene::view::blocks_json(&self.0).to_string()
    }
}

#[pymodule]
fn remarkable_annotations(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyScene>()?;
    cloud::register(m)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
