//! Parsing v6 blocks.

use super::io::{Error, Reader, Result, TAG_BYTE4, TAG_ID, TAG_LENGTH4};
use super::*;

pub(crate) fn parse_scene(data: &[u8]) -> Result<Scene> {
    if data.len() < HEADER_LENGTH || !data.starts_with(V6_HEADER.as_bytes()) {
        return Err(Error::Invalid("not a version 6 .rm file".into()));
    }
    let header: [u8; HEADER_LENGTH] = data[..HEADER_LENGTH].try_into().unwrap();
    let mut r = Reader::new(data, HEADER_LENGTH);
    let mut blocks = Vec::new();
    let mut trailing = Vec::new();
    while r.remaining() > 0 {
        if r.remaining() < 8 {
            trailing = r.bytes(r.remaining())?;
            break;
        }
        let length = r.u32()?;
        let reserved = r.u8()?;
        let min_version = r.u8()?;
        let current_version = r.u8()?;
        let block_type = r.u8()?;
        let start = r.offset();
        if length as usize > r.remaining() {
            // The block overruns the file: keep what is left and stop.
            blocks.push(Block {
                reserved,
                min_version,
                current_version,
                body: BlockBody::Unknown {
                    block_type,
                    data: r.bytes(r.remaining())?,
                    declared_length: Some(length),
                },
                extra_data: Vec::new(),
            });
            break;
        }
        let end = start + length as usize;
        let parsed = r.bounded(end, |r| read_body(r, block_type, current_version));
        let (body, extra_data) = match parsed {
            Ok(v) => v,
            Err(_) => {
                // Keep the raw bytes, so that the file still round-trips.
                r.seek(start);
                let data = r.bytes(length as usize)?;
                (
                    BlockBody::Unknown {
                        block_type,
                        data,
                        declared_length: None,
                    },
                    Vec::new(),
                )
            }
        };
        r.seek(end);
        blocks.push(Block {
            reserved,
            min_version,
            current_version,
            body,
            extra_data,
        });
    }
    Ok(Scene {
        header,
        blocks,
        trailing,
    })
}

fn read_body(r: &mut Reader, block_type: u8, version: u8) -> Result<BlockBody> {
    Ok(match block_type {
        0x00 => BlockBody::MigrationInfo {
            migration_id: r.read_id(1)?,
            is_device: r.read_bool(2)?,
        },
        0x01 => BlockBody::SceneTree {
            tree_id: r.read_id(1)?,
            node_id: r.read_id(2)?,
            is_update: r.read_bool(3)?,
            parent_id: r.subblock(4, |r| r.read_id(1))?,
        },
        0x02 => {
            let node_id = r.read_id(1)?;
            let label = r.read_lww(2, |r| r.read_string(2))?;
            let visible = r.read_lww(3, |r| r.read_bool(2))?;
            let anchor = if r.has_tag(7, TAG_LENGTH4) {
                Some(Anchor {
                    id: r.read_lww(7, |r| r.read_id(2))?,
                    kind: r.read_lww(8, |r| r.read_byte(2))?,
                    threshold: r.read_lww(9, |r| r.read_float(2))?,
                    origin_x: r.read_lww(10, |r| r.read_float(2))?,
                })
            } else {
                None
            };
            BlockBody::TreeNode {
                node_id,
                label,
                visible,
                anchor,
            }
        }
        0x03 => BlockBody::SceneGlyphItem(read_item(r, Some(0x01), read_glyph)?),
        0x04 => BlockBody::SceneGroupItem(read_item(r, Some(0x02), |r| r.read_id(2))?),
        0x05 => BlockBody::SceneLineItem(read_item(r, Some(0x03), |r| read_line(r, version))?),
        0x06 => BlockBody::SceneTextItem(read_item(r, None, |_| unreachable!())?),
        0x07 => BlockBody::RootText {
            block_id: r.read_id(1)?,
            text: read_text(r)?,
        },
        0x08 => BlockBody::SceneTombstone(read_item(r, None, |_| unreachable!())?),
        0x09 => {
            let count = r.varuint()?;
            let mut authors = Vec::new();
            for _ in 0..count {
                authors.push(r.subblock(0, |r| {
                    let n = r.varuint()? as usize;
                    if n != 16 {
                        return Err(Error::Invalid(format!("uuid of {n} bytes")));
                    }
                    let uuid_le: [u8; 16] = r.bytes(16)?.try_into().unwrap();
                    let id = r.u16()?;
                    Ok(Author { id, uuid_le })
                })?);
            }
            BlockBody::AuthorIds { authors }
        }
        0x0a => BlockBody::PageInfo {
            loads_count: r.read_int(1)?,
            merges_count: r.read_int(2)?,
            text_chars_count: r.read_int(3)?,
            text_lines_count: r.read_int(4)?,
            type_folio_use_count: if r.has_tag(5, TAG_BYTE4) {
                Some(r.read_int(5)?)
            } else {
                None
            },
        },
        0x0d => {
            let current_layer = r.read_lww(1, |r| r.read_id(2))?;
            let background_visible = if r.has_tag(2, TAG_LENGTH4) {
                Some(r.read_lww(2, |r| r.read_bool(2))?)
            } else {
                None
            };
            let root_document_visible = if r.has_tag(3, TAG_LENGTH4) {
                Some(r.read_lww(3, |r| r.read_bool(2))?)
            } else {
                None
            };
            let paper_size = if r.has_tag(5, TAG_LENGTH4) {
                Some(r.subblock(5, |r| Ok((r.u32()?, r.u32()?)))?)
            } else {
                None
            };
            BlockBody::SceneInfo {
                current_layer,
                background_visible,
                root_document_visible,
                paper_size,
            }
        }
        t => return Err(Error::Invalid(format!("unknown block type {t:#x}"))),
    })
}

