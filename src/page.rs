//! Annotations of a page: the strokes and text highlights of a `.rm` v6 page,
//! and rewriting a page with new ones.
//!
//! Coordinates are the reMarkable page's: x from the page centre, y from the
//! top, in device pixels ([`crate::geometry`] maps them to PDF points).
//!
//! Every scene item needs a unique id and a left neighbour in its layer's
//! CRDT sequence. [`rebuild_page`] mints ids under an author registered in
//! the page, with counters above every counter already used (the tablet drops
//! items whose counter is not), and chains new items after the layer's last
//! item. These rules were checked on a tablet.

use serde::{Deserialize, Serialize};

use crate::js::{max, round as js_round};

use crate::scene::{
    Author, Block, BlockBody, CrdtId, Error, GlyphRange, ItemBlock, ItemValue, Line, Lww, PointV2,
    Points, Rect, RmString, Scene, SceneItem, END_MARKER,
};

/// A point in page coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A colour as `[r, g, b, a]`, 0–255.
pub type Rgba = [u8; 4];

/// A pen or highlighter stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    /// Pen code.
    pub tool: u32,
    /// Colour code.
    pub color_index: u32,
    /// Explicit colour, when present (highlighters, recent pens).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgba: Option<Rgba>,
    /// The stroke's thickness scale (a pen setting).
    pub thickness: f64,
    /// The median point width, rounded (device units).
    pub point_width: f64,
    pub is_highlighter: bool,
    pub points: Vec<Point>,
    /// The scene item ids (set when read from a page).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<CrdtId>,
}

/// A text highlight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub text: String,
    pub color_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgba: Option<Rgba>,
    pub rects: Vec<Rect>,
    /// The scene item ids (set when read from a page).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<CrdtId>,
}

/// The annotations of a page, and what is needed to add items to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub strokes: Vec<Stroke>,
    pub highlights: Vec<Highlight>,
    /// The layer of the first stroke or highlight: where new items go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<CrdtId>,
    /// The last item of that layer, in file order: new items chain after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_item_id: Option<CrdtId>,
    /// The highest author id seen (items and author table).
    pub max_author: u32,
    /// The highest counter used by any scene item.
    pub max_counter: u64,
    /// The author table: page-local author id and uuid.
    pub authors: Vec<(u16, String)>,
}

/// Pen codes of the highlighters.
const HIGHLIGHTER_TOOLS: [u32; 2] = [5, 18];

/// The uuid of the author of pages created from scratch (author tables are
/// local to a page).
pub const AUTHOR_UUID: &str = "79736d72-636e-1000-8000-000000000001";

/// `[r, g, b, a]` from a packed `0xAARRGGBB` colour.
pub fn unpack_rgba(p: u32) -> Rgba {
    [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]
}

/// The packed `0xAARRGGBB` colour.
pub fn pack_rgba([r, g, b, a]: Rgba) -> u32 {
    u32::from(b) | u32::from(g) << 8 | u32::from(r) << 16 | u32::from(a) << 24
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    let m = xs.len() / 2;
    if xs.len() % 2 == 1 {
        xs[m]
    } else {
        (xs[m - 1] + xs[m]) / 2.0
    }
}

/// Sets `uuid` for author `id` in a table, keeping the table's order.
fn set_author(table: &mut Vec<(u16, String)>, id: u16, uuid: String) {
    match table.iter_mut().find(|(a, _)| *a == id) {
        Some(entry) => entry.1 = uuid,
        None => table.push((id, uuid)),
    }
}

/// The parent and item envelope fields of an item block, if it is one.
fn item_ids(body: &BlockBody) -> Option<(CrdtId, CrdtId, u32)> {
    let f = |p: CrdtId, i: CrdtId, d: u32| Some((p, i, d));
    match body {
        BlockBody::SceneGlyphItem(b) => f(b.parent_id, b.item.item_id, b.item.deleted_length),
        BlockBody::SceneGroupItem(b) => f(b.parent_id, b.item.item_id, b.item.deleted_length),
        BlockBody::SceneLineItem(b) => f(b.parent_id, b.item.item_id, b.item.deleted_length),
        BlockBody::SceneTextItem(b) | BlockBody::SceneTombstone(b) => {
            f(b.parent_id, b.item.item_id, b.item.deleted_length)
        }
        _ => None,
    }
}

