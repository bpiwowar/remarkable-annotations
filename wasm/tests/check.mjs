// The WebAssembly build against the fixtures: byte-exact round trips and the
// same blocks as rmapi-js (X.rmapi.json).
//
//   node wasm/tests/check.mjs PKG_DIR   (a wasm-bindgen --target nodejs build)
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const require = createRequire(import.meta.url);
const { Scene } = require(resolve(process.argv[2], "remarkable_annotations_wasm.js"));
const dir = new URL("../../tests/fixtures/rm/", import.meta.url).pathname;

let n = 0;
for (const name of readdirSync(dir, { recursive: true }).filter((f) => f.endsWith(".rm"))) {
  const bytes = new Uint8Array(readFileSync(join(dir, name)));
  const scene = new Scene(bytes);
  assert.deepEqual(scene.serialize(), bytes, `${name}: round trip`);
  const reference = JSON.parse(readFileSync(join(dir, name.replace(/\.rm$/, ".rmapi.json")), "utf8"));
  assert.deepEqual(JSON.parse(scene.blocksJson()), reference.blocks, `${name}: blocks`);
  scene.free();
  n++;
}
assert.throws(() => new Scene(new Uint8Array([1, 2, 3])));
console.log(`wasm: ${n} fixtures OK`);
