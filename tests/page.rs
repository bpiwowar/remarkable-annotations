//! The page layer against the TypeScript implementation
//! (tests/reference/page-dump.mjs): the same annotations for every fixture,
//! and the same bytes and ids when rebuilding pages.

mod common;

use std::fs;

use common::{diff, dir, fixtures};
use remarkable_annotations::page::{
    blank_structure, parse_page, rebuild_page, split_structure, Highlight, PageUpdate, Stroke,
};
use remarkable_annotations::scene::CrdtId;
use serde::Deserialize;
use serde_json::Value;

#[test]
fn parses_like_typescript() {
    for path in fixtures() {
        let reference: Value =
            serde_json::from_str(&fs::read_to_string(path.with_extension("page.json")).unwrap())
                .unwrap();
        let page = parse_page(&fs::read(&path).unwrap()).unwrap();
        let got = serde_json::to_value(&page).unwrap();
        if let Some(d) = diff(&got, &reference, "page") {
            panic!("{}: {d}", path.display());
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Base {
    Blank { blank: u8 },
    File(String),
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Overrides {
    layer_id: Option<CrdtId>,
    author: Option<u8>,
    start_counter: Option<u64>,
    #[serde(default)]
    authors: Vec<u8>,
    #[serde(default)]
    register: Vec<(u16, String)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    name: String,
    base: Base,
    #[serde(default)]
    strokes: Vec<Stroke>,
    #[serde(default)]
    highlights: Vec<Highlight>,
    #[serde(default)]
    remove_first: bool,
    #[serde(default)]
    update: Overrides,
}

#[derive(Deserialize)]
struct Expected {
    ids: Vec<CrdtId>,
}

#[test]
fn rebuilds_like_typescript() {
    let page_dir = dir("tests/fixtures/page");
    let scenarios: Vec<Scenario> =
        serde_json::from_str(&fs::read_to_string(page_dir.join("scenarios.json")).unwrap())
            .unwrap();
    for s in scenarios {
        let (bytes, mut update) = match &s.base {
            Base::Blank { blank } => {
                let b = blank_structure(*blank);
                (b.bytes, b.update)
            }
            Base::File(f) => {
                let bytes = fs::read(dir("tests/fixtures/rm").join(f)).unwrap();
                let page = parse_page(&bytes).unwrap();
                // A page without strokes or highlights has no layer to
                // default to: the scenario gives it.
                let update = PageUpdate::after(&page).unwrap_or(PageUpdate {
                    layer_id: CrdtId::default(),
                    last_item_id: None,
                    author: page.max_author as u8,
                    start_counter: page.max_counter + 1,
                    authors: Vec::new(),
                    register: Vec::new(),
                });
                (bytes, update)
            }
        };
        let o = s.update;
        if let Some(l) = o.layer_id {
            update.layer_id = l;
        }
        if let Some(a) = o.author {
            update.author = a;
        }
        if let Some(c) = o.start_counter {
            update.start_counter = c;
        }
        update.authors = o.authors;
        update.register = o.register;
        let mut remove = Vec::new();
        if s.remove_first {
            let p = parse_page(&bytes).unwrap();
            let first = p
                .highlights
                .first()
                .map(|h| h.ids[0])
                .or_else(|| p.strokes.first().map(|st| st.ids[0]))
                .unwrap();
            remove.push(first);
        }
        let (out, ids) = rebuild_page(&bytes, &s.strokes, &s.highlights, &remove, &update).unwrap();
        let expected: Expected = serde_json::from_str(
            &fs::read_to_string(page_dir.join(format!("{}.ids.json", s.name))).unwrap(),
        )
        .unwrap();
        assert_eq!(ids, expected.ids, "{}: ids", s.name);
        assert!(
            out == fs::read(page_dir.join(format!("{}.rm", s.name))).unwrap(),
            "{}: bytes differ",
            s.name
        );
        // What we wrote reads back.
        let back = parse_page(&out).unwrap();
        assert!(back.strokes.len() + back.highlights.len() >= ids.len());
    }
}

#[test]
fn structures_like_typescript() {
    let page_dir = dir("tests/fixtures/page");
    assert!(blank_structure(3).bytes == fs::read(page_dir.join("blank_3.rm")).unwrap());
    for f in [
        "Normal_A_stroke_2_layers.rm",
        "Bold_Heading_Bullet_Normal.rm",
    ] {
        let bytes = fs::read(dir("tests/fixtures/rm/rmscene").join(f)).unwrap();
        assert!(
            split_structure(&bytes).unwrap()
                == fs::read(page_dir.join(format!("split_{f}"))).unwrap(),
            "split {f}"
        );
    }
}
