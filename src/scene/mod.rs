//! reMarkable `.rm` version 6 scene files: a 43-byte header, then
//! length-prefixed, tagged blocks forming a CRDT scene tree.
//!
//! [`Scene::parse`] reads every block into [`Block`]s and
//! [`Scene::serialize`] writes them back. Writing back an unmodified scene
//! reproduces the input byte for byte: blocks that fail to parse are kept
//! raw ([`BlockBody::Unknown`]), each block keeps its unread tail, and the
//! few wire details that carry no meaning (string ASCII flags, optional
//! fields, item values of unexpected types) are kept as read.
//!
//! The format and the block model follow rmapi-js's `rm6` module (MIT), itself
//! modelled on the Python `rmscene` library. [`view`] gives the same JSON shape
//! as rmapi-js's parsed blocks, which the tests compare against.

mod io;
mod parse;
pub mod view;
mod write;

pub use io::Error;

/// The v6 header, padded with spaces to [`HEADER_LENGTH`] bytes.
pub const V6_HEADER: &str = "reMarkable .lines file, version=6";
/// Length of the file header.
pub const HEADER_LENGTH: usize = 43;

/// A CRDT identifier: `(author, counter)`, unique across replicas.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct CrdtId {
    /// The author (replica) that minted the id; the page's author table maps
    /// it to a uuid.
    pub author: u8,
    /// The author's counter.
    pub counter: u64,
}

impl CrdtId {
    pub const fn new(author: u8, counter: u64) -> Self {
        Self { author, counter }
    }
}

/// The scene-tree root node id.
pub const ROOT_ID: CrdtId = CrdtId::new(0, 1);
/// The CRDT sequence end marker (unset id).
pub const END_MARKER: CrdtId = CrdtId::new(0, 0);

/// A last-writer-wins value.
#[derive(Debug, Clone, PartialEq)]
pub struct Lww<T> {
    pub timestamp: CrdtId,
    pub value: T,
}

/// A string as stored on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct RmString {
    pub value: String,
    /// The "is ASCII" flag; the tablet writes 1 even for non-ASCII text.
    pub ascii_flag: u8,
}

impl From<&str> for RmString {
    fn from(value: &str) -> Self {
        Self {
            value: value.to_owned(),
            ascii_flag: 1,
        }
    }
}

/// A stroke point in the version 2 encoding (current tablets).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointV2 {
    pub x: f32,
    pub y: f32,
    pub speed: u16,
    pub width: u16,
    /// 0–255 maps onto 0–2π.
    pub direction: u8,
    pub pressure: u8,
}

/// A stroke point in the version 1 encoding, as raw floats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointV1 {
    pub x: f32,
    pub y: f32,
    /// Stored as speed / 4.
    pub speed: f32,
    /// In radians.
    pub direction: f32,
    /// Stored as width / 4.
    pub width: f32,
    /// 0–1.
    pub pressure: f32,
}

/// A stroke's points; the block's current version selects the encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum Points {
    V1(Vec<PointV1>),
    V2(Vec<PointV2>),
}

impl Points {
    pub fn len(&self) -> usize {
        match self {
            Points::V1(p) => p.len(),
            Points::V2(p) => p.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Pen code.
    pub tool: u32,
    /// Colour code.
    pub color: u32,
    pub thickness_scale: f64,
    pub starting_length: f32,
    pub points: Points,
    pub timestamp_id: Option<CrdtId>,
    pub move_id: Option<CrdtId>,
    /// Packed `0xAARRGGBB` colour (highlighters, recent shapes).
    pub color_rgba: Option<u32>,
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A highlighted run of the underlying (PDF) text.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRange {
    pub start: Option<u32>,
    /// The explicit length, when the file has one.
    pub length: Option<u32>,
    pub color: u32,
    pub text: RmString,
    pub rectangles: Vec<Rect>,
    /// Packed `0xAARRGGBB` colour.
    pub color_rgba: Option<u32>,
}

impl GlyphRange {
    /// The explicit length, or the text's length in UTF-16 code units.
    pub fn len(&self) -> u32 {
        self.length
            .unwrap_or_else(|| self.text.value.encode_utf16().count() as u32)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A scene item's value: parsed, or kept raw when its type is unexpected or
/// not modelled (text items, tombstones).
#[derive(Debug, Clone, PartialEq)]
pub enum ItemValue<V> {
    Value(V),
    Raw { item_type: u8, data: Vec<u8> },
}

impl<V> ItemValue<V> {
    pub fn value(&self) -> Option<&V> {
        match self {
            ItemValue::Value(v) => Some(v),
            ItemValue::Raw { .. } => None,
        }
    }
}

/// The CRDT-sequence envelope shared by scene items.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneItem<V> {
    pub item_id: CrdtId,
    pub left_id: CrdtId,
    pub right_id: CrdtId,
    pub deleted_length: u32,
    pub value: Option<ItemValue<V>>,
}

/// An item block: the parent group and the item.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemBlock<V> {
    pub parent_id: CrdtId,
    pub item: SceneItem<V>,
}

/// Value of an item we do not model (text items, tombstones).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opaque {}

/// The anchor of a tree node; its four values always come together.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub id: Lww<CrdtId>,
    pub kind: Lww<u8>,
    pub threshold: Lww<f32>,
    pub origin_x: Lww<f32>,
}

/// A text run, or an inline format code.
#[derive(Debug, Clone, PartialEq)]
pub struct TextValue {
    pub text: RmString,
    /// Present for format codes (the text is then empty).
    pub format: Option<u32>,
}

/// An item of the root text's CRDT sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct TextItem {
    pub item_id: CrdtId,
    pub left_id: CrdtId,
    pub right_id: CrdtId,
    pub deleted_length: u32,
    pub value: Option<TextValue>,
}

/// A paragraph style attached to a character.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub char_id: CrdtId,
    pub timestamp: CrdtId,
    /// The byte before the style (always 17 in files seen).
    pub tag: u8,
    pub style: u8,
}

