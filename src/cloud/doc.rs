//! Pages of a document from its `.content` (the plugin's rmdoc.ts, and
//! rmapi-js's `pageOrder`).

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

/// Page ids for writing pages (rmapi-js's `pageOrder`): `cPages` without
/// deleted pages, else `pages`.
pub(crate) fn page_order(content: &Value) -> Vec<String> {
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    match content.get("cPages") {
        Some(c) if !c.is_null() => c["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p.get("deleted").is_none())
            .filter_map(|p| p["id"].as_str().map(str::to_owned))
            .collect(),
        _ => match content.get("pages") {
            Some(p) if !p.is_null() => strings(p),
            _ => Vec::new(),
        },
    }
}

/// The ordered page ids of a document (rmdoc.ts's `orderedPageIds`): `pages`
/// when not empty, else the `cPages` not deleted.
pub fn ordered_page_ids(content: Option<&Value>) -> Vec<String> {
    let Some(content) = content else {
        return Vec::new();
    };
    if let Some(pages) = content.get("pages").and_then(Value::as_array) {
        if !pages.is_empty() {
            return pages
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect();
        }
    }
    let Some(cpages) = content
        .get("cPages")
        .and_then(|c| c.get("pages"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    cpages
        .iter()
        .filter(|p| match p.get("deleted") {
            None | Some(Value::Null) => true,
            Some(d) => d.get("value").and_then(Value::as_f64) == Some(0.0),
        })
        .filter_map(|p| p["id"].as_str().map(str::to_owned))
        .collect()
}

/// Each page id's 0-based PDF page index, from `redirectionPageMap`
/// (rmdoc.ts's `pdfIndexByPageId`).
pub fn pdf_index_by_page_id(content: Option<&Value>, page_ids: &[String]) -> HashMap<String, f64> {
    let redirect = content
        .and_then(|c| c.get("redirectionPageMap"))
        .and_then(Value::as_array);
    let mut out = HashMap::new();
    for (i, id) in page_ids.iter().enumerate() {
        let mapped = redirect.and_then(|r| r.get(i)).and_then(Value::as_f64);
        let index = match mapped {
            Some(m) if m >= 0.0 => m,
            _ => i as f64,
        };
        out.insert(id.clone(), index);
    }
    out
}

/// Tags of one page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageTags {
    pub pdf_page_index: f64,
    /// Sorted, without duplicates.
    pub names: Vec<String>,
}

/// JavaScript's `String.prototype.trim` whitespace.
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// Page tags grouped by PDF page (rmdoc.ts's `pageTagsOf`).
pub fn page_tags(content: Option<&Value>, page_ids: &[String]) -> Vec<PageTags> {
    let Some(tags) = content
        .and_then(|c| c.get("pageTags"))
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    else {
        return Vec::new();
    };
    let index_of = pdf_index_by_page_id(content, page_ids);
    let mut by_page: Vec<(f64, Vec<String>)> = Vec::new();
    for t in tags {
        let idx = t
            .get("pageId")
            .and_then(Value::as_str)
            .and_then(|p| index_of.get(p));
        let name = t
            .get("name")
            .and_then(Value::as_str)
            .map(js_trim)
            .unwrap_or("");
        let (Some(&idx), false) = (idx, name.is_empty()) else {
            continue;
        };
        match by_page.iter_mut().find(|(i, _)| *i == idx) {
            Some((_, names)) => {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
            None => by_page.push((idx, vec![name.to_owned()])),
        }
    }
    by_page.sort_by(|a, b| a.0.total_cmp(&b.0));
    by_page
        .into_iter()
        .map(|(pdf_page_index, mut names)| {
            // Array.prototype.sort: UTF-16 code unit order
            names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            PageTags {
                pdf_page_index,
                names,
            }
        })
        .collect()
}
