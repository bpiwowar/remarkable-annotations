//! Page coordinates ↔ PDF points.
//!
//! The tablet renders a PDF at a fixed resolution ([`RM_DPI`]) and stores
//! coordinates in those device pixels, x from the page centre and y from the
//! top. PDF points have their origin bottom-left. The conversion is one scale,
//! whatever the page size:
//!
//! ```text
//! pdfX = rmX * 72/RM_DPI + x0 + width/2
//! pdfY = y0 + height - rmY * 72/RM_DPI
//! ```
//!
//! Checked against a tablet export: highlight rectangles matched the PDF's
//! vectors within 1 pt.

use serde::{Deserialize, Serialize};

use crate::js::{max, min, round as js_round};
use crate::page::Point;
use crate::pdf;
use crate::scene::Rect;

/// A page's visible box, in PDF points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
    /// Lower-left corner of the visible (crop) box; 0 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x0: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y0: Option<f64>,
}

impl PageSize {
    /// US Letter, when nothing better is known.
    pub const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
        x0: None,
        y0: None,
    };

    fn origin(&self) -> (f64, f64) {
        (self.x0.unwrap_or(0.0), self.y0.unwrap_or(0.0))
    }
}

/// The tablet's rendering resolution (rM2, rMPP); the calibration constant.
pub const RM_DPI: f64 = 226.0;
const PT_PER_RM: f64 = 72.0 / RM_DPI;

/// Pen width (device units) to PDF points, fitted on an export: highlighter
/// width 120 → 11.34 pt, fineliner 16 → 1.51 pt.
const RM_WIDTH_FACTOR: f64 = 0.0945;

/// A pen width (device units) as an ink width in PDF points (at least 0.5).
pub fn ink_width(rm_point_width: f64) -> f64 {
    max(
        0.5,
        js_round(rm_point_width * RM_WIDTH_FACTOR * 100.0) / 100.0,
    )
}

/// The inverse of [`ink_width`] (at least 1).
pub fn pdf_width_to_rm(pdf_width: f64) -> f64 {
    max(1.0, js_round(pdf_width / RM_WIDTH_FACTOR))
}

/// A page point as a PDF point `[x, y]`.
pub fn rm_to_pdf(x: f64, y: f64, page: &PageSize) -> [f64; 2] {
    let (x0, y0) = page.origin();
    [
        x * PT_PER_RM + x0 + page.width / 2.0,
        y0 + page.height - y * PT_PER_RM,
    ]
}

/// A PDF point as a page point `[x, y]`.
pub fn pdf_to_rm(x: f64, y: f64, page: &PageSize) -> [f64; 2] {
    let (x0, y0) = page.origin();
    [
        (x - x0 - page.width / 2.0) / PT_PER_RM,
        (y0 + page.height - y) / PT_PER_RM,
    ]
}

/// A page rectangle as a PDF rectangle `[x1, y1, x2, y2]`.
pub fn rm_rect_to_pdf(r: &Rect, page: &PageSize) -> [f64; 4] {
    let [ax, ay] = rm_to_pdf(r.x, r.y, page);
    let [bx, by] = rm_to_pdf(r.x + r.w, r.y + r.h, page);
    [min(ax, bx), min(ay, by), max(ax, bx), max(ay, by)]
}

/// A PDF rectangle `[x1, y1, x2, y2]` as a page rectangle.
pub fn pdf_rect_to_rm(rect: [f64; 4], page: &PageSize) -> Rect {
    let [ax, ay] = pdf_to_rm(rect[0], rect[3], page);
    let [bx, by] = pdf_to_rm(rect[2], rect[1], page);
    Rect {
        x: ax,
        y: ay,
        w: bx - ax,
        h: by - ay,
    }
}

fn round3(n: f64) -> f64 {
    js_round(n * 1000.0) / 1000.0
}

/// A stroke's points as a flat PDF path `[x1, y1, x2, y2, …]`, rounded to
/// 1/1000 pt.
pub fn stroke_to_path(points: &[Point], page: &PageSize) -> Vec<f64> {
    points
        .iter()
        .flat_map(|p| {
            let [x, y] = rm_to_pdf(p.x, p.y, page);
            [round3(x), round3(y)]
        })
        .collect()
}

/// The inverse of [`stroke_to_path`].
pub fn pdf_path_to_rm(path: &[f64], page: &PageSize) -> Vec<Point> {
    path.chunks_exact(2)
        .map(|c| {
            let [x, y] = pdf_to_rm(c[0], c[1], page);
            Point { x, y }
        })
        .collect()
}

/// The visible size of every page, in order. Walks the page tree; when that
/// fails, uses every plain-text `/MediaBox` in file order, then US Letter.
pub fn read_pdf_page_sizes(bytes: &[u8]) -> Vec<PageSize> {
    if let Some(boxes) = pdf::read_page_boxes(bytes) {
        return boxes
            .into_iter()
            .map(|b| PageSize {
                width: b.width,
                height: b.height,
                x0: Some(b.x0),
                y0: Some(b.y0),
            })
            .collect();
    }
    let sizes = pdf::scan_media_boxes(bytes);
    if sizes.is_empty() {
        vec![PageSize::LETTER]
    } else {
        sizes
    }
}

/// The size of page `index`, or of the last known page.
pub fn page_size_at(sizes: &[PageSize], index: usize) -> PageSize {
    match sizes.len() {
        0 => PageSize::LETTER,
        n => sizes[index.min(n - 1)],
    }
}
