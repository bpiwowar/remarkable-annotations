//! Recognises the clean shapes the tablet's shape tool produces (draw, then
//! hold the pen): an axis-aligned rectangle is a closed 5-point polyline, an
//! oval a closed polyline sampled on an ellipse. Hand-drawn (wobbly) strokes
//! are not matched.

use serde::{Deserialize, Serialize};

use crate::js::{max, min};
use crate::page::Point;
use crate::scene::Rect;

/// The kind of a recognised shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
}

/// A recognised shape and its bounding box (page coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub bbox: Rect,
}

/// Tolerance, relative to the shape's smaller side.
const REL_TOL: f64 = 0.02;
/// Smallest side (device units, about 2.3 pt) of a shape.
const MIN_SIDE: f64 = 7.0;
/// Minimum sampling of an ellipse (the tablet uses many more points).
const MIN_ELLIPSE_POINTS: usize = 16;

/// The shape that `points` describe, if any (not freehand strokes, lines,
/// rotated rectangles or open paths).
pub fn detect_shape(points: &[Point]) -> Option<Shape> {
    if points.len() < 5 {
        return None;
    }
    let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
    let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        x0 = min(x0, p.x);
        x1 = max(x1, p.x);
        y0 = min(y0, p.y);
        y1 = max(y1, p.y);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    if min(w, h) < MIN_SIDE {
        return None;
    }
    let tol = max(1.0, min(w, h) * REL_TOL);
    let (first, last) = (points[0], points[points.len() - 1]);
    if (first.x - last.x).hypot(first.y - last.y) > tol {
        return None;
    }
    let bbox = Rect { x: x0, y: y0, w, h };
    if is_rectangle(points, x0, y0, x1, y1, tol) {
        Some(Shape {
            kind: ShapeKind::Rectangle,
            bbox,
        })
    } else if is_ellipse(points, x0, y0, x1, y1, tol) {
        Some(Shape {
            kind: ShapeKind::Ellipse,
            bbox,
        })
    } else {
        None
    }
}

/// Every point on the bounding box's border, and every corner reached.
fn is_rectangle(points: &[Point], x0: f64, y0: f64, x1: f64, y1: f64, tol: f64) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() <= tol;
    let on_border = |p: &Point| {
        ((near(p.x, x0) || near(p.x, x1)) && p.y >= y0 - tol && p.y <= y1 + tol)
            || ((near(p.y, y0) || near(p.y, y1)) && p.x >= x0 - tol && p.x <= x1 + tol)
    };
    points.iter().all(on_border)
        && [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .iter()
            .all(|&(cx, cy)| points.iter().any(|p| near(p.x, cx) && near(p.y, cy)))
}

/// Every point on the ellipse inscribed in the bounding box, going all the
/// way round it (no angular gap of π/4 or more).
fn is_ellipse(points: &[Point], x0: f64, y0: f64, x1: f64, y1: f64, tol: f64) -> bool {
    if points.len() < MIN_ELLIPSE_POINTS {
        return false;
    }
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (a, b) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let rel_tol = tol / min(a, b);
    let mut angles = Vec::with_capacity(points.len());
    for p in points {
        let (u, v) = ((p.x - cx) / a, (p.y - cy) / b);
        if (u.hypot(v) - 1.0).abs() > rel_tol {
            return false;
        }
        angles.push(v.atan2(u));
    }
    angles.sort_by(|p, q| p.total_cmp(q));
    let mut gap = angles[0] + 2.0 * std::f64::consts::PI - angles[angles.len() - 1];
    for w in angles.windows(2) {
        gap = max(gap, w[1] - w[0]);
    }
    gap < std::f64::consts::FRAC_PI_4
}
