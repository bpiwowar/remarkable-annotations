//! Serializing v6 blocks.

use super::io::Writer;
use super::*;

pub(crate) fn serialize_scene(scene: &Scene) -> Vec<u8> {
    let mut w = Writer::default();
    w.bytes(&scene.header);
    for block in &scene.blocks {
        let length_at = w.len();
        w.u32(0);
        w.u8(block.reserved);
        w.u8(block.min_version);
        w.u8(block.current_version);
        w.u8(block.body.block_type());
        let body_start = w.len();
        write_body(&mut w, &block.body);
        w.bytes(&block.extra_data);
        let length = match &block.body {
            BlockBody::Unknown {
                declared_length: Some(n),
                ..
            } => *n,
            _ => (w.len() - body_start) as u32,
        };
        w.patch_u32(length_at, length);
    }
    w.bytes(&scene.trailing);
    w.buf
}

fn write_body(w: &mut Writer, body: &BlockBody) {
    match body {
        BlockBody::MigrationInfo {
            migration_id,
            is_device,
        } => {
            w.write_id(1, *migration_id);
            w.write_bool(2, *is_device);
        }
        BlockBody::SceneTree {
            tree_id,
            node_id,
            is_update,
            parent_id,
        } => {
            w.write_id(1, *tree_id);
            w.write_id(2, *node_id);
            w.write_bool(3, *is_update);
            w.subblock(4, |w| w.write_id(1, *parent_id));
        }
        BlockBody::TreeNode {
            node_id,
            label,
            visible,
            anchor,
        } => {
            w.write_id(1, *node_id);
            w.write_lww(2, label, |w, v| w.write_string(2, v));
            w.write_lww(3, visible, |w, v| w.write_bool(2, *v));
            if let Some(a) = anchor {
                w.write_lww(7, &a.id, |w, v| w.write_id(2, *v));
                w.write_lww(8, &a.kind, |w, v| w.write_byte(2, *v));
                w.write_lww(9, &a.threshold, |w, v| w.write_float(2, *v));
                w.write_lww(10, &a.origin_x, |w, v| w.write_float(2, *v));
            }
        }
        BlockBody::SceneGlyphItem(b) => write_item(w, b, 0x01, write_glyph),
        BlockBody::SceneGroupItem(b) => write_item(w, b, 0x02, |w, id| w.write_id(2, *id)),
        BlockBody::SceneLineItem(b) => write_item(w, b, 0x03, write_line),
        BlockBody::SceneTextItem(b) | BlockBody::SceneTombstone(b) => {
            write_item(w, b, 0x00, |_, v: &Opaque| match *v {})
        }
        BlockBody::RootText { block_id, text } => {
            w.write_id(1, *block_id);
            write_text(w, text);
        }
        BlockBody::AuthorIds { authors } => {
            w.varuint(authors.len() as u64);
            for a in authors {
                w.subblock(0, |w| {
                    w.varuint(16);
                    w.bytes(&a.uuid_le);
                    w.u16(a.id);
                });
            }
        }
        BlockBody::PageInfo {
            loads_count,
            merges_count,
            text_chars_count,
            text_lines_count,
            type_folio_use_count,
        } => {
            w.write_int(1, *loads_count);
            w.write_int(2, *merges_count);
            w.write_int(3, *text_chars_count);
            w.write_int(4, *text_lines_count);
            if let Some(n) = type_folio_use_count {
                w.write_int(5, *n);
            }
        }
        BlockBody::SceneInfo {
            current_layer,
            background_visible,
            root_document_visible,
            paper_size,
        } => {
            w.write_lww(1, current_layer, |w, v| w.write_id(2, *v));
            if let Some(v) = background_visible {
                w.write_lww(2, v, |w, v| w.write_bool(2, *v));
            }
            if let Some(v) = root_document_visible {
                w.write_lww(3, v, |w, v| w.write_bool(2, *v));
            }
            if let Some((a, b)) = paper_size {
                w.subblock(5, |w| {
                    w.u32(*a);
                    w.u32(*b);
                });
            }
        }
        BlockBody::Unknown { data, .. } => w.bytes(data),
    }
}

fn write_item<V>(
    w: &mut Writer,
    b: &ItemBlock<V>,
    item_type: u8,
    write_value: impl FnOnce(&mut Writer, &V),
) {
    w.write_id(1, b.parent_id);
    w.write_id(2, b.item.item_id);
    w.write_id(3, b.item.left_id);
    w.write_id(4, b.item.right_id);
    w.write_int(5, b.item.deleted_length);
    match &b.item.value {
        None => {}
        Some(ItemValue::Value(v)) => w.subblock(6, |w| {
            w.u8(item_type);
            write_value(w, v);
        }),
        Some(ItemValue::Raw { item_type, data }) => w.subblock(6, |w| {
            w.u8(*item_type);
            w.bytes(data);
        }),
    }
}

fn write_line(w: &mut Writer, line: &Line) {
    w.write_int(1, line.tool);
    w.write_int(2, line.color);
    w.write_double(3, line.thickness_scale);
    w.write_float(4, line.starting_length);
    w.subblock(5, |w| match &line.points {
        Points::V1(points) => {
            for p in points {
                for v in [p.x, p.y, p.speed, p.direction, p.width, p.pressure] {
                    w.f32(v);
                }
            }
        }
        Points::V2(points) => {
            for p in points {
                w.f32(p.x);
                w.f32(p.y);
                w.u16(p.speed);
                w.u16(p.width);
                w.u8(p.direction);
                w.u8(p.pressure);
            }
        }
    });
    if let Some(id) = line.timestamp_id {
        w.write_id(6, id);
    }
    if let Some(id) = line.move_id {
        w.write_id(7, id);
    }
    if let Some(c) = line.color_rgba {
        w.write_int(8, c);
    }
}

fn write_glyph(w: &mut Writer, g: &GlyphRange) {
    if let Some(s) = g.start {
        w.write_int(2, s);
    }
    if let Some(n) = g.length {
        w.write_int(3, n);
    }
    w.write_int(4, g.color);
    w.write_string(5, &g.text);
    w.subblock(6, |w| {
        w.varuint(g.rectangles.len() as u64);
        for r in &g.rectangles {
            w.f64(r.x);
            w.f64(r.y);
            w.f64(r.w);
            w.f64(r.h);
        }
    });
    if let Some(c) = g.color_rgba {
        w.write_int(10, c);
    }
}

fn write_text(w: &mut Writer, text: &Text) {
    w.subblock(2, |w| {
        w.subblock(1, |w| {
            w.subblock(1, |w| {
                w.varuint(text.items.len() as u64);
                for item in &text.items {
                    w.subblock(0, |w| {
                        w.write_id(2, item.item_id);
                        w.write_id(3, item.left_id);
                        w.write_id(4, item.right_id);
                        w.write_int(5, item.deleted_length);
                        if let Some(v) = &item.value {
                            w.subblock(6, |w| {
                                w.string_body(&v.text);
                                if let Some(f) = v.format {
                                    w.write_int(2, f);
                                }
                            });
                        }
                    });
                }
            });
        });
        w.subblock(2, |w| {
            w.subblock(1, |w| {
                w.varuint(text.styles.len() as u64);
                for s in &text.styles {
                    w.crdt_id(s.char_id);
                    w.write_id(1, s.timestamp);
                    w.subblock(2, |w| {
                        w.u8(s.tag);
                        w.u8(s.style);
                    });
                }
            });
        });
    });
    w.subblock(3, |w| {
        w.f64(text.pos_x);
        w.f64(text.pos_y);
    });
    w.write_float(4, text.width);
}