/// The page's document text (typed text, not PDF text).
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub items: Vec<TextItem>,
    pub styles: Vec<TextStyle>,
    pub pos_x: f64,
    pub pos_y: f64,
    pub width: f32,
}

/// An entry of the author table: a page-local author id and its uuid.
#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub id: u16,
    /// The uuid in wire order (little-endian fields, as `uuid.bytes_le`).
    pub uuid_le: [u8; 16],
}

/// Byte order of a little-endian uuid; the permutation is its own inverse.
const UUID_LE_ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];

impl Author {
    /// The uuid as `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
    pub fn uuid(&self) -> String {
        let b: Vec<u8> = UUID_LE_ORDER.iter().map(|&i| self.uuid_le[i]).collect();
        let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
    }

    /// Builds an entry from a uuid string (hyphens optional).
    pub fn from_uuid(id: u16, uuid: &str) -> Result<Self, Error> {
        let hex: String = uuid.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 {
            return Err(Error::Invalid(format!("bad uuid {uuid:?}")));
        }
        let mut big = [0u8; 16];
        for (i, b) in big.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                .map_err(|_| Error::Invalid(format!("bad uuid {uuid:?}")))?;
        }
        let mut uuid_le = [0u8; 16];
        for (i, b) in uuid_le.iter_mut().enumerate() {
            *b = big[UUID_LE_ORDER[i]];
        }
        Ok(Self { id, uuid_le })
    }
}

/// The parsed body of a block.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockBody {
    /// `0x00`
    MigrationInfo {
        migration_id: CrdtId,
        is_device: bool,
    },
    /// `0x01`: a node/parent edge of the scene tree.
    SceneTree {
        tree_id: CrdtId,
        node_id: CrdtId,
        is_update: bool,
        parent_id: CrdtId,
    },
    /// `0x02`: a group's metadata (layer name and visibility).
    TreeNode {
        node_id: CrdtId,
        label: Lww<RmString>,
        visible: Lww<bool>,
        anchor: Option<Anchor>,
    },
    /// `0x03`: a text highlight.
    SceneGlyphItem(ItemBlock<GlyphRange>),
    /// `0x04`: attaches a child node to a group.
    SceneGroupItem(ItemBlock<CrdtId>),
    /// `0x05`: a stroke.
    SceneLineItem(ItemBlock<Line>),
    /// `0x06`: a text item (value kept raw).
    SceneTextItem(ItemBlock<Opaque>),
    /// `0x07`: the page's document text.
    RootText { block_id: CrdtId, text: Text },
    /// `0x08`: a deleted item.
    SceneTombstone(ItemBlock<Opaque>),
    /// `0x09`: the author table.
    AuthorIds { authors: Vec<Author> },
    /// `0x0a`
    PageInfo {
        loads_count: u32,
        merges_count: u32,
        text_chars_count: u32,
        text_lines_count: u32,
        type_folio_use_count: Option<u32>,
    },
    /// `0x0d`
    SceneInfo {
        current_layer: Lww<CrdtId>,
        background_visible: Option<Lww<bool>>,
        root_document_visible: Option<Lww<bool>>,
        paper_size: Option<(u32, u32)>,
    },
    /// A block that is not parsed (unknown type, or failed to parse).
    Unknown {
        block_type: u8,
        data: Vec<u8>,
        /// The declared length, when it overran the file.
        declared_length: Option<u32>,
    },
}

impl BlockBody {
    /// The wire block type.
    pub fn block_type(&self) -> u8 {
        match self {
            BlockBody::MigrationInfo { .. } => 0x00,
            BlockBody::SceneTree { .. } => 0x01,
            BlockBody::TreeNode { .. } => 0x02,
            BlockBody::SceneGlyphItem(_) => 0x03,
            BlockBody::SceneGroupItem(_) => 0x04,
            BlockBody::SceneLineItem(_) => 0x05,
            BlockBody::SceneTextItem(_) => 0x06,
            BlockBody::RootText { .. } => 0x07,
            BlockBody::SceneTombstone(_) => 0x08,
            BlockBody::AuthorIds { .. } => 0x09,
            BlockBody::PageInfo { .. } => 0x0a,
            BlockBody::SceneInfo { .. } => 0x0d,
            BlockBody::Unknown { block_type, .. } => *block_type,
        }
    }
}

/// A block: its header bytes, body and unread tail.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The header byte after the length (0 in files seen).
    pub reserved: u8,
    pub min_version: u8,
    /// Selects the point encoding of strokes (1 or 2).
    pub current_version: u8,
    pub body: BlockBody,
    /// The bytes after the parsed body, kept for round trips.
    pub extra_data: Vec<u8>,
}

/// A parsed v6 scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// The header as read (the v6 signature, padded).
    pub header: [u8; HEADER_LENGTH],
    pub blocks: Vec<Block>,
    /// Bytes after the last block too short to hold a block header.
    pub trailing: Vec<u8>,
}

impl Scene {
    /// Parses a v6 `.rm` file.
    pub fn parse(data: &[u8]) -> Result<Scene, Error> {
        parse::parse_scene(data)
    }

    /// Serializes the scene back to `.rm` bytes.
    pub fn serialize(&self) -> Vec<u8> {
        write::serialize_scene(self)
    }

    /// The standard v6 header.
    pub fn default_header() -> [u8; HEADER_LENGTH] {
        let mut h = [b' '; HEADER_LENGTH];
        h[..V6_HEADER.len()].copy_from_slice(V6_HEADER.as_bytes());
        h
    }
}
