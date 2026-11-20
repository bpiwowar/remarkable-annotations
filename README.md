# remarkable-annotations

Read and write the annotations of reMarkable documents: pen strokes and text
highlights on PDF pages, stored as `.rm` version 6 scene files.

One Rust library, published for three languages:

| Language   | Package                                   | Built from                       |
| ---------- | ----------------------------------------- | -------------------------------- |
| Rust       | `remarkable-annotations` (crates.io)      | this crate                       |
| JavaScript | `remarkable-annotations` (npm, WebAssembly) | [`wasm/`](wasm) (wasm-bindgen) |
| Python     | `remarkable-annotations` (PyPI)           | [`src/py`](src/py) (PyO3, maturin) |

The library does no I/O: it works on bytes, so it runs the same in a browser,
in privileged Firefox code (a Zotero plugin), in Node and in Python.

**Status:** early. Done: `.rm` v6 scenes, page annotations (reading,
rewriting pages), shape detection, coordinates and PDF page sizes. Next: the
reMarkable cloud sync (sans-I/O). All of it is ported from the TypeScript
implementation in
[zotero-remarkable-sync](https://github.com/bpiwowar/zotero-remarkable-sync),
and tested against it.

## Scenes

```rust
use remarkable_annotations::scene::Scene;

let bytes = std::fs::read("page.rm")?;
let scene = Scene::parse(&bytes)?;
assert_eq!(scene.serialize(), bytes); // byte for byte
```

Writing back an unmodified scene gives the input bytes: blocks that fail to
parse are kept raw, each block keeps its unread tail, and wire details with no
meaning (string ASCII flags, absent optional fields) are kept as read.

## Tests

```bash
cargo test                          # Rust
uv venv && uv pip install maturin pytest && maturin develop --uv && pytest  # Python
```

The fixtures in [`tests/fixtures/rm`](tests/fixtures/rm) are real tablet pages
(from [rmscene](https://github.com/ricklupton/rmscene)) and pages written by
the TypeScript implementation. For each one, the tests check that:

- it round-trips byte for byte;
- the parsed blocks are those of [rmapi-js](https://github.com/erikbrinkman/rmapi-js)
  (`X.rmapi.json`, written by [`tests/reference/dump.mjs`](tests/reference/dump.mjs));
- corrupted copies (bytes changed, inserted, removed, truncated) still parse
  and round-trip, without panics.

The page, geometry, shape and PDF modules are compared with the TypeScript
implementation: [`page-dump.mjs`](tests/reference/page-dump.mjs) and
[`geometry-dump.mjs`](tests/reference/geometry-dump.mjs) record its results
(annotations of every fixture, rebuilt pages byte for byte, shapes, coordinate
conversions, page sizes of the PDFs in [`tests/fixtures/pdf`](tests/fixtures/pdf),
made by [`gen-pdfs.py`](tests/reference/gen-pdfs.py)).

## License

Licensed under either of the [MIT license](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option. See
[NOTICE](NOTICE) for the projects this one derives from.