impl Page {
    /// The annotations of a parsed scene.
    pub fn from_scene(scene: &Scene) -> Page {
        let mut page = Page {
            strokes: Vec::new(),
            highlights: Vec::new(),
            layer_id: None,
            last_item_id: None,
            max_author: 0,
            max_counter: 0,
            authors: Vec::new(),
        };
        for block in &scene.blocks {
            if let BlockBody::AuthorIds { authors } = &block.body {
                for a in authors {
                    set_author(&mut page.authors, a.id, a.uuid());
                }
                continue;
            }
            let Some((parent, item, deleted)) = item_ids(&block.body) else {
                continue;
            };
            // Every scene item (tombstones and groups included) uses a counter.
            page.max_counter = page.max_counter.max(item.counter);
            if !matches!(
                block.body,
                BlockBody::SceneLineItem(_) | BlockBody::SceneGlyphItem(_)
            ) {
                continue;
            }
            let layer = *page.layer_id.get_or_insert(parent);
            if parent == layer {
                page.last_item_id = Some(item);
            }
            page.max_author = page
                .max_author
                .max(item.author.into())
                .max(parent.author.into());
            page.max_counter = page.max_counter.max(parent.counter);
            if deleted != 0 {
                continue;
            }
            match &block.body {
                BlockBody::SceneLineItem(b) => {
                    if let Some(s) = stroke_of(b, item) {
                        page.strokes.push(s);
                    }
                }
                BlockBody::SceneGlyphItem(b) => {
                    if let Some(h) = highlight_of(b, item) {
                        page.highlights.push(h);
                    }
                }
                _ => unreachable!(),
            }
        }
        for (id, _) in &page.authors {
            page.max_author = page.max_author.max((*id).into());
        }
        page
    }
}

fn stroke_of(b: &ItemBlock<Line>, item: CrdtId) -> Option<Stroke> {
    let line = b.item.value.as_ref()?.value()?;
    let (points, widths): (Vec<Point>, Vec<f64>) = match &line.points {
        Points::V2(ps) => ps
            .iter()
            .map(|p| {
                (
                    Point {
                        x: p.x.into(),
                        y: p.y.into(),
                    },
                    f64::from(p.width),
                )
            })
            .unzip(),
        Points::V1(ps) => ps
            .iter()
            .map(|p| {
                (
                    Point {
                        x: p.x.into(),
                        y: p.y.into(),
                    },
                    f64::from(p.width) * 4.0,
                )
            })
            .unzip(),
    };
    if points.is_empty() {
        return None;
    }
    Some(Stroke {
        tool: line.tool,
        color_index: line.color,
        rgba: line.color_rgba.map(unpack_rgba),
        thickness: line.thickness_scale,
        point_width: median(widths.into_iter().map(js_round).collect()),
        is_highlighter: HIGHLIGHTER_TOOLS.contains(&line.tool),
        points,
        ids: vec![item],
    })
}

fn highlight_of(b: &ItemBlock<GlyphRange>, item: CrdtId) -> Option<Highlight> {
    let glyph = b.item.value.as_ref()?.value()?;
    if glyph.rectangles.is_empty() {
        return None;
    }
    Some(Highlight {
        text: glyph.text.value.clone(),
        color_index: glyph.color,
        rgba: glyph.color_rgba.map(unpack_rgba),
        rects: glyph.rectangles.clone(),
        ids: vec![item],
    })
}

/// Parses a `.rm` v6 page into its annotations.
pub fn parse_page(bytes: &[u8]) -> Result<Page, Error> {
    Ok(Page::from_scene(&Scene::parse(bytes)?))
}

/// Where and how [`rebuild_page`] adds items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageUpdate {
    /// The layer to add items to.
    pub layer_id: CrdtId,
    /// The item to chain new items after (none: the start of the layer).
    #[serde(default)]
    pub last_item_id: Option<CrdtId>,
    /// An author id registered in the page, for new item ids.
    pub author: u8,
    /// The first counter for new item ids: above every counter in the page.
    pub start_counter: u64,
    /// The author of each added item (highlights, then strokes); `author`
    /// past the end.
    #[serde(default)]
    pub authors: Vec<u8>,
    /// Authors to add to (or update in) the page's author table.
    #[serde(default)]
    pub register: Vec<(u16, String)>,
}