/// Reads an item envelope. The value subblock starts with an item type: when
/// it is `expected`, `read_value` parses the rest; otherwise the value is
/// kept raw.
fn read_item<V>(
    r: &mut Reader,
    expected: Option<u8>,
    read_value: impl FnOnce(&mut Reader) -> Result<V>,
) -> Result<ItemBlock<V>> {
    let parent_id = r.read_id(1)?;
    let item_id = r.read_id(2)?;
    let left_id = r.read_id(3)?;
    let right_id = r.read_id(4)?;
    let deleted_length = r.read_int(5)?;
    let value = if r.has_tag(6, TAG_LENGTH4) {
        Some(r.subblock(6, |r| {
            let item_type = r.u8()?;
            if Some(item_type) == expected {
                Ok(ItemValue::Value(read_value(r)?))
            } else {
                Ok(ItemValue::Raw {
                    item_type,
                    data: r.bytes(r.remaining())?,
                })
            }
        })?)
    } else {
        None
    };
    Ok(ItemBlock {
        parent_id,
        item: SceneItem {
            item_id,
            left_id,
            right_id,
            deleted_length,
            value,
        },
    })
}

fn read_line(r: &mut Reader, version: u8) -> Result<Line> {
    let tool = r.read_int(1)?;
    let color = r.read_int(2)?;
    let thickness_scale = r.read_double(3)?;
    let starting_length = r.read_float(4)?;
    let points = r.subblock(5, |r| {
        if version == 1 {
            let n = r.remaining() / 24;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(PointV1 {
                    x: r.f32()?,
                    y: r.f32()?,
                    speed: r.f32()?,
                    direction: r.f32()?,
                    width: r.f32()?,
                    pressure: r.f32()?,
                });
            }
            Ok(Points::V1(v))
        } else {
            let n = r.remaining() / 14;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(PointV2 {
                    x: r.f32()?,
                    y: r.f32()?,
                    speed: r.u16()?,
                    width: r.u16()?,
                    direction: r.u8()?,
                    pressure: r.u8()?,
                });
            }
            Ok(Points::V2(v))
        }
    })?;
    let timestamp_id = opt(r, 6, TAG_ID, |r| r.read_id(6))?;
    let move_id = opt(r, 7, TAG_ID, |r| r.read_id(7))?;
    let color_rgba = opt(r, 8, TAG_BYTE4, |r| r.read_int(8))?;
    Ok(Line {
        tool,
        color,
        thickness_scale,
        starting_length,
        points,
        timestamp_id,
        move_id,
        color_rgba,
    })
}

fn read_glyph(r: &mut Reader) -> Result<GlyphRange> {
    let start = opt(r, 2, TAG_BYTE4, |r| r.read_int(2))?;
    let length = opt(r, 3, TAG_BYTE4, |r| r.read_int(3))?;
    let color = r.read_int(4)?;
    let text = r.read_string(5)?;
    let rectangles = r.subblock(6, |r| {
        let n = r.varuint()?;
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(Rect {
                x: r.f64()?,
                y: r.f64()?,
                w: r.f64()?,
                h: r.f64()?,
            });
        }
        Ok(v)
    })?;
    let color_rgba = opt(r, 10, TAG_BYTE4, |r| r.read_int(10))?;
    Ok(GlyphRange {
        start,
        length,
        color,
        text,
        rectangles,
        color_rgba,
    })
}

fn read_text(r: &mut Reader) -> Result<Text> {
    let (items, styles) = r.subblock(2, |r| {
        let items = r.subblock(1, |r| {
            r.subblock(1, |r| {
                let n = r.varuint()?;
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(r.subblock(0, |r| {
                        let item_id = r.read_id(2)?;
                        let left_id = r.read_id(3)?;
                        let right_id = r.read_id(4)?;
                        let deleted_length = r.read_int(5)?;
                        let value = if r.has_tag(6, TAG_LENGTH4) {
                            Some(r.subblock(6, |r| {
                                let text = r.string_body()?;
                                let format = opt(r, 2, TAG_BYTE4, |r| r.read_int(2))?;
                                Ok(TextValue { text, format })
                            })?)
                        } else {
                            None
                        };
                        Ok(TextItem {
                            item_id,
                            left_id,
                            right_id,
                            deleted_length,
                            value,
                        })
                    })?);
                }
                Ok(items)
            })
        })?;
        let styles = r.subblock(2, |r| {
            r.subblock(1, |r| {
                let n = r.varuint()?;
                let mut styles = Vec::new();
                for _ in 0..n {
                    let char_id = r.crdt_id()?;
                    let timestamp = r.read_id(1)?;
                    let (tag, style) = r.subblock(2, |r| Ok((r.u8()?, r.u8()?)))?;
                    styles.push(TextStyle {
                        char_id,
                        timestamp,
                        tag,
                        style,
                    });
                }
                Ok(styles)
            })
        })?;
        Ok((items, styles))
    })?;
    let (pos_x, pos_y) = r.subblock(3, |r| Ok((r.f64()?, r.f64()?)))?;
    let width = r.read_float(4)?;
    Ok(Text {
        items,
        styles,
        pos_x,
        pos_y,
        width,
    })
}

fn opt<T>(
    r: &mut Reader,
    index: u64,
    kind: u8,
    f: impl FnOnce(&mut Reader) -> Result<T>,
) -> Result<Option<T>> {
    if r.has_tag(index, kind) {
        f(r).map(Some)
    } else {
        Ok(None)
    }
}
