// An in-memory reMarkable cloud: the auth and sync endpoints rmapi-js uses,
// with fault injection (transient errors, network failures, other writers
// changing the root). Used by cloud-check.mjs to run rmapi-js and the Rust
// client side by side.
import { createHash } from "node:crypto";

export const AUTH_HOST = "https://auth.test";
export const RAW_HOST = "https://raw.test";
export const UPLOAD_HOST = "https://upload.test";

const enc = new TextEncoder();
const dec = new TextDecoder();
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// CRC-32C, for the x-goog-hash header
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0x82f63b78 ^ (c >>> 1) : c >>> 1;
    t[i] = c >>> 0;
  }
  return t;
})();
function crc32c(bytes) {
  let crc = 0xffffffff;
  for (const b of bytes) crc = CRC_TABLE[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

const b64url = (s) => Buffer.from(s).toString("base64url");
export function sessionToken(deviceId) {
  return [
    b64url(JSON.stringify({ alg: "none" })),
    b64url(JSON.stringify({ "device-id": deviceId, "auth0-userid": "user" })),
    "sig",
  ].join(".");
}

export class FakeCloud {
  /** hash -> bytes */
  files = new Map();
  root = { hash: "", generation: 0, schemaVersion: 4 };
  /** every request: {method, url, filename, bodyHash} */
  log = [];
  /** faults: {match(req) -> bool, times, response: {status, body} | "network"} */
  faults = [];
  /** run before handling a root PUT, one per PUT: (cloud) => void */
  beforeRootPut = [];

  constructor(schemaVersion = 4) {
    this.root.schemaVersion = schemaVersion;
  }

  clone() {
    const c = new FakeCloud(this.root.schemaVersion);
    c.files = new Map(this.files);
    c.root = { ...this.root };
    return c;
  }

  // --- seeding (and the "other writer" of conflict scenarios) ------------

  put(bytes) {
    const b = typeof bytes === "string" ? enc.encode(bytes) : bytes;
    const hash = sha256(b);
    this.files.set(hash, b);
    return { hash, size: b.length };
  }

  /** an index file, as rmapi-js's putEntries writes it */
  index(id, entries, schema) {
    const sorted = [...entries].sort((a, b) => a.id.localeCompare(b.id));
    const size = sorted.reduce((acc, e) => acc + e.size, 0);
    let text = `${schema}\n`;
    if (schema === 4) text += `0:${id === "root" ? "." : id}:${sorted.length}:${size}\n`;
    for (const e of sorted) {
      text += `${e.hash}:${schema === 4 ? 0 : e.type}:${e.id}:${e.subfiles}:${e.size}\n`;
    }
    const bytes = enc.encode(text);
    const hash =
      schema === 3
        ? sha256(Buffer.concat(sorted.map((e) => Buffer.from(e.hash, "hex"))))
        : sha256(bytes);
    this.files.set(hash, bytes);
    return { id, hash, type: schema === 3 ? 80000000 : 0, subfiles: sorted.length, size };
  }

  /** adds an item made of `files` ({name, bytes}) to the root */
  addItem(id, files, { commit = true } = {}) {
    const schema = this.root.schemaVersion;
    const entries = files.map(({ name, bytes }) => ({
      id: name,
      ...this.put(bytes),
      type: 0,
      subfiles: 0,
    }));
    const entry = this.index(id, entries, schema);
    if (commit) {
      const rootEntries = this.rootEntries().filter((e) => e.id !== id);
      rootEntries.push(entry);
      this.setRoot(rootEntries);
    }
    return entry;
  }

  rootEntries() {
    if (!this.root.hash) return [];
    const text = dec.decode(this.files.get(this.root.hash)).slice(0, -1);
    const [version, ...rest] = text.split("\n");
    const lines = version === "4" ? rest.slice(1) : rest;
    return lines.map((l) => {
      const [hash, type, id, subfiles, size] = l.split(":");
      return { hash, type: +type, id, subfiles: +subfiles, size: +size };
    });
  }

  setRoot(entries) {
    // seeds keep the account's schema for the root, like real accounts
    const e = this.index("root", entries, this.root.schemaVersion);
    this.root.hash = e.hash;
    this.root.generation += 1;
  }

  /** the item (id, hash) named `visibleName` in the current root */
  find(visibleName) {
    for (const e of this.rootEntries()) {
      const text = dec.decode(this.files.get(e.hash));
      const metaLine = text.split("\n").find((l) => l.includes(".metadata"));
      if (!metaLine) continue;
      const meta = JSON.parse(dec.decode(this.files.get(metaLine.split(":")[0])));
      if (meta.visibleName === visibleName) return { id: e.id, hash: e.hash };
    }
    throw new Error(`no item ${visibleName}`);
  }

  /** the state to compare: root and every stored file */
  state() {
    const files = [...this.files.keys()].sort();
    return { root: this.root, files };
  }

  // --- HTTP ---------------------------------------------------------------

  fetch = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const headers = new Headers(init.headers ?? {});
    const body =
      init.body == null
        ? new Uint8Array()
        : typeof init.body === "string"
          ? enc.encode(init.body)
          : new Uint8Array(init.body);
    const req = { method, url, headers, body };
    this.log.push({
      method,
      url,
      filename: headers.get("rm-filename"),
      body: body.length ? sha256(body) : "",
    });
    for (const f of this.faults) {
      if (f.times > 0 && f.match(req)) {
        f.times--;
        if (f.response === "network") throw new TypeError("fetch failed");
        return new Response(f.response.body ?? "fault", { status: f.response.status });
      }
    }
    return this.handle(req);
  };

  handle({ method, url, headers, body }) {
    const u = new URL(url);
    const path = u.pathname;
    const text = (s, status = 200) => new Response(s, { status });
    const json = (o, status = 200) => text(JSON.stringify(o), status);
    if (u.origin === AUTH_HOST && method === "POST") {
      if (path === "/token/json/2/device/new") {
        const { code, deviceID } = JSON.parse(dec.decode(body));
        if (code === "BADBADBA") return text("invalid code", 400);
        return text(`device-token-${deviceID}`);
      }
      if (path === "/token/json/2/user/new") {
        const auth = headers.get("authorization") ?? "";
        if (!auth.startsWith("Bearer device-token-")) return text("unauthorized", 401);
        return text(sessionToken(auth.slice("Bearer device-token-".length)));
      }
    }
    if (u.origin !== RAW_HOST) return text("not found", 404);
    if (!(headers.get("authorization") ?? "").startsWith("Bearer ")) {
      return text("unauthorized", 401);
    }
    if (path === "/sync/v4/root" && method === "GET") return json(this.root);
    if (path === "/sync/v3/root" && method === "PUT") {
      if (this.beforeRootPut.length) this.beforeRootPut.shift()(this);
      const { hash, generation } = JSON.parse(dec.decode(body));
      if (generation !== this.root.generation) {
        return text('{"message":"precondition failed"}\n', 412);
      }
      if (!this.files.has(hash)) return text("root file missing", 400);
      this.root.hash = hash;
      this.root.generation += 1;
      return json({ hash, generation: this.root.generation });
    }
    const m = /^\/sync\/v3\/files\/([0-9a-f]{64})$/.exec(path);
    if (m && method === "GET") {
      const bytes = this.files.get(m[1]);
      return bytes ? new Response(bytes) : text("not found", 404);
    }
    if (m && method === "PUT") {
      const crc = Buffer.alloc(4);
      crc.writeUInt32BE(crc32c(body));
      if (headers.get("x-goog-hash") !== `crc32c=${crc.toString("base64")}`) {
        return text("bad crc32c", 400);
      }
      const name = headers.get("rm-filename") ?? "";
      const schema3Index = name.endsWith(".docSchema") && dec.decode(body).startsWith("3\n");
      if (!schema3Index && sha256(body) !== m[1]) return text("bad hash", 400);
      this.files.set(m[1], body);
      return text("");
    }
    return text("not found", 404);
  }
}
