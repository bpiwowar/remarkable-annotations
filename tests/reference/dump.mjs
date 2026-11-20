// Writes, for each tests/fixtures/rm/**/X.rm, X.rmapi.json: the blocks as parsed
// by rmapi-js (maps as [key, value] pairs, byte arrays as hex), and whether
// rmapi-js writes the file back byte for byte. tests/scene.rs compares the
// Rust parser with these files.
//
//   cd tests/reference && npm ci && npm run dump
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
// rmapi-js does not export its scene module: load it from its dist folder
// (next to its main entry).
const rm6 = await import(
  new URL("rm6.js", import.meta.resolve("rmapi-js")).href
);

const dir = join(dirname(fileURLToPath(import.meta.url)), "../fixtures/rm");
const replacer = (_key, value) => {
  if (value instanceof Map) return [...value.entries()];
  if (value instanceof Uint8Array) return Buffer.from(value).toString("hex");
  return value;
};

const names = readdirSync(dir, { recursive: true })
  .filter((f) => f.endsWith(".rm"))
  .sort();
for (const name of names) {
  const bytes = new Uint8Array(readFileSync(join(dir, name)));
  const scene = rm6.parseRmScene(bytes);
  const out = rm6.serializeRmScene(scene);
  const roundTrips =
    out.length === bytes.length && out.every((b, i) => b === bytes[i]);
  const json = JSON.stringify({ roundTrips, blocks: scene.blocks }, replacer);
  writeFileSync(join(dir, name.replace(/\.rm$/, ".rmapi.json")), json + "\n");
  console.log(`${name}: ${scene.blocks.length} blocks, round trip ${roundTrips}`);
}
