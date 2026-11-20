// The TypeScript implementation's geometry, shape detection and PDF page
// sizes (zotero-remarkable-sync's packages/remarkable-annotations), the
// reference for tests/geometry.rs. Writes tests/fixtures/geometry.json with
// the inputs and the expected outputs.
//
//   node geometry-dump.mjs .../packages/remarkable-annotations/.scaffold/index.mjs
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ts = await import(process.argv[2]);
const fixtures = join(dirname(fileURLToPath(import.meta.url)), "../fixtures");

// PDF page sizes
const pdf = {};
for (const name of readdirSync(join(fixtures, "pdf")).sort()) {
  pdf[name] = ts.readPdfPageSizes(new Uint8Array(readFileSync(join(fixtures, "pdf", name))));
}

// Strokes and highlights of every page fixture
const strokes = [];
const rects = [];
for (const name of readdirSync(join(fixtures, "rm"), { recursive: true }).filter((f) => f.endsWith(".rm")).sort()) {
  const page = ts.parseRmPage(new Uint8Array(readFileSync(join(fixtures, "rm", name))));
  for (const s of page.strokes) strokes.push(s.points);
  for (const h of page.highlights) rects.push(...h.rects);
}

// Synthetic shapes (as in the TypeScript tests), with a fixed-seed generator
let seed = 42;
const rand = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
const pts = (...xy) => Array.from({ length: xy.length / 2 }, (_, i) => ({ x: xy[2 * i], y: xy[2 * i + 1] }));
const ellipse = (n, jitter = 0, a = 270, b = 78) =>
  Array.from({ length: n + 1 }, (_, i) => {
    const t = Math.PI + (2 * Math.PI * i) / n;
    const j = i % 2 ? jitter : -jitter;
    return { x: 100 + (a + j) * Math.cos(t), y: 300 + (b + j) * Math.sin(t) };
  });
const shapes = [
  pts(-100, 780, 500, 780, 500, 900, -100, 900, -100, 780),
  pts(0, 0, 50, 0, 100, 0, 100, 40, 100, 80, 0, 80, 0, 0),
  pts(0, 0, 50, 50, 100, 100, 150, 150, 200, 200),
  pts(0, 0, 100, 0, 50, 80, 0, 0, 0, 0),
  pts(50, 0, 100, 50, 50, 100, 0, 50, 50, 0),
  pts(0, 0, 3, 0, 3, 3, 0, 3, 0, 0),
  ellipse(83), ellipse(83, 25), ellipse(83).slice(0, 60), ellipse(8), ellipse(16), ellipse(200, 0.5, 30, 30),
  ...Array.from({ length: 50 }, () => ellipse(16 + Math.floor(rand() * 100), rand() * 3, 5 + rand() * 300, 5 + rand() * 300)),
  ...Array.from({ length: 50 }, () => {
    const [x, y, w, h] = [rand() * 1000 - 500, rand() * 1800, 1 + rand() * 600, 1 + rand() * 600];
    const e = () => (rand() - 0.5) * 4;
    return pts(x, y, x + w + e(), y + e(), x + w + e(), y + h + e(), x + e(), y + h + e(), x + e(), y + e());
  }),
  ...Array.from({ length: 50 }, () => Array.from({ length: 3 + Math.floor(rand() * 40) }, () => ({ x: rand() * 400, y: rand() * 400 }))),
];

const sizes = [
  { width: 612, height: 792 },
  { width: 595.28, height: 841.89, x0: 10, y0: 20 },
  ...Object.values(pdf).flat().slice(0, 6),
];
const conversions = sizes.map((size) => ({
  size,
  paths: strokes.slice(0, 40).map((p) => ts.strokeToPath(p, size)),
  back: strokes.slice(0, 40).map((p) => ts.pdfPathToRm(ts.strokeToPath(p, size), size)),
  rects: rects.map((r) => ts.rmRectToPdf(r, size)),
  rectsBack: rects.map((r) => ts.pdfRectToRm(ts.rmRectToPdf(r, size), size)),
}));
const widths = Array.from({ length: 300 }, (_, i) => i * 0.37);

const out = {
  pdf,
  shapes: [...shapes, ...strokes].map((points) => ({ points, shape: ts.detectShape(points) })),
  conversions,
  inkWidth: widths.map((w) => [w, ts.inkWidth(w)]),
  pdfWidthToRm: widths.map((w) => [w / 10, ts.pdfWidthToRm(w / 10)]),
};
writeFileSync(join(fixtures, "geometry.json"), JSON.stringify(out) + "\n");
console.log(`pdf ${Object.keys(pdf).length}, shapes ${out.shapes.length} (${out.shapes.filter((s) => s.shape).length} found), sizes ${sizes.length}`);
