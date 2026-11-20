// The TypeScript implementation's results (zotero-remarkable-sync's
// packages/remarkable-annotations), the reference for tests/page.rs:
//
// - tests/fixtures/rm/**/X.page.json: parseRmPage of each fixture;
// - tests/fixtures/page/NAME.rm and NAME.ids.json: rebuildPage (or
//   blankStructure) for each scenario of tests/fixtures/page/scenarios.json.
//
//   (in zotero-remarkable-sync/packages/remarkable-annotations) npm run bundle
//   node page-dump.mjs .../packages/remarkable-annotations/.scaffold/index.mjs
//
// Ids are written as {author, counter} (the TypeScript part1/part2).
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ts = await import(process.argv[2]);
const here = dirname(fileURLToPath(import.meta.url));
const rmDir = join(here, "../fixtures/rm");
const pageDir = join(here, "../fixtures/page");

// TypeScript ids/maps -> the Rust JSON shapes
const out = (v) =>
  JSON.parse(
    JSON.stringify(v, (_k, x) => {
      if (x instanceof Map) return [...x.entries()];
      if (x && typeof x === "object" && "part1" in x && "part2" in x)
        return { author: x.part1, counter: x.part2 };
      return x;
    }),
  );
const toTs = (id) => id && { part1: id.author, part2: id.counter };

for (const name of readdirSync(rmDir, { recursive: true }).filter((f) => f.endsWith(".rm")).sort()) {
  const page = ts.parseRmPage(new Uint8Array(readFileSync(join(rmDir, name))));
  writeFileSync(join(rmDir, name.replace(/\.rm$/, ".page.json")), JSON.stringify(out(page)) + "\n");
}

const scenarios = JSON.parse(readFileSync(join(pageDir, "scenarios.json"), "utf8"));
for (const s of scenarios) {
  let bytes, meta;
  if (s.base.blank !== undefined) {
    const b = ts.blankStructure(s.base.blank);
    bytes = b.structure;
    meta = { layerId: b.layerId, author: b.author, startCounter: b.startCounter };
  } else {
    bytes = new Uint8Array(readFileSync(join(rmDir, s.base)));
    const p = ts.parseRmPage(bytes);
    meta = { layerId: p.layerId, lastItemId: p.lastItemId, author: p.maxAuthor, startCounter: p.maxCounter + 1 };
  }
  const u = s.update ?? {};
  if (u.layerId) meta.layerId = toTs(u.layerId);
  if (u.author !== undefined) meta.author = u.author;
  if (u.startCounter !== undefined) meta.startCounter = u.startCounter;
  if (u.authors) meta.authors = u.authors;
  if (u.register) meta.register = new Map(u.register);
  const remove = new Set();
  if (s.removeFirst) {
    const p = ts.parseRmPage(bytes);
    const first = [...p.highlights, ...p.strokes][0];
    remove.add(`${first.ids[0].part1},${first.ids[0].part2}`);
  }
  const r = ts.rebuildPage(bytes, s.strokes ?? [], s.highlights ?? [], remove, meta);
  writeFileSync(join(pageDir, `${s.name}.rm`), r.bytes);
  writeFileSync(join(pageDir, `${s.name}.ids.json`), JSON.stringify(out({ ids: r.ids, removed: [...remove] })) + "\n");
  console.log(s.name, r.bytes.length, r.ids.length);
}
// The blank structure itself
writeFileSync(join(pageDir, "blank_3.rm"), ts.blankStructure(3).structure);
for (const f of ["rmscene/Normal_A_stroke_2_layers.rm", "rmscene/Bold_Heading_Bullet_Normal.rm"]) {
  const split = ts.splitStructure(new Uint8Array(readFileSync(join(rmDir, f))));
  writeFileSync(join(pageDir, f.replace(/^.*\//, "split_")), split);
}
