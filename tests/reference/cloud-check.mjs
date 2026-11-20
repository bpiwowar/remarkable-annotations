// The Rust cloud client against rmapi-js, on fake clouds (fakecloud.mjs).
//
// Each scenario seeds two identical clouds, runs the same operations with
// rmapi-js (plus the plugin's helpers, plugin-helpers.mjs) on one and with
// the Rust client (WebAssembly build, driven like a host would) on the other,
// then checks that both return the same results (as JSON text, key order
// included), write the same root updates, and leave the same cloud: root,
// generation, and every stored file.
//
//   cd tests/reference && npm ci
//   wasm-pack build --target nodejs --out-dir /tmp/pkg ../../wasm
//   node cloud-check.mjs /tmp/pkg
//
// rmapi-js's randomness and clock are pinned: uuids are derived from the
// operation's index, Date is fixed per operation, backoffs are 0 ms.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { AUTH_HOST, FakeCloud, RAW_HOST, UPLOAD_HOST, sessionToken } from "./fakecloud.mjs";
import * as helpers from "./plugin-helpers.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const wasm = require(resolve(process.argv[2], "remarkable_annotations_wasm.js"));

// --- pinned randomness and clock (before rmapi-js is loaded) -------------
let uuidPrefix = 0;
let uuidCount = 0;
const uuidFor = (op, n) =>
  `${op.toString(16).padStart(8, "0")}-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
globalThis.crypto.randomUUID = () => uuidFor(uuidPrefix, uuidCount++);
const RealDate = Date;
let NOW = 0;
globalThis.Date = class extends RealDate {
  constructor(...a) {
    if (a.length) super(...a);
    else super(NOW);
  }
  static now() {
    return NOW;
  }
};
Math.random = () => 0;

const rmapi = await import("rmapi-js");

const enc = new TextEncoder();
const fixture = (p) => new Uint8Array(readFileSync(join(here, "../fixtures", p)));
const RM_A1 = fixture("rm/rmscene/Normal_A_stroke_2_layers.rm");
const RM_A2 = fixture("rm/rmscene/Wikipedia_highlighted_p1.rm");
const RM_B = fixture("rm/rmscene/Color_and_tool_v3.14.4.rm");
const RM_NEW = fixture("page/lines_append.rm");
const PDF = fixture("pdf/mixed_generate.pdf");

const ID = {
  zotero: "10000000-0000-4000-8000-000000000001",
  papers: "10000000-0000-4000-8000-000000000002",
  a: "20000000-0000-4000-8000-00000000000a",
  b: "20000000-0000-4000-8000-00000000000b",
  c: "20000000-0000-4000-8000-00000000000c",
  t: "20000000-0000-4000-8000-00000000000d",
  other: "30000000-0000-4000-8000-000000000001",
};
const json = (o) => JSON.stringify(o);

function folder(id, name, parent, contentText = json({ tags: [] })) {
  return [
    { name: `${id}.content`, bytes: contentText },
    {
      name: `${id}.metadata`,
      bytes: json({ visibleName: name, type: "CollectionType", parent, pinned: false, lastModified: "1690000000000", createdTime: "1690000000000" }),
    },
  ];
}

/** two clouds with folders, documents (old and new content formats), a template and a trashed item */
function seed(schema) {
  const c = new FakeCloud(schema);
  c.addItem(ID.zotero, folder(ID.zotero, "Zotero", ""));
  c.addItem(ID.papers, folder(ID.papers, "Papers", ID.zotero, json({ tags: [{ name: "t", timestamp: 1 }], })));
  // A: legacy page list, odd key order, unknown keys, numeric-like keys,
  // floats written as 1.0, page tags with blanks and duplicates
  const aContent = `{"zzUnknown":{"b":1,"a":2},"pages":["pa1","pa2","pa3"],"fileType":"pdf",
    "coverPageNumber":0,"documentMetadata":{"title":"T","x":[1]},"extraMetadata":{"2":"b","1":"a","LastTool":"x"},
    "fontName":"","lineHeight":-1,"orientation":"portrait","pageCount":3,"textAlignment":"justify",
    "textScale":1.0,"redirectionPageMap":[0,2,-1],"tags":[{"name":"x","timestamp":1.5e3}],
    "pageTags":[{"name":" red ","pageId":"pa2","timestamp":1},{"name":"red","pageId":"pa2","timestamp":2},
    {"name":"blue","pageId":"pa1","timestamp":3},{"name":"  ","pageId":"pa1","timestamp":4},{"name":"gone","pageId":"zz","timestamp":5}],
    "margins":125,"formatVersion":1,"sizeInBytes":"10"}\n`;
  c.addItem(ID.a, [
    { name: `${ID.a}.content`, bytes: aContent },
    { name: `${ID.a}.metadata`, bytes: json({ visibleName: "Paper A", type: "DocumentType", parent: ID.zotero, pinned: false, lastModified: "1690000000001", version: 3, extra: true }) },
    { name: `${ID.a}.pdf`, bytes: PDF },
    { name: `${ID.a}.pagedata`, bytes: "Blank\nBlank\nBlank\n" },
    { name: `${ID.a}/pa1.rm`, bytes: RM_A1 },
    { name: `${ID.a}/pa2.rm`, bytes: RM_A2 },
    { name: `${ID.a}/pa2-metadata.json`, bytes: json({ layers: [{ name: "Layer 1" }] }) },
  ]);
  // B: cPages, with deleted pages
  const page = (id, idx, extra = {}) => ({ id, idx: { timestamp: "1:2", value: idx }, ...extra });
  const bContent = json({
    coverPageNumber: -1, documentMetadata: {}, extraMetadata: {}, fileType: "pdf", fontName: "",
    lineHeight: -1, orientation: "landscape", pageCount: 3, textAlignment: "left", textScale: 1.25,
    cPages: {
      lastOpened: { timestamp: "1:1", value: "pb1" }, original: { timestamp: "0:0", value: 3 },
      pages: [page("pb1", "ba"), page("pb2", "bb", { deleted: { timestamp: "1:3", value: 1 } }),
        page("pb3", "bc", { deleted: { timestamp: "1:4", value: 0 }, redir: { timestamp: "1:5", value: 2 } })],
      uuids: [{ first: "dev", second: 1 }],
    },
    tags: null,
  });
  c.addItem(ID.b, [
    { name: `${ID.b}.content`, bytes: bContent },
    { name: `${ID.b}.metadata`, bytes: json({ visibleName: "Paper B", type: "DocumentType", parent: ID.papers, pinned: true, lastOpened: "5" }) },
    { name: `${ID.b}.pdf`, bytes: PDF.slice(0, 100) },
    { name: `${ID.b}/pb1.rm`, bytes: RM_B },
    { name: `${ID.b}/pb3.rm`, bytes: enc.encode("not a page") },
  ]);
  // C: trashed, legacy string tags
  c.addItem(ID.c, [
    { name: `${ID.c}.content`, bytes: json({ coverPageNumber: 0, documentMetadata: {}, extraMetadata: {}, fileType: "epub", fontName: "", lineHeight: -1, orientation: "portrait", pageCount: 0, textAlignment: "", textScale: 1, tags: ["old"] }) },
    { name: `${ID.c}.metadata`, bytes: json({ visibleName: "Trashed", type: "DocumentType", parent: "trash", pinned: false }) },
  ]);
  // T: a template
  c.addItem(ID.t, [
    { name: `${ID.t}.content`, bytes: json({ name: "Grid", author: "me", iconData: "", categories: ["c"], orientation: "portrait", templateVersion: "1.0.0", items: [], formatVersion: 1 }) },
    { name: `${ID.t}.metadata`, bytes: json({ visibleName: "Grid", type: "TemplateType", parent: "", pinned: false, new: true, source: "com.remarkable.methods" }) },
  ]);
  return c;
}

// --- operations, as each implementation runs them --------------------------

const item = (cloud, name) => cloud.find(name);
const STALE = "0".repeat(64);

/** rmapi-js (and the plugin helpers) */
async function runRmapi(api, cloud, op) {
  const ref = op.item && (op.stale ? { id: item(cloud, op.item).id, hash: STALE } : item(cloud, op.item));
  switch (op.op) {
    case "listItems": return api.listItems(op.refresh);
    case "listRefs": return api.listRefs(op.refresh);
    case "getContent": return api.getContent(ref);
    case "getMetadata": return api.getMetadata(ref);
    case "getEntries": return api.raw.getEntries(op.root ? { id: "root", hash: cloud.root.hash } : ref);
    case "getHash": {
      const { entries } = await api.raw.getEntries(ref);
      return api.raw.getHash(entries.find((e) => e.id.endsWith(op.suffix)));
    }
    case "putPdf": return api.putPdf(op.name, PDF, { parent: op.parent, tags: op.tags });
    case "putFolder": return api.putFolder(op.name, { parent: op.parent });
    case "updateDocument": return api.updateDocument(ref, op.update);
    case "move": return api.move(ref, op.parent);
    case "purge": return api.purge(ref);
    case "putRmPages":
      return api.putRmPages(ref, new Map(Object.entries(op.pages).map(([k, v]) => [k, rmapi_parse(v)])));
    case "uploadPdf":
      return helpers.uploadPdf(api, op.name, PDF, op.folder, { tags: op.tags, pageCount: op.pageCount, pageTags: op.pageTags });
    case "ensureFolder": return helpers.ensureFolder(api, op.path);
    case "fetchAnnotations":
      return helpers.fetchAnnotations(api, ref.id, ref.hash, (b) => JSON.parse(wasm.cloudParsePage(b)));
    case "mapPdfPages": return helpers.mapPdfPages(api, ref.id, ref.hash);
  }
  throw new Error(`unknown op ${op.op}`);
}

const PAGES = { RM_NEW, RM_A2 };
// rmapi-js's putRmPages takes parsed scenes (the plugin parses the bytes)
const rm6 = await import(new URL("rm6.js", import.meta.resolve("rmapi-js")).href);
const rmapi_parse = (name) => rm6.parseRmScene(PAGES[name]);

// Batch items complete in a shuffled order (seeded), as concurrent requests
// would; outcomes still go back in item order.
let jitterSeed = 7;
const jitter = () => ((jitterSeed = (jitterSeed * 1103515245 + 12345) % 2147483648) % 3);
const tick = (n) => new Promise((ok) => setTimeout(ok, n));

/** one request (or sleep) of the Rust client, on the fake cloud */
async function perform(cloud, item) {
  if (item.type === "sleep") return undefined;
  try {
    const r = await cloud.fetch(item.url, { method: item.method, headers: item.headers, body: item.body });
    return { status: r.status, statusText: r.statusText, body: new Uint8Array(await r.arrayBuffer()) };
  } catch (e) {
    return { networkError: String(e) };
  }
}

/** the Rust client, driven by a host loop; counts round trips */
export const trips = { count: 0, widest: 0 };
async function drive(cloud, o) {
  let step = o.start();
  for (;;) {
    if (step.type === "request" || step.type === "sleep") {
      trips.count++;
      step = o.resume(await perform(cloud, step));
    } else if (step.type === "batch") {
      trips.count++;
      trips.widest = Math.max(trips.widest, step.items.length);
      const outcomes = await Promise.all(
        step.items.map(async (item) => {
          await tick(jitter());
          return perform(cloud, item);
        }),
      );
      step = o.resumeBatch(outcomes);
    } else if (step.type === "done") {
      if (step.json !== undefined) return { json: step.json };
      if (step.bytes !== undefined) return step.bytes;
      if (step.text !== undefined) return step.text;
      return undefined;
    } else {
      throw Object.assign(new Error(step.error.message), { rust: step.error });
    }
  }
}

function runRust(s, cloud, op, opIndex) {
  const ref = op.item && (op.stale ? { id: item(cloud, op.item).id, hash: STALE } : item(cloud, op.item));
  const uuids = Array.from({ length: 10 }, (_, n) => uuidFor(opIndex, n));
  switch (op.op) {
    case "listItems": return drive(cloud, s.listItems(!!op.refresh));
    case "listRefs": return drive(cloud, s.listRefs(!!op.refresh));
    case "getContent": return drive(cloud, s.getContent(ref.id, ref.hash));
    case "getMetadata": return drive(cloud, s.getMetadata(ref.id, ref.hash));
    case "getEntries":
      return op.root ? drive(cloud, s.getEntries("root", cloud.root.hash)) : drive(cloud, s.getEntries(ref.id, ref.hash));
    case "getHash": return (async () => {
      const { json: j } = await drive(cloud, s.getEntries(ref.id, ref.hash));
      const e = JSON.parse(j).entries.find((x) => x.id.endsWith(op.suffix));
      return drive(cloud, s.getHash(e.id, e.hash));
    })();
    case "putPdf":
      return drive(cloud, s.putPdf(op.name, PDF, json({ parent: op.parent ?? "", tags: op.tags }), uuids[0], uuids[1], NOW));
    case "putFolder": return drive(cloud, s.putFolder(op.name, op.parent ?? "", uuids[0], NOW));
    case "updateDocument": return drive(cloud, s.updateDocument(ref.id, ref.hash, json(op.update)));
    case "move": return drive(cloud, s.move(ref.id, ref.hash, op.parent));
    case "purge": return drive(cloud, s.purge(ref.id, ref.hash));
    case "putRmPages": {
      const ids = Object.keys(op.pages);
      return drive(cloud, s.putRmPages(ref.id, ref.hash, ids, ids.map((k) => PAGES[op.pages[k]])));
    }
    case "uploadPdf":
      return drive(cloud, s.uploadPdf(op.name, PDF, op.folder, op.tags ?? [], op.pageCount, json(op.pageTags ?? {}), uuids, NOW));
    case "ensureFolder": return drive(cloud, s.ensureFolder(op.path, uuids, NOW));
    case "fetchAnnotations": return drive(cloud, s.fetchAnnotations(ref.id, ref.hash));
    case "mapPdfPages": return drive(cloud, s.mapPdfPages(ref.id, ref.hash));
  }
  throw new Error(`unknown op ${op.op}`);
}

// --- comparing ----------------------------------------------------------------

const hex = (b) => Buffer.from(b).toString("hex");
const replacer = (_k, v) => {
  if (v instanceof Map) return [...v.entries()];
  if (v instanceof Uint8Array) return hex(v);
  return v;
};
/** rmapi-js error classes → the Rust error kinds */
function errorKind(e) {
  if (e.rust) return { kind: e.rust.kind, status: e.rust.status };
  const name = e.constructor.name;
  const kind = {
    GenerationError: "generation", HashNotFoundError: "hashNotFound", ResponseError: "response",
    RegisterError: "register", AuthError: "auth", ValidationError: "validation", ZodError: "validation",
    TypeError: "network",
  }[name] ?? "other";
  return { kind, status: e.status };
}
const outcome = async (p) => {
  try {
    const v = await p;
    if (v instanceof Uint8Array) return { bytes: hex(v) };
    if (v && typeof v === "object" && "json" in v && Object.keys(v).length === 1) return { json: v.json };
    return { json: v === undefined ? undefined : JSON.stringify(v, replacer) };
  } catch (e) {
    return { error: errorKind(e), message: e.message };
  }
};
const rootPuts = (cloud) => cloud.log.filter((r) => r.method === "PUT" && r.url.endsWith("/sync/v3/root")).map((r) => r.body);

// --- scenarios ----------------------------------------------------------------

const READS = [
  { op: "listItems" }, { op: "listRefs" }, { op: "getContent", item: "Paper A" },
  { op: "getContent", item: "Paper B" }, { op: "getContent", item: "Zotero" }, { op: "getContent", item: "Grid" },
  { op: "getMetadata", item: "Paper A" }, { op: "getEntries", item: "Paper A" }, { op: "getEntries", root: true },
  { op: "getHash", item: "Paper A", suffix: ".pdf" }, { op: "fetchAnnotations", item: "Paper A" },
  { op: "fetchAnnotations", item: "Paper B" }, { op: "mapPdfPages", item: "Paper A" },
  { op: "mapPdfPages", item: "Paper B" }, { op: "fetchAnnotations", item: "Zotero" }, { op: "listItems", refresh: true },
];
const WRITES = [
  { op: "putFolder", name: "New folder", parent: ID.zotero },
  { op: "putPdf", name: "Upload ✓", parent: ID.papers, tags: ["a", "b"] },
  { op: "uploadPdf", name: "With pages", folder: ID.zotero, tags: ["z"], pageCount: 3, pageTags: { 2: ["y", "x"], 0: ["w"], 7: ["none"] } },
  { op: "uploadPdf", name: "No count", folder: "" },
  { op: "ensureFolder", path: " Zotero / Papers /Deep/ Deeper " },
  { op: "ensureFolder", path: "Zotero" },
  { op: "updateDocument", item: "Paper A", update: { pageTags: [{ name: "n", pageId: "pa1", timestamp: 9 }], newKey: { a: 1 }, textScale: 2 } },
  { op: "putRmPages", item: "Paper A", pages: { pa1: "RM_NEW", pa3: "RM_A2" } },
  { op: "putRmPages", item: "Paper B", pages: { pb3: "RM_NEW" } },
  { op: "move", item: "Paper B", parent: "trash" },
  { op: "move", item: "Upload ✓", parent: ID.zotero },
  { op: "purge", item: "Trashed" },
  { op: "listItems", refresh: true },
  { op: "fetchAnnotations", item: "Paper A" },
];
const ERRORS = [
  { op: "putRmPages", item: "Paper A", stale: true, pages: { pa1: "RM_NEW" }, expect: "hashNotFound" },
  { op: "putRmPages", item: "Paper A", pages: { nope: "RM_NEW" }, expect: "other" },
  { op: "putRmPages", item: "Paper B", pages: { pb2: "RM_NEW" }, expect: "other" },
  { op: "move", item: "Paper A", parent: "not-an-id", expect: "validation" },
  { op: "updateDocument", item: "Zotero", update: { x: 1 }, expect: "other" },
  { op: "updateDocument", item: "Paper A", stale: true, update: { x: 1 }, expect: "hashNotFound" },
  { op: "purge", item: "Paper A", stale: true, expect: "hashNotFound" },
  { op: "putFolder", name: "x", parent: "bad parent", expect: "validation" },
  { op: "getContent", item: "Paper A", stale: true, expect: "response" },
  { op: "listItems", expect: "ok" },
];

const fail = (match, response, times = 1) => ({ match, response, times });
const isFiles = (method) => (r) => r.method === method && r.url.includes("/sync/v3/files/");
const SCENARIOS = [
  { name: "reads, schema 4", schema: 4, ops: READS },
  { name: "reads, schema 3", schema: 3, ops: READS },
  { name: "reads, small cache", schema: 4, ops: [...READS, ...READS], options: { maxCachedBytes: 300 } },
  { name: "writes, schema 4", schema: 4, ops: WRITES },
  { name: "writes, schema 3", schema: 3, ops: WRITES },
  { name: "errors", schema: 4, ops: ERRORS },
  {
    name: "transient failures",
    schema: 4,
    ops: [{ op: "listItems" }, { op: "putFolder", name: "F" }, { op: "putRmPages", item: "Paper A", pages: { pa1: "RM_NEW" } }],
    faults: [
      fail(isFiles("GET"), { status: 503 }, 2),
      fail(isFiles("PUT"), { status: 500 }, 1),
      fail((r) => r.url.endsWith("/sync/v4/root"), { status: 429 }, 1),
      fail(isFiles("GET"), "network", 1),
    ],
  },
  {
    name: "too many failures",
    schema: 4,
    ops: [{ op: "listItems", expect: "network" }, { op: "listRefs", expect: "response" }],
    faults: [fail(isFiles("GET"), { status: 502 }, 4), fail((r) => r.url.endsWith("/sync/v4/root"), "network", 4)],
  },
  {
    name: "root PUT failures are not retried",
    schema: 4,
    ops: [{ op: "putFolder", name: "F", expect: "response" }, { op: "listItems", refresh: true, expect: "ok" }],
    faults: [fail((r) => r.method === "PUT" && r.url.endsWith("/sync/v3/root"), { status: 503 }, 1)],
  },
  {
    name: "concurrent writers",
    schema: 4,
    ops: [
      { op: "listItems" },
      { op: "putFolder", name: "Mine", expect: "ok" },
      { op: "putRmPages", item: "Paper A", pages: { pa2: "RM_NEW" }, expect: "ok" },
      { op: "move", item: "Paper B", parent: "trash" },
      { op: "listItems" },
      { op: "listItems", refresh: true },
    ],
    // another device commits before our first two root PUTs
    rootPutHooks: [
      (c) => c.addItem(ID.other, folder(ID.other, "Theirs", "")),
      (c) => c.setRoot(c.rootEntries()),
    ],
  },
  {
    name: "a concurrent edit of the target",
    schema: 4,
    ops: [{ op: "listItems" }, { op: "putRmPages", item: "Paper A", pages: { pa1: "RM_NEW" }, expect: "hashNotFound" }],
    rootPutHooks: [
      (c) => {
        const files = [{ name: `${ID.a}.metadata`, bytes: json({ visibleName: "Paper A", type: "DocumentType", parent: "", pinned: false }) },
          { name: `${ID.a}.content`, bytes: json({ tags: [] }) }];
        c.addItem(ID.a, files);
      },
    ],
  },
  { name: "too many conflicts", schema: 4, ops: [{ op: "putFolder", name: "F", expect: "generation" }], rootPutHooks: Array.from({ length: 12 }, () => (c) => c.setRoot(c.rootEntries())) },
];

// --- run --------------------------------------------------------------------

async function scenario(sc) {
  const base = seed(sc.schema);
  const clouds = { rmapi: base.clone(), rust: base.clone() };
  const options = { rawHost: RAW_HOST, uploadHost: UPLOAD_HOST, ...sc.options };
  const token = sessionToken("device-1");
  const api = rmapi.session(token, options);
  const rust = new wasm.CloudSession(token, JSON.stringify(options));
  for (const [who, cloud] of Object.entries(clouds)) {
    cloud.faults = (sc.faults ?? []).map((f) => ({ ...f }));
    cloud.beforeRootPut = [...(sc.rootPutHooks ?? [])];
  }
  for (const [i, op] of sc.ops.entries()) {
    const results = {};
    for (const who of ["rmapi", "rust"]) {
      const cloud = clouds[who];
      globalThis.fetch = cloud.fetch;
      uuidPrefix = i + 1;
      uuidCount = 0;
      NOW = 1_700_000_000_000 + i * 1000;
      results[who] = await outcome(who === "rmapi" ? runRmapi(api, cloud, op) : runRust(rust, cloud, op, i + 1));
    }
    const what = `${sc.name}: op ${i} ${op.op}`;
    if (op.expect) {
      const got = results.rust.error?.kind ?? "ok";
      assert.equal(got, op.expect, `${what}: expected ${op.expect} (${results.rust.message ?? ""})`);
    }
    if (results.rmapi.error || results.rust.error) {
      assert.deepEqual(results.rust.error, results.rmapi.error, `${what}: error\n${json(results)}`);
    } else {
      assert.equal(results.rust.json ?? results.rust.bytes, results.rmapi.json ?? results.rmapi.bytes, `${what}: result`);
    }
  }
  assert.deepEqual(clouds.rust.state(), clouds.rmapi.state(), `${sc.name}: final cloud`);
  assert.deepEqual(rootPuts(clouds.rust), rootPuts(clouds.rmapi), `${sc.name}: root updates`);
  const puts = (c) => c.log.filter((r) => r.method === "PUT").map((r) => `${r.url} ${r.filename} ${r.body}`).sort();
  if (sc.rootPutHooks?.length) {
    // the conflicts did happen: more root PUTs than successful commits
    assert.ok(rootPuts(clouds.rust).length > clouds.rust.root.generation - base.root.generation - sc.rootPutHooks.length);
  }
  return { requests: clouds.rust.log.length, rmapiRequests: clouds.rmapi.log.length, puts: puts(clouds.rust).length, rootPuts: rootPuts(clouds.rust).length };
}

async function auth() {
  const cloud = new FakeCloud(4);
  const results = [];
  for (const [who, run] of [
    ["rmapi", async (code, token) => [
      await outcome(rmapi.register(code, { deviceDesc: "browser-chrome", uuid: "dev-uuid", authHost: AUTH_HOST })),
      await outcome(rmapi.auth(token, { authHost: AUTH_HOST })),
    ]],
    ["rust", async (code, token) => [
      await outcome(drive(cloud, wasm.cloudRegister(code, "browser-chrome", "dev-uuid", AUTH_HOST))),
      await outcome(drive(cloud, wasm.cloudAuth(token, AUTH_HOST))),
    ]],
  ]) {
    globalThis.fetch = cloud.fetch;
    results.push([who, [...(await run("ABCDEFGH", "device-token-dev-uuid")), ...(await run("BADBADBA", "nope")), ...(await run("short", "nope"))]]);
  }
  const [[, a], [, b]] = results;
  const norm = (r) => r.map((x) => (x.error ? { error: x.error } : { json: x.json === undefined ? undefined : x.json.replace(/^"|"$/g, "") }));
  assert.deepEqual(norm(b), norm(a), "register/auth");
  // the session token's device id
  assert.equal(new wasm.CloudSession(sessionToken("dev-x")).deviceId, "dev-x");
  assert.throws(() => new wasm.CloudSession("not-a-jwt"));
}

// rmapi-js unrefs its backoff timers: keep node alive while they run
const keepAlive = setInterval(() => {}, 1 << 30);
await auth();
console.log("register/auth: OK");
for (const sc of SCENARIOS) {
  const stats = await scenario(sc);
  console.log(`${sc.name}: OK (${sc.ops.length} ops; requests: rust ${stats.requests}, rmapi-js ${stats.rmapiRequests}; ${stats.puts} uploads, ${stats.rootPuts} root PUTs)`);
}

// Round trips: listing runs the documents concurrently (as rmapi-js's
// Promise.all), so it waits on the host a constant number of times.
{
  const cloud = new FakeCloud(4);
  for (let i = 0; i < 50; i++) {
    const id = `40000000-0000-4000-8000-${String(i).padStart(12, "0")}`;
    cloud.addItem(id, folder(id, `F${i}`, ""));
  }
  globalThis.fetch = cloud.fetch;
  const options = { rawHost: RAW_HOST, uploadHost: UPLOAD_HOST };
  const s = new wasm.CloudSession(sessionToken("device-1"), JSON.stringify(options));
  trips.count = 0;
  trips.widest = 0;
  const { json: listed } = await drive(cloud, s.listItems(true));
  assert.equal(JSON.parse(listed).length, 50);
  assert.equal(trips.count, 4, "round trips to list 50 documents");
  assert.equal(trips.widest, 100, "requests in the widest batch");
  const rmapiList = await rmapi.session(sessionToken("device-1"), options).listItems(true);
  assert.equal(listed, JSON.stringify(rmapiList), "same listing as rmapi-js");
  console.log(`listing 50 documents: OK (${trips.count} round trips, up to ${trips.widest} requests at once)`);
}
clearInterval(keepAlive);
