//! # remarkable-annotations
//!
//! Read and write the annotations of reMarkable documents: pen strokes and
//! text highlights on PDF pages, stored as `.rm` version 6 scene files.
//!
//! - [`scene`]: parse and serialize `.rm` v6 files, byte for byte.
//!
//! The library does no I/O. It is published for Rust (crates.io), JavaScript
//! (npm, WebAssembly) and Python (PyPI).

pub mod scene;

#[cfg(feature = "python")]
mod py;
