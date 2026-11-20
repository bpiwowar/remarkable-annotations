//! Geometry, shape detection and PDF page sizes against the TypeScript
//! implementation (tests/reference/geometry-dump.mjs → geometry.json).

mod common;

use std::fs;

use common::{diff, dir};
use remarkable_annotations::geometry::{
    ink_width, pdf_path_to_rm, pdf_rect_to_rm, pdf_width_to_rm, read_pdf_page_sizes,
    rm_rect_to_pdf, stroke_to_path, PageSize,
};
use remarkable_annotations::page::Point;
use remarkable_annotations::scene::Rect;
use remarkable_annotations::shapes::detect_shape;
use serde_json::{json, Value};

fn reference() -> Value {
    serde_json::from_str(&fs::read_to_string(dir("tests/fixtures/geometry.json")).unwrap()).unwrap()
}

fn check(got: Value, expected: &Value, what: &str) {
    if let Some(d) = diff(&got, expected, what) {
        panic!("{d}");
    }
}

#[test]
fn pdf_page_sizes() {
    let r = reference();
    for (name, expected) in r["pdf"].as_object().unwrap() {
        let bytes = fs::read(dir("tests/fixtures/pdf").join(name)).unwrap();
        check(json!(read_pdf_page_sizes(&bytes)), expected, name);
    }
}

#[test]
fn shapes() {
    let r = reference();
    for (i, case) in r["shapes"].as_array().unwrap().iter().enumerate() {
        let points: Vec<Point> = serde_json::from_value(case["points"].clone()).unwrap();
        check(
            json!(detect_shape(&points)),
            &case["shape"],
            &format!("shape {i}"),
        );
    }
}

#[test]
fn conversions() {
    let r = reference();
    for c in r["conversions"].as_array().unwrap() {
        let size: PageSize = serde_json::from_value(c["size"].clone()).unwrap();
        for (path, back) in c["paths"]
            .as_array()
            .unwrap()
            .iter()
            .zip(c["back"].as_array().unwrap())
        {
            let flat: Vec<f64> = serde_json::from_value(path.clone()).unwrap();
            check(json!(pdf_path_to_rm(&flat, &size)), back, "pdfPathToRm");
            let points: Vec<Point> = serde_json::from_value(back.clone()).unwrap();
            // strokeToPath of the round trip is the path again (rounding)
            check(json!(stroke_to_path(&points, &size)), path, "strokeToPath");
        }
        let rects: Vec<Value> = c["rects"].as_array().unwrap().clone();
        for (pdf, back) in rects.iter().zip(c["rectsBack"].as_array().unwrap()) {
            let p: [f64; 4] = serde_json::from_value(pdf.clone()).unwrap();
            check(json!(pdf_rect_to_rm(p, &size)), back, "pdfRectToRm");
            let rm: Rect = serde_json::from_value(back.clone()).unwrap();
            let again = rm_rect_to_pdf(&rm, &size);
            for (a, b) in again.iter().zip(p) {
                assert!((a - b).abs() < 1e-9, "rmRectToPdf round trip");
            }
        }
    }
    for pair in r["inkWidth"].as_array().unwrap() {
        assert_eq!(
            ink_width(pair[0].as_f64().unwrap()),
            pair[1].as_f64().unwrap()
        );
    }
    for pair in r["pdfWidthToRm"].as_array().unwrap() {
        assert_eq!(
            pdf_width_to_rm(pair[0].as_f64().unwrap()),
            pair[1].as_f64().unwrap()
        );
    }
}

/// Damaged PDFs never panic.
#[test]
fn damaged_pdfs() {
    let mut x: u64 = 7;
    for entry in fs::read_dir(dir("tests/fixtures/pdf")).unwrap() {
        let original = fs::read(entry.unwrap().path()).unwrap();
        for _ in 0..200 {
            let mut bytes = original.clone();
            for _ in 0..3 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                if bytes.is_empty() {
                    break;
                }
                let at = (x % bytes.len() as u64) as usize;
                match x % 3 {
                    0 => bytes[at] = (x >> 8) as u8,
                    1 => bytes.truncate(at),
                    _ => {
                        bytes.remove(at);
                    }
                }
            }
            assert!(!read_pdf_page_sizes(&bytes).is_empty());
        }
    }
}
