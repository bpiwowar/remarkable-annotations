// The WebAssembly build against the fixtures: byte-exact round trips, the
// same blocks as rmapi-js (X.rmapi.json), and the same results as the
// TypeScript implementation through the JavaScript API (X.page.json, the
// page scenarios, geometry.json).
//
//   node wasm/tests/check.mjs PKG_DIR   (a wasm-pack --target nodejs build)
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const require = createRequire(import.meta.url);
const wasm = require(resolve(process.argv[2], "remarkable_annotations_wasm.js"));
const fixtures = new URL("../../tests/fixtures/", import.meta.url).pathname;
const dir = join(fixtures, "rm");
const json = (v) => JSON.parse(JSON.stringify(v));
const read = (p) => new Uint8Array(readFileSync(p));

let n = 0;
for (const name of readdirSync(dir, { recursive: true }).filter((f) => f.endsWith(".rm"))) {
  const bytes = read(join(dir, name));
  const scene = new wasm.Scene(bytes);
  assert.deepEqual(scene.serialize(), bytes, `${name}: round trip`);
  const reference = JSON.parse(readFileSync(join(dir, name.replace(/\.rm$/, ".rmapi.json")), "utf8"));
  assert.deepEqual(JSON.parse(scene.blocksJson()), reference.blocks, `${name}: blocks`);
  scene.free();
  const page = JSON.parse(readFileSync(join(dir, name.replace(/\.rm$/, ".page.json")), "utf8"));
  assert.deepEqual(json(wasm.parsePage(bytes)), page, `${name}: parsePage`);
  n++;
}
assert.throws(() => new wasm.Scene(new Uint8Array([1, 2, 3])));
assert.throws(() => wasm.parsePage(new Uint8Array([1, 2, 3])));

// Page scenarios (as in tests/page.rs)
const pageDir = join(fixtures, "page");
for (const s of JSON.parse(readFileSync(join(pageDir, "scenarios.json"), "utf8"))) {
  let bytes, update;
  if (s.base.blank !== undefined) {
    const b = wasm.blankStructure(s.base.blank);
    bytes = b.bytes;
    update = b.update;
  } else {
    bytes = read(join(dir, s.base));
    const p = wasm.parsePage(bytes);
    update = { layerId: p.layerId ?? { author: 0, counter: 0 }, lastItemId: p.lastItemId, author: p.maxAuthor, startCounter: p.maxCounter + 1 };
  }
  Object.assign(update, s.update ?? {});
  const remove = [];
  if (s.removeFirst) {
    const p = wasm.parsePage(bytes);
    remove.push([...p.highlights, ...p.strokes][0].ids[0]);
  }
  const r = wasm.rebuildPage(bytes, s.strokes ?? [], s.highlights ?? [], remove, update);
  assert.ok(r.bytes instanceof Uint8Array);
  assert.deepEqual(r.bytes, read(join(pageDir, `${s.name}.rm`)), `${s.name}: bytes`);
  assert.deepEqual(r.ids, JSON.parse(readFileSync(join(pageDir, `${s.name}.ids.json`), "utf8")).ids, `${s.name}: ids`);
}

// Geometry, shapes and page sizes
const g = JSON.parse(readFileSync(join(fixtures, "geometry.json"), "utf8"));
for (const [name, sizes] of Object.entries(g.pdf)) {
  assert.deepEqual(json(wasm.readPdfPageSizes(read(join(fixtures, "pdf", name)))), sizes, name);
}
for (const { points, shape } of g.shapes) assert.deepEqual(json(wasm.detectShape(points) ?? null), shape);
for (const c of g.conversions) {
  c.paths.forEach((path, i) => {
    assert.deepEqual(wasm.pdfPathToRm(path, c.size), c.back[i]);
    assert.deepEqual([...wasm.strokeToPath(c.back[i], c.size)], path);
  });
  c.rects.forEach((r, i) => assert.deepEqual(json(wasm.pdfRectToRm(r, c.size)), c.rectsBack[i]));
}
for (const [w, v] of g.inkWidth) assert.equal(wasm.inkWidth(w), v);
for (const [w, v] of g.pdfWidthToRm) assert.equal(wasm.pdfWidthToRm(w), v);
console.log(`wasm: ${n} fixtures, page scenarios, geometry OK`);
