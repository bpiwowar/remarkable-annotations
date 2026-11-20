//! Page sizes of a PDF, read by walking its page tree.
//!
//! Recent PDFs keep page dictionaries in compressed object streams, where a
//! text scan for `/MediaBox` finds nothing. This indexes every object
//! (inflating object streams), then walks Catalog → Pages → Kids in order,
//! with inherited MediaBox/CropBox and indirect references. It is small on
//! purpose: no encryption, no stream predictors. Anything unexpected gives
//! `None`, and the caller falls back.
//!
//! Ported from the TypeScript implementation, including its JavaScript
//! number parsing, so that both give the same sizes.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::bytes::Regex;

use crate::geometry::PageSize;

/// A page's visible box (CropBox ∩ MediaBox).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfBox {
    pub x0: f64,
    pub y0: f64,
    pub width: f64,
    pub height: f64,
}

/// JavaScript's `\s` on Latin-1 text.
const WS: &str = r"[\t\n\x0B\x0C\r \xA0]";

fn re(pattern: &str) -> Regex {
    Regex::new(&format!(
        "(?-u){}",
        pattern.replace(r"\s", WS).replace(r"\d", "[0-9]")
    ))
    .unwrap()
}

macro_rules! static_re {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| re($pattern))
        }
    };
}

static_re!(obj_re, r"(\d+)\s+(\d+)\s+obj\b");
static_re!(length_re, r"/Length\s+(\d+)(?:\s+(\d+)\s+R)?");
static_re!(objstm_re, r"/Type\s*/ObjStm\b");
static_re!(flate_re, r"/FlateDecode\b");
static_re!(n_re, r"/N\s+(\d+)");
static_re!(first_re, r"/First\s+(\d+)");
static_re!(ws_re, r"\s+");
static_re!(array_re, r"\[([^\]]*)\]");
static_re!(refs_re, r"(\d+)\s+\d+\s+R");
static_re!(root_re, r"/Root\s+(\d+)\s+\d+\s+R");
static_re!(has_pages_re, r"/Pages\s+\d+");
static_re!(catalog_re, r"/Type\s*/Catalog\b");
static_re!(pages_ref_re, r"/Pages\s+(\d+)\s+\d+\s+R");
static_re!(kids_re, r"/Kids\s*(\[[^\]]*\]|(\d+)\s+\d+\s+R)");
static_re!(type_page_re, r"/Type\s*/Page");
static_re!(media_box_re, r"/MediaBox\s*(\[[^\]]*\]|(\d+)\s+\d+\s+R)");
static_re!(crop_box_re, r"/CropBox\s*(\[[^\]]*\]|(\d+)\s+\d+\s+R)");
static_re!(
    scan_re,
    r"/MediaBox\s*\[\s*(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)\s*\]"
);

fn is_ws(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ' | 0xA0)
}

fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|b| !is_ws(*b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !is_ws(*b)).map_or(start, |e| e + 1);
    &s[start..end]
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// `parseInt` of decimal digits.
fn int(digits: &[u8]) -> f64 {
    std::str::from_utf8(digits)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

/// `parseInt(s, 10)`: optional sign and leading digits, after whitespace.
fn parse_int(s: &[u8]) -> f64 {
    let s = trim(s);
    let (sign, rest) = match s.first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    let n = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if n == 0 {
        f64::NAN
    } else {
        sign * int(&rest[..n])
    }
}

/// `parseFloat` of a `-?[\d.]+` token: its longest numeric prefix.
fn parse_float(s: &[u8]) -> f64 {
    let mut end = usize::from(s.first() == Some(&b'-'));
    let digits =
        |s: &[u8], from: usize| s[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let int_digits = digits(s, end);
    end += int_digits;
    let mut frac_digits = 0;
    if s.get(end) == Some(&b'.') {
        frac_digits = digits(s, end + 1);
        if int_digits + frac_digits > 0 {
            end += 1 + frac_digits;
        }
    }
    if int_digits + frac_digits == 0 {
        return f64::NAN;
    }
    std::str::from_utf8(&s[..end])
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

/// `Number(token)` for a whitespace-free token.
fn number(token: &[u8]) -> f64 {
    if token.is_empty() {
        return 0.0;
    }
    match token {
        b"Infinity" | b"+Infinity" => return f64::INFINITY,
        b"-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let numeric = token
        .iter()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'));
    if !numeric {
        return f64::NAN;
    }
    std::str::from_utf8(token)
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

/// `s.trim().split(/\s+/).map(Number)`.
fn numbers(s: &[u8]) -> Vec<f64> {
    ws_re().split(trim(s)).map(number).collect()
}

/// `String.prototype.slice(start, end)` on bytes.
fn js_slice(s: &[u8], start: f64, end: f64) -> &[u8] {
    let len = s.len() as f64;
    let clamp = |x: f64| {
        let x = if x.is_nan() { 0.0 } else { x.trunc() };
        if x < 0.0 {
            (len + x).max(0.0)
        } else {
            x.min(len)
        }
    };
    let (a, b) = (clamp(start) as usize, clamp(end) as usize);
    if b <= a {
        &[]
    } else {
        &s[a..b]
    }
}

/// Objects by number, in insertion order (a JavaScript `Map`).
#[derive(Default)]
struct Objects {
    order: Vec<(u64, Vec<u8>)>,
    index: HashMap<u64, usize>,
}

impl Objects {
    fn key(num: f64) -> u64 {
        // SameValueZero: -0 and 0 are the same key.
        if num == 0.0 {
            0f64.to_bits()
        } else {
            num.to_bits()
        }
    }

    fn get(&self, num: f64) -> Option<&[u8]> {
        self.index
            .get(&Self::key(num))
            .map(|&i| self.order[i].1.as_slice())
    }

    fn has(&self, num: f64) -> bool {
        self.index.contains_key(&Self::key(num))
    }

    fn set(&mut self, num: f64, value: Vec<u8>) {
        match self.index.get(&Self::key(num)) {
            Some(&i) => self.order[i].1 = value,
            None => {
                self.index.insert(Self::key(num), self.order.len());
                self.order.push((Self::key(num), value));
            }
        }
    }

    fn values(&self) -> impl Iterator<Item = &[u8]> {
        self.order.iter().map(|(_, v)| v.as_slice())
    }
}

/// fflate's `unzlibSync`, then `inflateSync`.
fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use miniz_oxide::inflate::decompress_to_vec;
    let zlib_ok = data.len() >= 6
        && data[0] & 15 == 8
        && data[0] >> 4 <= 7
        && (u16::from(data[0]) << 8 | u16::from(data[1])) % 31 == 0
        && data[1] & 32 == 0;
    if zlib_ok {
        if let Ok(v) = decompress_to_vec(&data[2..data.len() - 4]) {
            return Some(v);
        }
    }
    decompress_to_vec(data).ok()
}

/// The stream bytes after the `stream` keyword at `at`.
fn stream_body(s: &[u8], dict: &[u8], at: usize, objects: &Objects) -> Option<(usize, usize)> {
    let mut start = at + b"stream".len();
    if s.get(start) == Some(&b'\r') {
        start += 1;
    }
    if s.get(start) == Some(&b'\n') {
        start += 1;
    }
    if let Some(m) = length_re().captures(dict) {
        let mut len = int(&m[1]);
        if m.get(2).is_some() {
            len = objects.get(len).map_or(f64::NAN, parse_int);
        }
        if len.is_finite() && start as f64 + len <= s.len() as f64 {
            return Some((start, start + len as usize));
        }
    }
    let end = find(s, b"endstream", start)?;
    let mut e = end;
    if e > 0 && s[e - 1] == b'\n' {
        e -= 1;
    }
    if e > 0 && s[e - 1] == b'\r' {
        e -= 1;
    }
    // JavaScript slices an inverted range as empty.
    Some((start, e.max(start)))
}

/// Every object (number → text), with object streams expanded.
fn index_objects(s: &[u8]) -> Objects {
    let mut objects = Objects::default();
    let mut pending: Vec<(Vec<u8>, usize)> = Vec::new();
    let mut pos = 0;
    while let Some(m) = obj_re().captures_at(s, pos) {
        let whole = m.get(0).unwrap();
        let num = int(&m[1]);
        let body_start = whole.end();
        let Some(mut endobj) = find(s, b"endobj", body_start) else {
            break;
        };
        let mut dict = &s[body_start..endobj];
        if let Some(st) = find(s, b"stream", body_start).filter(|&st| st < endobj) {
            dict = &s[body_start..st];
            if let Some(e) = find(s, b"endstream", st).and_then(|es| find(s, b"endobj", es)) {
                endobj = endobj.max(e);
            }
            if objstm_re().is_match(dict) {
                pending.push((dict.to_vec(), st));
            }
        }
        objects.set(num, dict.to_vec());
        pos = endobj + b"endobj".len();
        if pos > s.len() {
            break;
        }
    }
    // Objects defined directly in the file win over compressed copies.
    for (dict, at) in pending {
        let Some((a, b)) = stream_body(s, &dict, at, &objects) else {
            continue;
        };
        let raw = &s[a..b];
        let data = if flate_re().is_match(&dict) {
            match inflate(raw) {
                Some(d) => d,
                None => continue,
            }
        } else {
            raw.to_vec()
        };
        let cap = |r: &Regex| r.captures(&dict).map_or(f64::NAN, |c| int(&c[1]));
        let (n, first) = (cap(n_re()), cap(first_re()));
        if !n.is_finite() || !first.is_finite() {
            continue;
        }
        let nums = numbers(js_slice(&data, 0.0, first));
        let at = |i: f64| {
            if i >= 0.0 && (i as usize) < nums.len() {
                nums[i as usize]
            } else {
                f64::NAN
            }
        };
        let mut i = 0.0;
        while i < n {
            let obj_num = at(2.0 * i);
            let off = first + at(2.0 * i + 1.0);
            let next = if i + 1.0 < n {
                first + at(2.0 * i + 3.0)
            } else {
                data.len() as f64
            };
            if obj_num.is_finite() && off.is_finite() && !objects.has(obj_num) {
                objects.set(obj_num, js_slice(&data, off, next).to_vec());
            }
            i += 1.0;
        }
    }
    objects
}

type Box4 = [f64; 4];

/// A numeric box `[a b c d]`, or a reference to one.
fn read_box(dict: &[u8], key_re: &Regex, objects: &Objects) -> Option<Box4> {
    let m = key_re.captures(dict)?;
    let arr: &[u8] = match m.get(2) {
        Some(r) => objects.get(int(r.as_bytes())).unwrap_or(b""),
        None => m.get(1).unwrap().as_bytes(),
    };
    let inner = array_re().captures(arr)?;
    let nums = numbers(&inner[1]);
    if nums.len() != 4 || nums.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some([
        nums[0].min(nums[2]),
        nums[1].min(nums[3]),
        nums[0].max(nums[2]),
        nums[1].max(nums[3]),
    ])
}

fn is_page(node: &[u8]) -> bool {
    type_page_re()
        .find_iter(node)
        .any(|m| !node.get(m.end()).is_some_and(|b| b.is_ascii_alphabetic()))
}

/// The visible box of every page, in order; `None` when the page tree
/// cannot be read.
pub fn read_page_boxes(s: &[u8]) -> Option<Vec<PdfBox>> {
    if s.is_empty() {
        return None;
    }
    let objects = index_objects(s);
    // The last /Root wins (incremental updates append a newer trailer).
    let mut catalog = root_re()
        .captures_iter(s)
        .last()
        .and_then(|c| objects.get(int(&c[1])));
    if !catalog.is_some_and(|c| has_pages_re().is_match(c)) {
        catalog = objects.values().find(|o| catalog_re().is_match(o));
    }
    let pages = int(&pages_ref_re().captures(catalog?)?[1]);

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(f64, Option<Box4>, Option<Box4>)> = vec![(pages, None, None)];
    while let Some((num, media, crop)) = stack.pop() {
        if !seen.insert(Objects::key(num)) {
            continue;
        }
        let Some(node) = objects.get(num) else {
            continue;
        };
        let media = read_box(node, media_box_re(), &objects).or(media);
        let crop = read_box(node, crop_box_re(), &objects).or(crop);
        if let Some(kids) = kids_re().captures(node) {
            if !is_page(node) {
                let list: &[u8] = match kids.get(2) {
                    Some(r) => objects.get(int(r.as_bytes())).unwrap_or(b""),
                    None => kids.get(1).unwrap().as_bytes(),
                };
                let refs: Vec<f64> = refs_re().captures_iter(list).map(|c| int(&c[1])).collect();
                for k in refs.into_iter().rev() {
                    stack.push((k, media, crop));
                }
                continue;
            }
        }
        let Some(media) = media else {
            continue;
        };
        let b = match crop {
            Some(c) => [
                media[0].max(c[0]),
                media[1].max(c[1]),
                media[2].min(c[2]),
                media[3].min(c[3]),
            ],
            None => media,
        };
        if b[2] <= b[0] || b[3] <= b[1] {
            continue;
        }
        out.push(PdfBox {
            x0: b[0],
            y0: b[1],
            width: b[2] - b[0],
            height: b[3] - b[1],
        });
    }
    (!out.is_empty()).then_some(out)
}

/// Every plain-text `/MediaBox [a b c d]`, in file order.
pub(crate) fn scan_media_boxes(s: &[u8]) -> Vec<PageSize> {
    scan_re()
        .captures_iter(s)
        .filter_map(|m| {
            let v: Vec<f64> = (1..=4).map(|i| parse_float(&m[i])).collect();
            let (width, height) = ((v[2] - v[0]).abs(), (v[3] - v[1]).abs());
            (width > 0.0 && height > 0.0).then_some(PageSize {
                width,
                height,
                x0: None,
                y0: None,
            })
        })
        .collect()
}
