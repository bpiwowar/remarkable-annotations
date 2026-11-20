//! What the cloud client needs from JavaScript to write the same bytes as
//! rmapi-js: `JSON.stringify` (key order, number formatting), base64, CRC32C,
//! SHA-256 and `localeCompare` ordering of file names.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Whether `key` is an array index, which JavaScript objects order first
/// (ascending), before the other keys in insertion order.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|&n| n < u64::from(u32::MAX))
        .map(|n| n as u32)
}

/// The keys of an object in JavaScript property order.
pub(crate) fn js_key_order(map: &Map<String, Value>) -> Vec<&String> {
    let mut indices: Vec<(u32, &String)> = map
        .keys()
        .filter_map(|k| array_index(k).map(|i| (i, k)))
        .collect();
    indices.sort();
    let mut keys: Vec<&String> = indices.into_iter().map(|(_, k)| k).collect();
    keys.extend(map.keys().filter(|k| array_index(k).is_none()));
    keys
}

/// `Number.prototype.toString()`: the shortest digits that round-trip and,
/// among those, the closest to the exact value.
pub(crate) fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x == 0.0 {
        return "0".into();
    }
    // The number of significant digits of the shortest representation
    let shortest = format!("{:e}", x.abs());
    let (m, _) = shortest.split_once('e').unwrap();
    let k = m.chars().filter(char::is_ascii_digit).count();
    // Correctly rounded to k digits: the closest k-digit decimal
    let s = format!("{:.*e}", k - 1, x.abs());
    let (m, e) = s.split_once('e').unwrap();
    let digits: String = m.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len() as i32;
    let n = e.parse::<i32>().unwrap() + 1; // decimal point position
    let sign = if x < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let exp = n - 1;
        let exp = if exp < 0 {
            format!("-{}", -exp)
        } else {
            format!("+{exp}")
        };
        if k == 1 {
            format!("{digits}e{exp}")
        } else {
            format!("{}.{}e{exp}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

fn number_json(n: &serde_json::Number) -> String {
    let x = n.as_f64().unwrap_or(f64::NAN);
    if x.is_finite() {
        number_to_string(x)
    } else {
        "null".into()
    }
}

fn string_json(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_json(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_json(n)),
        Value::String(s) => string_json(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, k) in js_key_order(m).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string_json(k, out);
                out.push(':');
                write_json(&m[k], out);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(v)`. Values JavaScript would leave out (`undefined`) are
/// absent from `v`; non-finite numbers cannot occur in a `Value`.
pub(crate) fn stringify(v: &Value) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

/// `JSON.parse` of UTF-8 text decoded like `TextDecoder` (invalid bytes
/// replaced), keeping key order.
pub(crate) fn parse(bytes: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))
}

/// A JavaScript number as a JSON value (integers stay integers).
pub(crate) fn num(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        if x >= 0.0 {
            Value::from(x as u64)
        } else {
            Value::from(x as i64)
        }
    } else {
        serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
    }
}

/// `Number.isSafeInteger`.
pub(crate) fn is_safe_integer(x: f64) -> bool {
    x.fract() == 0.0 && x.abs() <= 9_007_199_254_740_991.0
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64, padded.
pub(crate) fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes base64url (or standard base64), ignoring padding and an
/// incomplete last chunk's extra bits, like `Uint8Array.fromBase64` with
/// `lastChunkHandling: "loose"`.
pub(crate) fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\n' | b'\x0C' | b'\r' => continue,
            _ => return None,
        };
        bits = bits << 6 | u32::from(v);
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
            bits &= (1 << nbits) - 1;
        }
    }
    Some(out)
}

/// CRC-32C (Castagnoli), as the `x-goog-hash` header wants it.
pub(crate) fn crc32c(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0x82F6_3B78 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut crc = !0u32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The SHA-256 of `data`, in hex.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Primary collation weight of an ASCII character in the Unicode root
/// collation (what `localeCompare` uses): spaces and punctuation, then
/// symbols, digits, then letters (case-insensitive).
fn primary(c: char) -> (u8, u32) {
    // Root collation order of ASCII punctuation and symbols.
    const ORDER: &str = "\t\n\x0B\x0C\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    if let Some(i) = ORDER.find(c) {
        (0, i as u32)
    } else if c.is_ascii_digit() {
        (1, c as u32)
    } else if c.is_ascii_alphabetic() {
        (2, c.to_ascii_lowercase() as u32)
    } else {
        (3, c as u32)
    }
}

/// `a.localeCompare(b)` for the ASCII file names of an index (ids, `.`, `/`,
/// `-`): primary weights first, then lowercase before uppercase, then code
/// points. Not a full collation for other text.
pub(crate) fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<(u8, u32)> = a.chars().map(primary).collect();
    let pb: Vec<(u8, u32)> = b.chars().map(primary).collect();
    pa.cmp(&pb)
        .then_with(|| {
            let ta = a.chars().map(|c| c.is_ascii_uppercase());
            let tb = b.chars().map(|c| c.is_ascii_uppercase());
            ta.cmp(tb)
        })
        .then_with(|| a.cmp(b))
}

/// `Number(s)` for a string.
pub(crate) fn js_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = t.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).map_or(f64::NAN, |n| n as f64);
        }
    }
    if t.bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        t.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_like_javascript() {
        for (x, s) in [
            (1.0, "1"),
            (0.1, "0.1"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (1.5e-7, "1.5e-7"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-6, "0.000001"),
            (2.5, "2.5"),
            (-30.081863403320312, "-30.081863403320312"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (123.456, "123.456"),
            (1e20, "100000000000000000000"),
        ] {
            assert_eq!(number_to_string(x), s);
        }
    }

    #[test]
    fn stringify_like_javascript() {
        let v: Value =
            serde_json::from_str(r#"{"b":1.0,"2":"x","a":[true,null,"\u0001é"],"1":{}}"#).unwrap();
        assert_eq!(
            stringify(&v),
            r#"{"1":{},"2":"x","b":1,"a":[true,null,"\u0001é"]}"#
        );
    }

    #[test]
    fn checksums() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(base64(&[0xE3, 0x06, 0x92, 0x83]), "4waSgw==");
        assert_eq!(base64url_decode("eyJhIjoxfQ").unwrap(), br#"{"a":1}"#);
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn file_name_order() {
        let mut v = vec![
            "a.metadata",
            "a/b.rm",
            "a.content",
            "a-b",
            "a.pdf",
            "A.x",
            "a0",
            "aa",
        ];
        v.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(
            v,
            [
                "a-b",
                "a.content",
                "a.metadata",
                "a.pdf",
                "A.x",
                "a/b.rm",
                "a0",
                "aa"
            ]
        );
    }
}