impl PageUpdate {
    /// Adding items to `page` after its existing ones, under its highest
    /// author; `None` when the page has no stroke or highlight (no layer).
    pub fn after(page: &Page) -> Option<PageUpdate> {
        Some(PageUpdate {
            layer_id: page.layer_id?,
            last_item_id: page.last_item_id,
            author: u8::try_from(page.max_author).unwrap_or(u8::MAX),
            start_counter: page.max_counter + 1,
            authors: Vec::new(),
            register: Vec::new(),
        })
    }
}

/// Scene items: what [`rebuild_page`] adds and removes, and
/// [`split_structure`] drops. Everything else is the page's structure.
fn is_item(body: &BlockBody) -> bool {
    matches!(
        body,
        BlockBody::SceneLineItem(_)
            | BlockBody::SceneGlyphItem(_)
            | BlockBody::SceneTombstone(_)
            | BlockBody::Unknown {
                block_type: 0x03 | 0x05 | 0x08,
                ..
            }
    )
}

fn block(min_version: u8, current_version: u8, body: BlockBody) -> Block {
    Block {
        reserved: 0,
        min_version,
        current_version,
        body,
        extra_data: Vec::new(),
    }
}

fn new_item<V>(item_id: CrdtId, left_id: CrdtId, value: V) -> SceneItem<V> {
    SceneItem {
        item_id,
        left_id,
        right_id: END_MARKER,
        deleted_length: 0,
        value: Some(ItemValue::Value(value)),
    }
}

/// Rewrites a page: drops the items whose id is in `remove`, keeps every
/// other block as is, then appends `highlights` and `strokes` (in that
/// order). Returns the new page and the ids given to the added items.
pub fn rebuild_page(
    existing: &[u8],
    strokes: &[Stroke],
    highlights: &[Highlight],
    remove: &[CrdtId],
    update: &PageUpdate,
) -> Result<(Vec<u8>, Vec<CrdtId>), Error> {
    let mut scene = Scene::parse(existing)?;
    scene.blocks.retain(|b| {
        !(is_item(&b.body) && item_ids(&b.body).is_some_and(|(_, id, _)| remove.contains(&id)))
    });

    if !update.register.is_empty() {
        let at = match scene
            .blocks
            .iter()
            .position(|b| matches!(b.body, BlockBody::AuthorIds { .. }))
        {
            Some(at) => at,
            None => {
                scene.blocks.insert(
                    0,
                    block(
                        1,
                        1,
                        BlockBody::AuthorIds {
                            authors: Vec::new(),
                        },
                    ),
                );
                0
            }
        };
        let BlockBody::AuthorIds { authors } = &mut scene.blocks[at].body else {
            unreachable!()
        };
        for (id, uuid) in &update.register {
            let entry = Author::from_uuid(*id, uuid)?;
            match authors.iter_mut().find(|a| a.id == *id) {
                Some(a) => *a = entry,
                None => authors.push(entry),
            }
        }
    }

    let mut left = update.last_item_id.unwrap_or(END_MARKER);
    let mut counter = update.start_counter;
    let mut ids: Vec<CrdtId> = Vec::new();
    let mut next_item = |ids: &mut Vec<CrdtId>| {
        let author = update
            .authors
            .get(ids.len())
            .copied()
            .unwrap_or(update.author);
        let item_id = CrdtId::new(author, counter);
        counter += 1;
        ids.push(item_id);
        let left_id = std::mem::replace(&mut left, item_id);
        (item_id, left_id)
    };

    for h in highlights {
        let (item_id, left_id) = next_item(&mut ids);
        let glyph = GlyphRange {
            start: Some(0),
            length: Some(h.text.encode_utf16().count() as u32),
            color: h.color_index,
            text: RmString::from(h.text.as_str()),
            rectangles: h.rects.clone(),
            color_rgba: h.rgba.map(pack_rgba),
        };
        let item = new_item(item_id, left_id, glyph);
        scene.blocks.push(block(
            0,
            1,
            BlockBody::SceneGlyphItem(ItemBlock {
                parent_id: update.layer_id,
                item,
            }),
        ));
    }
    for s in strokes {
        let (item_id, left_id) = next_item(&mut ids);
        let w = if s.point_width == 0.0 || s.point_width.is_nan() {
            16.0
        } else {
            s.point_width
        };
        let width = max(1.0, js_round(w)) as i64;
        let line = Line {
            tool: s.tool,
            color: s.color_index,
            thickness_scale: if s.thickness == 0.0 || s.thickness.is_nan() {
                2.0
            } else {
                s.thickness
            },
            starting_length: 0.0,
            points: Points::V2(
                s.points
                    .iter()
                    .map(|p| PointV2 {
                        x: p.x as f32,
                        y: p.y as f32,
                        speed: 0,
                        width: width.rem_euclid(1 << 16) as u16,
                        direction: 0,
                        pressure: 0,
                    })
                    .collect(),
            ),
            timestamp_id: Some(CrdtId::new(0, 1)),
            move_id: None,
            color_rgba: s.rgba.map(pack_rgba),
        };
        let item = new_item(item_id, left_id, line);
        scene.blocks.push(block(
            2,
            2,
            BlockBody::SceneLineItem(ItemBlock {
                parent_id: update.layer_id,
                item,
            }),
        ));
    }
    Ok((scene.serialize(), ids))
}

