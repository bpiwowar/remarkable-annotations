//! # remarkable-annotations
//!
//! Read and write the annotations of reMarkable documents: pen strokes and
//! text highlights on PDF pages, stored as `.rm` version 6 scene files.
//!
//! - [`scene`]: parse and serialize `.rm` v6 files, byte for byte.
//! - [`page`]: the strokes and highlights of a page; rewriting pages.
//! - [`shapes`]: rectangles and ellipses drawn with the shape tool.
//! - [`geometry`]: page coordinates ↔ PDF points; PDF page sizes ([`pdf`]).
//!
//! The library does no I/O. It is published for Rust (crates.io), JavaScript
//! (npm, WebAssembly) and Python (PyPI).

pub mod cloud;
pub mod geometry;
mod js;
pub mod page;
pub mod pdf;
pub mod scene;
pub mod shapes;

#[cfg(feature = "python")]
mod py;
