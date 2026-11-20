// The Zotero plugin's document helpers on top of rmapi-js (its TypeScript
// rmdoc.ts and document.ts, types removed), the reference for the Rust
// client's fetchAnnotations, mapPdfPages, uploadPdf and ensureFolder. The
// page parser is injected (it is tested on its own).

function pageTagsOf(content, pageIds) {
  const tags = content?.pageTags;
  if (!tags?.length) return [];
  const indexOf = pdfIndexByPageId(content, pageIds);
  const byPage = new Map();
  for (const t of tags) {
    const idx = indexOf.get(t.pageId);
    const name = t.name?.trim();
    if (idx === undefined || !name) continue;
    (byPage.get(idx) ?? byPage.set(idx, new Set()).get(idx)).add(name);
  }
  return [...byPage]
    .sort(([a], [b]) => a - b)
    .map(([pdfPageIndex, names]) => ({ pdfPageIndex, names: [...names].sort() }));
}

export function pdfIndexByPageId(content, pageIds = orderedPageIds(content)) {
  const redirect = content?.redirectionPageMap;
  const indexOf = new Map();
  pageIds.forEach((id, i) => {
    const mapped = redirect?.[i];
    indexOf.set(id, typeof mapped === "number" && mapped >= 0 ? mapped : i);
  });
  return indexOf;
}

function orderedPageIds(content) {
  if (!content) return [];
  if (Array.isArray(content.pages) && content.pages.length) return content.pages;
  const cpages = content.cPages?.pages;
  if (cpages?.length) {
    return cpages.filter((p) => !p.deleted || p.deleted.value === 0).map((p) => p.id);
  }
  return [];
}

async function readDoc(api, id, hash) {
  const ref = { id, hash };
  const [{ entries }, content] = await Promise.all([
    api.raw.getEntries(ref),
    api.getContent(ref).then((c) => c, () => null),
  ]);
  const rmByPage = new Map();
  for (const e of entries) {
    if (e.id.endsWith(".rm")) {
      const base = e.id.slice(0, -3).split("/").pop();
      rmByPage.set(base, { id: e.id, hash: e.hash });
    }
  }
  const pageIds = orderedPageIds(content);
  const pdfIndex = pdfIndexByPageId(content, pageIds);
  return { content, pageIds, pdfIndex, rmByPage };
}

export async function fetchAnnotations(api, id, hash, parsePage) {
  const { content, pageIds, pdfIndex, rmByPage } = await readDoc(api, id, hash);
  const pages = [];
  for (const pageUuid of pageIds) {
    const ent = rmByPage.get(pageUuid);
    if (!ent) continue;
    let parsed;
    try {
      parsed = parsePage(await api.raw.getHash(ent));
    } catch {
      continue;
    }
    if (!parsed.strokes.length && !parsed.highlights.length) continue;
    pages.push({ pdfPageIndex: pdfIndex.get(pageUuid), pageUuid, page: parsed });
  }
  return { pages, pageTags: pageTagsOf(content, pageIds), content };
}

export async function mapPdfPages(api, id, hash) {
  const { pageIds, pdfIndex, rmByPage } = await readDoc(api, id, hash);
  const byPdfIndex = new Map();
  for (const pageUuid of pageIds) {
    byPdfIndex.set(pdfIndex.get(pageUuid), { pageUuid, rmEntry: rmByPage.get(pageUuid) });
  }
  return byPdfIndex;
}

export async function ensureFolder(api, path) {
  const parts = path.split("/").map((s) => s.trim()).filter(Boolean);
  if (parts.length === 0) return "";
  const items = await api.listItems(true);
  let parent = "";
  for (const name of parts) {
    const found = items.find(
      (e) => e.type === "CollectionType" && e.visibleName === name && (e.parent ?? "") === parent,
    );
    if (found) parent = found.id;
    else parent = (await api.putFolder(name, { parent })).id;
  }
  return parent;
}

export async function uploadPdf(api, visibleName, bytes, folderId, opts = {}) {
  const { tags = [], pageCount, pageTags = {} } = opts;
  const ref = await api.putPdf(visibleName, bytes, { parent: folderId, tags });
  if (!pageCount || pageCount < 1) return ref;
  const pages = Array.from({ length: pageCount }, () => globalThis.crypto.randomUUID());
  const now = Date.now();
  return api.updateDocument(ref, {
    originalPageCount: pages.length,
    pageCount: pages.length,
    pages,
    redirectionPageMap: pages.map((_, i) => i),
    pageTags: Object.entries(pageTags).flatMap(([idx, names]) => {
      const pageId = pages[Number(idx)];
      return pageId ? names.map((name) => ({ name, pageId, timestamp: now })) : [];
    }),
  });
}
