// Pages written by the TypeScript writer (the remarkable-annotations package
// of zotero-remarkable-sync), as fixtures: tests/fixtures/rm/generated.
//
//   (in zotero-remarkable-sync/packages/remarkable-annotations) npm run bundle
//   node gen-fixtures.mjs .../.scaffold/index.mjs ../fixtures/rm/rmscene ../fixtures/rm/generated
//   npm run dump
//
// The uuids are made up.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const [bundle, rmsceneDir, out] = process.argv.slice(2);
const { rebuildPage, blankStructure, splitStructure, parseRmPage } = await import(bundle);

const stroke = (x, colorIndex = 0, extra = {}) => ({
  tool: 17, colorIndex, thickness: 2, pointWidth: 16, isHighlighter: false,
  points: [{ x, y: x }, { x: x + 10.5, y: x - 3.25 }, { x: x + 20, y: x + 7 }],
  ...extra,
});
const highlight = (colorIndex, text = "some text") => ({
  text, colorIndex, rects: [{ x: -100, y: 50, w: 200, h: 30 }, { x: -90, y: 85, w: 120.5, h: 30 }],
});
const write = (name, bytes) => { writeFileSync(join(out, name), bytes); console.log(name, bytes.length); };

// A blank page with strokes and highlights (colours 3 and 14, non-ASCII text).
const blank = blankStructure();
write("blank_strokes_highlights.rm", rebuildPage(blank.structure,
  [stroke(0), stroke(100, 6), stroke(-50, 14, { tool: 18, isHighlighter: true })],
  [highlight(3), highlight(14, "naïve café — ✓")], new Set(), blank).bytes);

// A blank page with extra authors registered (made-up uuids), one per item.
const b2 = blankStructure(2);
write("blank_registered_authors.rm", rebuildPage(b2.structure, [stroke(1), stroke(2)], [highlight(4)],
  new Set(), { ...b2, authors: [5, 6, 7], register: new Map([
    [5, "00000000-0000-4000-8000-000000000005"],
    [6, "00000000-0000-4000-8000-000000000006"],
    [7, "00000000-0000-4000-8000-000000000007"]]) }).bytes);

// Real pages: append items, then remove one of the existing items.
for (const f of ["Normal_A_stroke_2_layers.rm", "Wikipedia_highlighted_p1.rm"]) {
  const bytes = new Uint8Array(readFileSync(join(rmsceneDir, f)));
  const p = parseRmPage(bytes);
  const meta = { layerId: p.layerId, lastItemId: p.lastItemId, author: p.maxAuthor, startCounter: p.maxCounter + 1 };
  const added = rebuildPage(bytes, [stroke(10, 7)], [highlight(5)], new Set(), meta).bytes;
  write(f.replace(".rm", "_appended.rm"), added);
  const first = [...p.strokes, ...p.highlights][0];
  if (first?.ids?.length) {
    const id = `${first.ids[0].part1},${first.ids[0].part2}`;
    write(f.replace(".rm", "_removed.rm"), rebuildPage(bytes, [], [], new Set([id]), meta).bytes);
  }
  write(f.replace(".rm", "_split.rm"), splitStructure(bytes));
}
