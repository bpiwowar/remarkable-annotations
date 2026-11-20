//! A JSON view of the blocks in the shape of rmapi-js's parsed blocks
//! (`parseRmScene(bytes).blocks`), with maps as `[key, value]` pairs and byte
//! arrays as hex strings. The tests compare it with rmapi-js's output
//! (`tests/reference`).
//!
//! The view follows rmapi-js where it loses information: item values kept
//! raw are left out, glyph lengths default to the text's UTF-16 length,
//! version 1 points are converted to version 2 units, and ASCII flags and
//! the header are not shown.

use serde_json::{json, Map, Value};

use super::*;

/// The blocks of `scene` as JSON.
pub fn blocks_json(scene: &Scene) -> Value {
    Value::Array(scene.blocks.iter().map(block_json).collect())
}

fn hex(bytes: &[u8]) -> Value {
    Value::String(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn num(v: f64) -> Value {
    // JSON.stringify writes non-finite numbers as null.
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

fn id(i: CrdtId) -> Value {
    json!({"authorId": i.author, "counter": i.counter})
}

fn lww<T>(l: &Lww<T>, f: impl FnOnce(&T) -> Value) -> Value {
    json!({"timestamp": id(l.timestamp), "value": f(&l.value)})
}

fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

fn block_json(b: &Block) -> Value {
    let mut v = body_json(&b.body);
    let m = v.as_object_mut().unwrap();
    m.insert("reserved".into(), json!(b.reserved));
    m.insert("minVersion".into(), json!(b.min_version));
    m.insert("currentVersion".into(), json!(b.current_version));
    m.insert("extraData".into(), hex(&b.extra_data));
    v
}

fn item_json<V>(kind: &str, b: &ItemBlock<V>, f: impl FnOnce(&V) -> Value) -> Value {
    let it = &b.item;
    let item = obj(vec![
        ("itemId", Some(id(it.item_id))),
        ("leftId", Some(id(it.left_id))),
        ("rightId", Some(id(it.right_id))),
        ("deletedLength", Some(json!(it.deleted_length))),
        ("value", it.value.as_ref().and_then(|v| v.value()).map(f)),
    ]);
    json!({"type": kind, "parentId": id(b.parent_id), "item": item})
}

fn line_json(l: &Line) -> Value {
    let points: Vec<Value> = match &l.points {
        Points::V2(ps) => ps
            .iter()
            .map(|p| {
                json!({"x": num(p.x.into()), "y": num(p.y.into()), "speed": p.speed,
                       "width": p.width, "direction": p.direction, "pressure": p.pressure})
            })
            .collect(),
        // Same arithmetic as rmapi-js, in doubles.
        Points::V1(ps) => ps
            .iter()
            .map(|p| {
                json!({
                    "x": num(p.x.into()),
                    "y": num(p.y.into()),
                    "speed": num(f64::from(p.speed) * 4.0),
                    "direction": num((f64::from(p.direction) * 255.0) / (2.0 * std::f64::consts::PI)),
                    "width": num(f64::from(p.width) * 4.0),
                    "pressure": num(f64::from(p.pressure) * 255.0),
                })
            })
            .collect(),
    };
    obj(vec![
        ("tool", Some(json!(l.tool))),
        ("color", Some(json!(l.color))),
        ("thicknessScale", Some(num(l.thickness_scale))),
        ("startingLength", Some(num(l.starting_length.into()))),
        ("points", Some(Value::Array(points))),
        ("timestampId", l.timestamp_id.map(id)),
        ("moveId", l.move_id.map(id)),
        ("colorRgba", l.color_rgba.map(|c| json!(c))),
    ])
}

fn glyph_json(g: &GlyphRange) -> Value {
    let rects: Vec<Value> = g
        .rectangles
        .iter()
        .map(|r| json!({"x": num(r.x), "y": num(r.y), "w": num(r.w), "h": num(r.h)}))
        .collect();
    obj(vec![
        ("start", g.start.map(|s| json!(s))),
        ("length", Some(json!(g.len()))),
        ("color", Some(json!(g.color))),
        ("text", Some(json!(g.text.value))),
        ("rectangles", Some(Value::Array(rects))),
        ("colorRgba", g.color_rgba.map(|c| json!(c))),
    ])
}

fn text_json(t: &Text) -> Value {
    let items: Vec<Value> = t
        .items
        .iter()
        .map(|i| {
            let value = match &i.value {
                None => json!(""),
                Some(TextValue {
                    format: Some(f), ..
                }) => json!(f),
                Some(TextValue { text, .. }) => json!(text.value),
            };
            json!({"itemId": id(i.item_id), "leftId": id(i.left_id), "rightId": id(i.right_id),
                   "deletedLength": i.deleted_length, "value": value})
        })
        .collect();
    // rmapi-js keys styles by char id in a Map: a later duplicate replaces
    // the value but keeps the first position.
    let mut styles: Vec<(String, Value)> = Vec::new();
    for s in &t.styles {
        let key = format!("{}:{}", s.char_id.author, s.char_id.counter);
        let value = json!({"timestamp": id(s.timestamp), "value": s.style});
        match styles.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => styles.push((key, value)),
        }
    }
    let styles: Vec<Value> = styles.into_iter().map(|(k, v)| json!([k, v])).collect();
    json!({"items": items, "styles": styles, "posX": num(t.pos_x), "posY": num(t.pos_y),
           "width": num(t.width.into())})
}

fn body_json(body: &BlockBody) -> Value {
    match body {
        BlockBody::MigrationInfo {
            migration_id,
            is_device,
        } => {
            json!({"type": "migrationInfo", "migrationId": id(*migration_id), "isDevice": is_device})
        }
        BlockBody::SceneTree {
            tree_id,
            node_id,
            is_update,
            parent_id,
        } => json!({"type": "sceneTree", "treeId": id(*tree_id), "nodeId": id(*node_id),
                    "isUpdate": is_update, "parentId": id(*parent_id)}),
        BlockBody::TreeNode {
            node_id,
            label,
            visible,
            anchor,
        } => obj(vec![
            ("type", Some(json!("treeNode"))),
            ("nodeId", Some(id(*node_id))),
            ("label", Some(lww(label, |s| json!(s.value)))),
            ("visible", Some(lww(visible, |b| json!(b)))),
            (
                "anchor",
                anchor.as_ref().map(|a| {
                    json!({
                        "id": lww(&a.id, |i| id(*i)),
                        "type": lww(&a.kind, |k| json!(k)),
                        "threshold": lww(&a.threshold, |t| num((*t).into())),
                        "originX": lww(&a.origin_x, |x| num((*x).into())),
                    })
                }),
            ),
        ]),
        BlockBody::SceneGlyphItem(b) => item_json("sceneGlyphItem", b, glyph_json),
        BlockBody::SceneGroupItem(b) => item_json("sceneGroupItem", b, |i| id(*i)),
        BlockBody::SceneLineItem(b) => item_json("sceneLineItem", b, line_json),
        BlockBody::SceneTextItem(b) => item_json("sceneTextItem", b, |v: &Opaque| match *v {}),
        BlockBody::SceneTombstone(b) => item_json("sceneTombstone", b, |v: &Opaque| match *v {}),
        BlockBody::RootText { block_id, text } => {
            json!({"type": "rootText", "blockId": id(*block_id), "text": text_json(text)})
        }
        BlockBody::AuthorIds { authors } => {
            let a: Vec<Value> = authors.iter().map(|a| json!([a.id, a.uuid()])).collect();
            json!({"type": "authorIds", "authors": a})
        }
        BlockBody::PageInfo {
            loads_count,
            merges_count,
            text_chars_count,
            text_lines_count,
            type_folio_use_count,
        } => obj(vec![
            ("type", Some(json!("pageInfo"))),
            ("loadsCount", Some(json!(loads_count))),
            ("mergesCount", Some(json!(merges_count))),
            ("textCharsCount", Some(json!(text_chars_count))),
            ("textLinesCount", Some(json!(text_lines_count))),
            ("typeFolioUseCount", type_folio_use_count.map(|n| json!(n))),
        ]),
        BlockBody::SceneInfo {
            current_layer,
            background_visible,
            root_document_visible,
            paper_size,
        } => obj(vec![
            ("type", Some(json!("sceneInfo"))),
            ("currentLayer", Some(lww(current_layer, |i| id(*i)))),
            (
                "backgroundVisible",
                background_visible.as_ref().map(|l| lww(l, |b| json!(b))),
            ),
            (
                "rootDocumentVisible",
                root_document_visible.as_ref().map(|l| lww(l, |b| json!(b))),
            ),
            ("paperSize", paper_size.map(|(a, b)| json!([a, b]))),
        ]),
        BlockBody::Unknown {
            block_type,
            data,
            declared_length,
        } => obj(vec![
            ("type", Some(json!("unknown"))),
            ("blockType", Some(json!(block_type))),
            ("data", Some(hex(data))),
            ("declaredLength", declared_length.map(|n| json!(n))),
        ]),
    }
}
