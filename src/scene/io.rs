//! Tagged reader and writer for the v6 block stream.

use super::{CrdtId, Lww, RmString};

/// Tag types (the low nibble of a tag).
pub(crate) const TAG_BYTE1: u8 = 0x1;
pub(crate) const TAG_BYTE4: u8 = 0x4;
pub(crate) const TAG_BYTE8: u8 = 0x8;
pub(crate) const TAG_LENGTH4: u8 = 0xc;
pub(crate) const TAG_ID: u8 = 0xf;

/// A parse error.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data at offset {0}")]
    Eof(usize),
    #[error("unexpected tag {found} at offset {offset} (wanted {index}/{kind})")]
    Tag {
        offset: usize,
        found: u64,
        index: u64,
        kind: u8,
    },
    #[error("{0}")]
    Invalid(String),
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// A cursor over the block stream, bounded by the innermost (sub)block.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
    bounds: Vec<usize>,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], offset: usize) -> Self {
        Self {
            data,
            offset,
            bounds: Vec::new(),
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn seek(&mut self, offset: usize) {
        self.offset = offset;
    }

    fn boundary(&self) -> usize {
        *self.bounds.last().unwrap_or(&self.data.len())
    }

    pub fn remaining(&self) -> usize {
        self.boundary().saturating_sub(self.offset)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Eof(self.offset));
        }
        let s = &self.data[self.offset..self.offset + n];
        self.offset += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        Ok(self.take(n)?.to_vec())
    }

    pub fn varuint(&mut self) -> Result<u64> {
        let start = self.offset;
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            if shift >= 64 {
                return Err(Error::Invalid(format!("varuint overflow at {start}")));
            }
            result |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
    }

    pub fn crdt_id(&mut self) -> Result<CrdtId> {
        Ok(CrdtId {
            author: self.u8()?,
            counter: self.varuint()?,
        })
    }

    /// The next tag as `(index, type)`, if any, without consuming it.
    pub fn peek_tag(&mut self) -> Option<(u64, u8)> {
        if self.remaining() == 0 {
            return None;
        }
        let save = self.offset;
        let raw = self.varuint().ok();
        self.offset = save;
        raw.map(|r| (r >> 4, (r & 0xf) as u8))
    }

    pub fn has_tag(&mut self, index: u64, kind: u8) -> bool {
        self.peek_tag() == Some((index, kind))
    }

    fn expect_tag(&mut self, index: u64, kind: u8) -> Result<()> {
        let offset = self.offset;
        let raw = self.varuint()?;
        if raw >> 4 != index || (raw & 0xf) as u8 != kind {
            return Err(Error::Tag {
                offset,
                found: raw,
                index,
                kind,
            });
        }
        Ok(())
    }

    pub fn read_int(&mut self, index: u64) -> Result<u32> {
        self.expect_tag(index, TAG_BYTE4)?;
        self.u32()
    }

    pub fn read_float(&mut self, index: u64) -> Result<f32> {
        self.expect_tag(index, TAG_BYTE4)?;
        self.f32()
    }

    pub fn read_double(&mut self, index: u64) -> Result<f64> {
        self.expect_tag(index, TAG_BYTE8)?;
        self.f64()
    }

    pub fn read_id(&mut self, index: u64) -> Result<CrdtId> {
        self.expect_tag(index, TAG_ID)?;
        self.crdt_id()
    }

    pub fn read_byte(&mut self, index: u64) -> Result<u8> {
        self.expect_tag(index, TAG_BYTE1)?;
        self.u8()
    }

    /// A bool; any byte other than 0 or 1 is an error, so that the block is
    /// kept raw rather than rewritten.
    pub fn read_bool(&mut self, index: u64) -> Result<bool> {
        match self.read_byte(index)? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(Error::Invalid(format!("bool byte {b}"))),
        }
    }

    /// Runs `f` inside a Length4 subblock, which it must read entirely.
    pub fn subblock<T>(&mut self, index: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.expect_tag(index, TAG_LENGTH4)?;
        let length = self.u32()? as usize;
        let end = self.offset + length;
        if end > self.boundary() {
            return Err(Error::Invalid(format!(
                "subblock {index} overruns its parent at {}",
                self.offset
            )));
        }
        self.bounds.push(end);
        let value = f(self);
        self.bounds.pop();
        let value = value?;
        if self.offset != end {
            return Err(Error::Invalid(format!(
                "subblock {index} left {} bytes unread",
                end - self.offset
            )));
        }
        Ok(value)
    }

    /// Runs `f` bounded by `end`; returns its value and the unread tail.
    pub fn bounded<T>(
        &mut self,
        end: usize,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<(T, Vec<u8>)> {
        self.bounds.push(end);
        let value = f(self);
        self.bounds.pop();
        let value = value?;
        let tail = self.data[self.offset..end].to_vec();
        self.offset = end;
        Ok((value, tail))
    }

    pub fn read_lww<T>(
        &mut self,
        index: u64,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<Lww<T>> {
        self.subblock(index, |r| {
            let timestamp = r.read_id(1)?;
            let value = f(r)?;
            Ok(Lww { timestamp, value })
        })
    }

    /// The body of a string subblock: varuint length, ASCII flag, UTF-8.
    pub fn string_body(&mut self) -> Result<RmString> {
        let length = self.varuint()? as usize;
        let ascii_flag = self.u8()?;
        let bytes = self.bytes(length)?;
        let value =
            String::from_utf8(bytes).map_err(|_| Error::Invalid("invalid UTF-8 string".into()))?;
        Ok(RmString { value, ascii_flag })
    }

    pub fn read_string(&mut self, index: u64) -> Result<RmString> {
        self.subblock(index, |r| r.string_body())
    }
}

/// The writing counterpart of [`Reader`].
#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn varuint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.u8(byte);
                return;
            }
            self.u8(byte | 0x80);
        }
    }

    pub fn crdt_id(&mut self, id: CrdtId) {
        self.u8(id.author);
        self.varuint(id.counter);
    }

    pub fn tag(&mut self, index: u64, kind: u8) {
        self.varuint(index * 16 + u64::from(kind));
    }

    pub fn write_int(&mut self, index: u64, v: u32) {
        self.tag(index, TAG_BYTE4);
        self.u32(v);
    }

    pub fn write_float(&mut self, index: u64, v: f32) {
        self.tag(index, TAG_BYTE4);
        self.f32(v);
    }

    pub fn write_double(&mut self, index: u64, v: f64) {
        self.tag(index, TAG_BYTE8);
        self.f64(v);
    }

    pub fn write_id(&mut self, index: u64, id: CrdtId) {
        self.tag(index, TAG_ID);
        self.crdt_id(id);
    }

    pub fn write_byte(&mut self, index: u64, v: u8) {
        self.tag(index, TAG_BYTE1);
        self.u8(v);
    }

    pub fn write_bool(&mut self, index: u64, v: bool) {
        self.write_byte(index, u8::from(v));
    }

    pub fn subblock(&mut self, index: u64, f: impl FnOnce(&mut Self)) {
        self.tag(index, TAG_LENGTH4);
        let at = self.len();
        self.u32(0);
        f(self);
        let length = (self.len() - at - 4) as u32;
        self.patch_u32(at, length);
    }

    pub fn write_lww<T>(&mut self, index: u64, lww: &Lww<T>, f: impl FnOnce(&mut Self, &T)) {
        self.subblock(index, |w| {
            w.write_id(1, lww.timestamp);
            f(w, &lww.value);
        });
    }

    pub fn string_body(&mut self, s: &RmString) {
        self.varuint(s.value.len() as u64);
        self.u8(s.ascii_flag);
        self.bytes(s.value.as_bytes());
    }

    pub fn write_string(&mut self, index: u64, s: &RmString) {
        self.subblock(index, |w| w.string_body(s));
    }
}
