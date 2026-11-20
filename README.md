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
rewriting pages), shape detection, coordinates and PDF page sizes, and a
reMarkable cloud client (sans-I/O). All of it is ported from the TypeScript
implementation in
[zotero-remarkable-sync](https://github.com/bpiwowar/zotero-remarkable-sync),
and tested against it.

## Used by

- [zotero-remarkable-sync](https://github.com/bpiwowar/zotero-remarkable-sync), a
  Zotero plugin that syncs annotations between Zotero and a reMarkable tablet. It
  uses the JavaScript (WebAssembly) package for the `.rm` scenes, the pages and
  the reMarkable cloud client.

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

## Reading and writing pages

[`page::parse_page`](src/page.rs) (`parsePage` in JavaScript) maps a page's
blocks to strokes (line items) and highlights (glyph ranges). Each comes with
its scene item ids, so a caller can remove it later.

[`page::rebuild_page`](src/page.rs) (`rebuildPage`) drops the items whose id is
in `remove`, keeps every other block as is, then appends the new items
(highlights first) after the layer's last item. It returns the ids it gave
them.

A page without a `.rm` file gets one from a template:
[`split_structure`](src/page.rs) copies another page's structure without its
items, and [`blank_structure`](src/page.rs) builds one from scratch.

### Rules the tablet enforces

These were checked on a real tablet:

- **Counters:** new items must use counters above every counter on the page,
  whatever their author (`max_counter` covers tombstones and groups too). The
  tablet drops the others when it loads the page.
- **Authors:** item ids are `(author, counter)`, and the author is a byte, so a
  page holds at most 255 authors. Authors added to the page's author table
  (`PageUpdate::register`) survive tablet saves. An erased item leaves a
  tombstone that keeps its author id.
- **Per-item authors:** `PageUpdate::authors` sets the author of each added
  item. The Zotero plugin uses this to tag each item with the annotation it
  mirrors (an author uuid encoding the Zotero key).

## Pages and PDF pages

A document's `.content` gives the page order (`pages`, or `cPages.pages`
without deleted pages) and the `redirectionPageMap`, which maps reMarkable
pages to PDF pages; each page's annotations are in `{pageUuid}.rm`. Page tags
(the tablet's tag-a-page feature) name the page they belong to.

## Coordinates

The tablet renders a PDF at a fixed resolution (`RM_DPI` = 226, calibrated on
an rM2) and stores coordinates in those device pixels, with x measured from the
page centre and y from the top. [`geometry`](src/geometry.rs) converts them to
PDF points (origin bottom-left) with a single scale:

```
pdfX = rmX * 72/RM_DPI + x0 + pageWidth/2
pdfY = y0 + pageHeight - rmY * 72/RM_DPI
```

The mapping was checked against a real export: highlight rectangles matched the
PDF's vectors within 1 pt. Pen widths use a factor fitted the same way
(`ink_width`, `pdf_width_to_rm`).

[`read_pdf_page_sizes`](src/geometry.rs) gives each page's visible size. It
walks the PDF's page tree ([`pdf`](src/pdf.rs)), expanding compressed object
streams and honouring inherited boxes. If that fails, it falls back to a
plain-text scan of `/MediaBox`, then to US Letter.

## Cloud

[`cloud`](src/cloud) is a reMarkable cloud client that does no I/O: each
operation hands the host the HTTP requests to make, and continues with the
responses. Where rmapi-js runs requests in parallel (`Promise.all`: listing
items, a document's metadata and content, page uploads), the operation hands
them over together as a batch, which the host runs concurrently; listing 50
documents takes 4 round trips. The host also provides the clock and uuids.
The same client runs in a browser, in privileged Firefox code, in Node and in
Python.

```js
import init, { CloudSession, cloudAuth } from "remarkable-annotations";

async function perform(item) {
  // a request, or a sleep before a retry
  if (item.type === "sleep") {
    await new Promise((ok) => setTimeout(ok, Math.random() * item.ms));
    return undefined;
  }
  const r = await fetch(item.url, { method: item.method, headers: item.headers, body: item.body });
  return { status: r.status, statusText: r.statusText, body: new Uint8Array(await r.arrayBuffer()) };
}

async function run(op) {
  for (let step = op.start(); ; ) {
    if (step.type === "batch") step = op.resumeBatch(await Promise.all(step.items.map(perform)));
    else if (step.type === "request" || step.type === "sleep") step = op.resume(await perform(step));
    else if (step.type === "done") return step; // json, bytes or text
    else throw step.error; // {kind: "generation" | "hashNotFound" | "response" | ..., message}
  }
}

const { text: token } = await run(cloudAuth(deviceToken));
const session = new CloudSession(token);
const items = JSON.parse((await run(session.listItems(false))).json);
```

Operations: `listItems`, `listRefs`, `getContent`, `getMetadata`,
`getEntries`, `getHash`, `putPdf`, `putFolder`, `updateDocument`, `move`,
`purge`, `putRmPages` (one commit; `hashNotFound` if the document changed),
and the document helpers `uploadPdf` (with the full page list),
`ensureFolder`, `fetchAnnotations` and `mapPdfPages`; `cloudRegister` and
`cloudAuth` for tokens. Generation conflicts and transient errors are retried
as rmapi-js does.

The protocol is [rmapi-js](https://github.com/erikbrinkman/rmapi-js)'s,
ported operation by operation. [`cloud-check.mjs`](tests/reference/cloud-check.mjs)
runs rmapi-js and this client side by side on fake clouds
([`fakecloud.mjs`](tests/reference/fakecloud.mjs)): accounts on schema 3 and 4,
reads, writes, errors, transient failures, and other devices writing at the
same time. Both must return the same results (as JSON text) and leave the same
cloud, byte for byte.

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
