//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

pub fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rm") {
            out.push(path);
        }
    }
}

/// Every `.rm` file under tests/fixtures/rm.
pub fn fixtures() -> Vec<PathBuf> {
    let mut v = Vec::new();
    collect(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rm"),
        &mut v,
    );
    v.sort();
    assert!(!v.is_empty(), "no fixtures");
    v
}

/// The first difference between two JSON values, numbers compared as f64
/// (JavaScript has a single number type).
pub fn diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (x.as_f64() != y.as_f64()).then(|| format!("{path}: {x} != {y}"))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: length {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (x, y))| diff(x, y, &format!("{path}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|k| match (x.get(k), y.get(k)) {
                (Some(x), Some(y)) => diff(x, y, &format!("{path}.{k}")),
                (x, y) => Some(format!("{path}.{k}: {x:?} != {y:?}")),
            })
        }
        _ => (a != b).then(|| format!("{path}: {a} != {b}")),
    }
}