/// A copy of a page without its scene items: its structure (author table,
/// page info, scene tree, layers), as a template for a page with no `.rm`.
pub fn split_structure(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut scene = Scene::parse(bytes)?;
    scene.blocks.retain(|b| !is_item(&b.body));
    Ok(scene.serialize())
}

/// A page structure to add items to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Structure {
    /// The page, without items.
    pub bytes: Vec<u8>,
    /// Where to add items.
    pub update: PageUpdate,
    /// The page's author table.
    pub authors: Vec<(u16, String)>,
}

/// A blank page the tablet accepts: author table, migration and page info,
/// scene tree, root group and one layer (rmscene's `simple_text_document`
/// without the text).
pub fn blank_structure(author: u8) -> Structure {
    let id = CrdtId::new;
    let lww = |timestamp, value| Lww { timestamp, value };
    let author_entry = Author::from_uuid(author.into(), AUTHOR_UUID).unwrap();
    let mut migration = block(
        1,
        1,
        BlockBody::MigrationInfo {
            migration_id: id(1, 1),
            is_device: true,
        },
    );
    migration.extra_data = vec![0x31, 0]; // field 3: false
    let blocks = vec![
        block(
            1,
            1,
            BlockBody::AuthorIds {
                authors: vec![author_entry],
            },
        ),
        migration,
        block(
            0,
            1,
            BlockBody::PageInfo {
                loads_count: 1,
                merges_count: 0,
                text_chars_count: 0,
                text_lines_count: 0,
                type_folio_use_count: Some(0),
            },
        ),
        // The layer's subtree, under the root group.
        block(
            1,
            1,
            BlockBody::SceneTree {
                tree_id: id(0, 11),
                node_id: id(0, 0),
                is_update: true,
                parent_id: id(0, 1),
            },
        ),
        block(
            1,
            2,
            BlockBody::TreeNode {
                node_id: id(0, 1),
                label: lww(id(0, 0), RmString::from("")),
                visible: Lww {
                    timestamp: id(0, 0),
                    value: true,
                },
                anchor: None,
            },
        ),
        block(
            1,
            2,
            BlockBody::TreeNode {
                node_id: id(0, 11),
                label: lww(id(0, 12), RmString::from("Layer 1")),
                visible: Lww {
                    timestamp: id(0, 0),
                    value: true,
                },
                anchor: None,
            },
        ),
        // The layer, as a child of the root.
        block(
            1,
            1,
            BlockBody::SceneGroupItem(ItemBlock {
                parent_id: id(0, 1),
                item: SceneItem {
                    item_id: id(0, 13),
                    left_id: END_MARKER,
                    right_id: END_MARKER,
                    deleted_length: 0,
                    value: Some(ItemValue::Value(id(0, 11))),
                },
            }),
        ),
    ];
    let scene = Scene {
        header: Scene::default_header(),
        blocks,
        trailing: Vec::new(),
    };
    Structure {
        bytes: scene.serialize(),
        update: PageUpdate {
            layer_id: id(0, 11),
            last_item_id: None,
            author,
            start_counter: 20,
            authors: Vec::new(),
            register: Vec::new(),
        },
        authors: vec![(author.into(), AUTHOR_UUID.to_owned())],
    }
}
