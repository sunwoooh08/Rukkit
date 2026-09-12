//! Binary NBT, in both the file and network encodings.
//!
//! Two details trip up most reimplementations and are handled explicitly here:
//!
//! * **Network NBT has no root name.** Since 1.20.2 a network root is written
//!   as a bare type byte followed by its payload, where the file format writes
//!   a type byte, a name, and then the payload.
//! * **Strings are Java's "modified UTF-8", not UTF-8.** Supplementary
//!   characters are written as a six-byte surrogate pair and `U+0000` is
//!   written as `C0 80`, so a plain UTF-8 copy corrupts anything above the BMP
//!   (emoji in kick messages and item names, most visibly).

use crate::error::{ProtocolError, Result};
use crate::reader::PacketReader;
use crate::writer::PacketWrite;

pub const TAG_END: u8 = 0;
pub const TAG_BYTE: u8 = 1;
pub const TAG_SHORT: u8 = 2;
pub const TAG_INT: u8 = 3;
pub const TAG_LONG: u8 = 4;
pub const TAG_FLOAT: u8 = 5;
pub const TAG_DOUBLE: u8 = 6;
pub const TAG_BYTE_ARRAY: u8 = 7;
pub const TAG_STRING: u8 = 8;
pub const TAG_LIST: u8 = 9;
pub const TAG_COMPOUND: u8 = 10;
pub const TAG_INT_ARRAY: u8 = 11;
pub const TAG_LONG_ARRAY: u8 = 12;

/// Nesting limit, matching vanilla. Decoding is recursive, so without this a
/// few kilobytes of nested lists from a hostile client would overflow the
/// stack.
pub const MAX_DEPTH: usize = 512;

/// Cap on any single array payload, to bound allocation from a bad length.
const MAX_ARRAY_LEN: usize = 1 << 24;

#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(NbtList),
    Compound(NbtCompound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtTag {
    #[must_use]
    pub const fn type_id(&self) -> u8 {
        match self {
            Self::Byte(_) => TAG_BYTE,
            Self::Short(_) => TAG_SHORT,
            Self::Int(_) => TAG_INT,
            Self::Long(_) => TAG_LONG,
            Self::Float(_) => TAG_FLOAT,
            Self::Double(_) => TAG_DOUBLE,
            Self::ByteArray(_) => TAG_BYTE_ARRAY,
            Self::String(_) => TAG_STRING,
            Self::List(_) => TAG_LIST,
            Self::Compound(_) => TAG_COMPOUND,
            Self::IntArray(_) => TAG_INT_ARRAY,
            Self::LongArray(_) => TAG_LONG_ARRAY,
        }
    }

    #[must_use]
    pub fn as_compound(&self) -> Option<&NbtCompound> {
        match self {
            Self::Compound(c) => Some(c),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Reads the integer value of any integral tag, widening to `i64`.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Self::Byte(v) => i64::from(*v),
            Self::Short(v) => i64::from(*v),
            Self::Int(v) => i64::from(*v),
            Self::Long(v) => *v,
            _ => return None,
        })
    }
}

impl From<i8> for NbtTag {
    fn from(v: i8) -> Self {
        Self::Byte(v)
    }
}
impl From<bool> for NbtTag {
    fn from(v: bool) -> Self {
        Self::Byte(i8::from(v))
    }
}
impl From<i16> for NbtTag {
    fn from(v: i16) -> Self {
        Self::Short(v)
    }
}
impl From<i32> for NbtTag {
    fn from(v: i32) -> Self {
        Self::Int(v)
    }
}
impl From<i64> for NbtTag {
    fn from(v: i64) -> Self {
        Self::Long(v)
    }
}
impl From<f32> for NbtTag {
    fn from(v: f32) -> Self {
        Self::Float(v)
    }
}
impl From<f64> for NbtTag {
    fn from(v: f64) -> Self {
        Self::Double(v)
    }
}
impl From<String> for NbtTag {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}
impl From<&str> for NbtTag {
    fn from(v: &str) -> Self {
        Self::String(v.to_owned())
    }
}
impl From<NbtCompound> for NbtTag {
    fn from(v: NbtCompound) -> Self {
        Self::Compound(v)
    }
}
impl From<NbtList> for NbtTag {
    fn from(v: NbtList) -> Self {
        Self::List(v)
    }
}

/// A homogeneous NBT list.
///
/// The element type is stored rather than derived, because an empty list still
/// carries one on the wire and round-tripping must preserve it.
#[derive(Debug, Clone, PartialEq)]
pub struct NbtList {
    element_type: u8,
    items: Vec<NbtTag>,
}

impl NbtList {
    /// An empty list, which vanilla writes with an element type of `TAG_End`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            element_type: TAG_END,
            items: Vec::new(),
        }
    }

    /// Builds a list from tags, which must all share one type.
    ///
    /// Returns `None` for a mixed-type list, which NBT cannot represent.
    #[must_use]
    pub fn new(items: Vec<NbtTag>) -> Option<Self> {
        let Some(first) = items.first() else {
            return Some(Self::empty());
        };
        let element_type = first.type_id();
        if items.iter().any(|t| t.type_id() != element_type) {
            return None;
        }
        Some(Self {
            element_type,
            items,
        })
    }

    #[must_use]
    pub const fn element_type(&self) -> u8 {
        self.element_type
    }

    #[must_use]
    pub fn items(&self) -> &[NbtTag] {
        &self.items
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// An NBT compound.
///
/// Backed by an insertion-ordered `Vec` rather than a hash map: compounds in
/// practice hold a handful of keys, where linear scanning over contiguous
/// memory beats hashing, and preserving order keeps encodings reproducible.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NbtCompound {
    entries: Vec<(String, NbtTag)>,
}

impl NbtCompound {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Inserts a value, replacing any existing entry with the same key.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<NbtTag>) -> &mut Self {
        let key = key.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
        self
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&NbtTag> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &NbtTag)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Modified UTF-8
// ---------------------------------------------------------------------------

/// Length of `s` in Java's modified UTF-8 encoding.
#[must_use]
pub fn modified_utf8_len(s: &str) -> usize {
    s.chars()
        .map(|c| match c as u32 {
            0 => 2,
            cp if cp < 0x80 => 1,
            cp if cp < 0x800 => 2,
            cp if cp < 0x1_0000 => 3,
            // Written as a surrogate pair, three bytes each.
            _ => 6,
        })
        .sum()
}

fn write_modified_utf8(s: &str, out: &mut impl PacketWrite) {
    let len = modified_utf8_len(s);
    // NBT string lengths are u16; anything longer is unrepresentable, so it is
    // truncated at a character boundary rather than producing a corrupt frame.
    let (s, len) = if len > u16::MAX as usize {
        let mut cut = 0;
        let mut used = 0;
        for (idx, c) in s.char_indices() {
            let need = modified_utf8_len(c.encode_utf8(&mut [0u8; 4]));
            if used + need > u16::MAX as usize {
                break;
            }
            used += need;
            cut = idx + c.len_utf8();
        }
        (&s[..cut], used)
    } else {
        (s, len)
    };

    out.write_u16(len as u16);
    for c in s.chars() {
        let cp = c as u32;
        if cp != 0 && cp < 0x80 {
            out.write_u8(cp as u8);
        } else if cp == 0 || cp < 0x800 {
            out.write_u8(0xC0 | (cp >> 6) as u8);
            out.write_u8(0x80 | (cp & 0x3F) as u8);
        } else if cp < 0x1_0000 {
            out.write_u8(0xE0 | (cp >> 12) as u8);
            out.write_u8(0x80 | ((cp >> 6) & 0x3F) as u8);
            out.write_u8(0x80 | (cp & 0x3F) as u8);
        } else {
            let v = cp - 0x1_0000;
            for unit in [0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)] {
                out.write_u8(0xE0 | (unit >> 12) as u8);
                out.write_u8(0x80 | ((unit >> 6) & 0x3F) as u8);
                out.write_u8(0x80 | (unit & 0x3F) as u8);
            }
        }
    }
}

fn read_modified_utf8(r: &mut PacketReader<'_>) -> Result<String> {
    let len = r.read_u16()? as usize;
    let bytes = r.read_bytes(len)?;
    decode_modified_utf8(bytes)
}

fn continuation(bytes: &[u8], i: usize) -> Result<u32> {
    let b = *bytes
        .get(i)
        .ok_or(ProtocolError::Nbt("truncated modified UTF-8 sequence"))?;
    if b & 0xC0 != 0x80 {
        return Err(ProtocolError::Nbt("bad continuation byte"));
    }
    Ok(u32::from(b & 0x3F))
}

fn decode_modified_utf8(bytes: &[u8]) -> Result<String> {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(b as char);
            i += 1;
        } else if b & 0xE0 == 0xC0 {
            let cp = (u32::from(b & 0x1F) << 6) | continuation(bytes, i + 1)?;
            out.push(char::from_u32(cp).ok_or(ProtocolError::Nbt("invalid code point"))?);
            i += 2;
        } else if b & 0xF0 == 0xE0 {
            let unit = (u32::from(b & 0x0F) << 12)
                | (continuation(bytes, i + 1)? << 6)
                | continuation(bytes, i + 2)?;
            if (0xD800..0xDC00).contains(&unit) {
                // High surrogate: the matching low surrogate follows as its own
                // three-byte sequence.
                let lead = *bytes
                    .get(i + 3)
                    .ok_or(ProtocolError::Nbt("unpaired high surrogate"))?;
                if lead & 0xF0 != 0xE0 {
                    return Err(ProtocolError::Nbt("unpaired high surrogate"));
                }
                let low = (u32::from(lead & 0x0F) << 12)
                    | (continuation(bytes, i + 4)? << 6)
                    | continuation(bytes, i + 5)?;
                if !(0xDC00..0xE000).contains(&low) {
                    return Err(ProtocolError::Nbt("unpaired high surrogate"));
                }
                let cp = 0x1_0000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                out.push(char::from_u32(cp).ok_or(ProtocolError::Nbt("invalid code point"))?);
                i += 6;
            } else if (0xDC00..0xE000).contains(&unit) {
                return Err(ProtocolError::Nbt("unpaired low surrogate"));
            } else {
                out.push(char::from_u32(unit).ok_or(ProtocolError::Nbt("invalid code point"))?);
                i += 3;
            }
        } else {
            return Err(ProtocolError::Nbt("invalid modified UTF-8 lead byte"));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Writes a tag in the network encoding: type byte, then payload, no root name.
pub fn write_network(tag: &NbtTag, out: &mut impl PacketWrite) {
    out.write_u8(tag.type_id());
    write_payload(tag, out);
}

/// Writes a tag in the file encoding: type byte, root name, then payload.
pub fn write_named(name: &str, tag: &NbtTag, out: &mut impl PacketWrite) {
    out.write_u8(tag.type_id());
    write_modified_utf8(name, out);
    write_payload(tag, out);
}

fn write_payload(tag: &NbtTag, out: &mut impl PacketWrite) {
    match tag {
        NbtTag::Byte(v) => out.write_i8(*v),
        NbtTag::Short(v) => out.write_i16(*v),
        NbtTag::Int(v) => out.write_i32(*v),
        NbtTag::Long(v) => out.write_i64(*v),
        NbtTag::Float(v) => out.write_f32(*v),
        NbtTag::Double(v) => out.write_f64(*v),
        NbtTag::ByteArray(v) => {
            out.write_i32(v.len() as i32);
            for &b in v {
                out.write_i8(b);
            }
        }
        NbtTag::String(v) => write_modified_utf8(v, out),
        NbtTag::List(list) => {
            out.write_u8(list.element_type);
            out.write_i32(list.items.len() as i32);
            for item in &list.items {
                write_payload(item, out);
            }
        }
        NbtTag::Compound(compound) => {
            for (name, value) in compound.iter() {
                out.write_u8(value.type_id());
                write_modified_utf8(name, out);
                write_payload(value, out);
            }
            out.write_u8(TAG_END);
        }
        NbtTag::IntArray(v) => {
            out.write_i32(v.len() as i32);
            for &n in v {
                out.write_i32(n);
            }
        }
        NbtTag::LongArray(v) => {
            out.write_i32(v.len() as i32);
            for &n in v {
                out.write_i64(n);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Reads a tag in the network encoding (no root name).
pub fn read_network(r: &mut PacketReader<'_>) -> Result<NbtTag> {
    let type_id = r.read_u8()?;
    if type_id == TAG_END {
        return Err(ProtocolError::Nbt("root tag is TAG_End"));
    }
    read_payload(r, type_id, 0)
}

/// Reads a tag in the file encoding, returning the root name alongside it.
pub fn read_named(r: &mut PacketReader<'_>) -> Result<(String, NbtTag)> {
    let type_id = r.read_u8()?;
    if type_id == TAG_END {
        return Err(ProtocolError::Nbt("root tag is TAG_End"));
    }
    let name = read_modified_utf8(r)?;
    Ok((name, read_payload(r, type_id, 0)?))
}

fn read_len(r: &mut PacketReader<'_>) -> Result<usize> {
    let len = r.read_i32()?;
    if len < 0 {
        return Err(ProtocolError::NegativeLength(len));
    }
    let len = len as usize;
    if len > MAX_ARRAY_LEN {
        return Err(ProtocolError::LengthLimitExceeded {
            len,
            max: MAX_ARRAY_LEN,
        });
    }
    Ok(len)
}

fn read_payload(r: &mut PacketReader<'_>, type_id: u8, depth: usize) -> Result<NbtTag> {
    if depth > MAX_DEPTH {
        return Err(ProtocolError::NbtTooDeep { max: MAX_DEPTH });
    }
    Ok(match type_id {
        TAG_BYTE => NbtTag::Byte(r.read_i8()?),
        TAG_SHORT => NbtTag::Short(r.read_i16()?),
        TAG_INT => NbtTag::Int(r.read_i32()?),
        TAG_LONG => NbtTag::Long(r.read_i64()?),
        TAG_FLOAT => NbtTag::Float(r.read_f32()?),
        TAG_DOUBLE => NbtTag::Double(r.read_f64()?),
        TAG_BYTE_ARRAY => {
            let len = read_len(r)?;
            let bytes = r.read_bytes(len)?;
            NbtTag::ByteArray(bytes.iter().map(|&b| b as i8).collect())
        }
        TAG_STRING => NbtTag::String(read_modified_utf8(r)?),
        TAG_LIST => {
            let element_type = r.read_u8()?;
            let len = read_len(r)?;
            if element_type == TAG_END && len > 0 {
                return Err(ProtocolError::Nbt("non-empty list of TAG_End"));
            }
            // Bound the eager allocation by the bytes actually available: even
            // the smallest element is one byte.
            let mut items = Vec::with_capacity(len.min(r.remaining()));
            for _ in 0..len {
                items.push(read_payload(r, element_type, depth + 1)?);
            }
            NbtTag::List(NbtList {
                element_type,
                items,
            })
        }
        TAG_COMPOUND => {
            let mut compound = NbtCompound::new();
            loop {
                let entry_type = r.read_u8()?;
                if entry_type == TAG_END {
                    break;
                }
                let name = read_modified_utf8(r)?;
                let value = read_payload(r, entry_type, depth + 1)?;
                compound.insert(name, value);
            }
            NbtTag::Compound(compound)
        }
        TAG_INT_ARRAY => {
            let len = read_len(r)?;
            let mut values = Vec::with_capacity(len.min(r.remaining() / 4));
            for _ in 0..len {
                values.push(r.read_i32()?);
            }
            NbtTag::IntArray(values)
        }
        TAG_LONG_ARRAY => {
            let len = read_len(r)?;
            let mut values = Vec::with_capacity(len.min(r.remaining() / 8));
            for _ in 0..len {
                values.push(r.read_i64()?);
            }
            NbtTag::LongArray(values)
        }
        other => {
            return Err(ProtocolError::InvalidEnum {
                kind: "NBT tag type",
                value: i64::from(other),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_network(tag: &NbtTag) -> NbtTag {
        let mut buf = Vec::new();
        write_network(tag, &mut buf);
        let mut r = PacketReader::new(&buf);
        let decoded = read_network(&mut r).expect("decode");
        assert!(r.is_empty(), "decoder left {} bytes", r.remaining());
        decoded
    }

    #[test]
    fn primitives_round_trip() {
        for tag in [
            NbtTag::Byte(-128),
            NbtTag::Short(-32768),
            NbtTag::Int(i32::MIN),
            NbtTag::Long(i64::MAX),
            NbtTag::Float(1.5),
            NbtTag::Double(-2.25),
            NbtTag::String("hello".into()),
            NbtTag::ByteArray(vec![-1, 0, 1]),
            NbtTag::IntArray(vec![1, -2, 3]),
            NbtTag::LongArray(vec![i64::MIN, 0, i64::MAX]),
        ] {
            assert_eq!(round_trip_network(&tag), tag);
        }
    }

    #[test]
    fn nested_compound_round_trips_and_keeps_order() {
        let mut inner = NbtCompound::new();
        inner.insert("z_first", 1i32).insert("a_second", "text");

        let mut root = NbtCompound::new();
        root.insert("name", "Rukkit")
            .insert("count", 7i32)
            .insert("flag", true)
            .insert("inner", inner);

        let decoded = round_trip_network(&NbtTag::Compound(root.clone()));
        assert_eq!(decoded, NbtTag::Compound(root.clone()));

        // Insertion order is preserved, which keeps encodings byte-stable.
        let keys: Vec<_> = decoded
            .as_compound()
            .unwrap()
            .iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, ["name", "count", "flag", "inner"]);
    }

    #[test]
    fn empty_list_keeps_its_element_type() {
        let tag = NbtTag::List(NbtList::empty());
        let decoded = round_trip_network(&tag);
        match decoded {
            NbtTag::List(list) => {
                assert_eq!(list.element_type(), TAG_END);
                assert!(list.is_empty());
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn list_rejects_mixed_element_types() {
        assert!(NbtList::new(vec![NbtTag::Int(1), NbtTag::String("x".into())]).is_none());
        assert!(NbtList::new(vec![NbtTag::Int(1), NbtTag::Int(2)]).is_some());
    }

    #[test]
    fn named_root_round_trips() {
        let tag = NbtTag::Compound({
            let mut c = NbtCompound::new();
            c.insert("DataVersion", 4903i32);
            c
        });
        let mut buf = Vec::new();
        write_named("Level", &tag, &mut buf);
        let (name, decoded) = read_named(&mut PacketReader::new(&buf)).unwrap();
        assert_eq!(name, "Level");
        assert_eq!(decoded, tag);
    }

    #[test]
    fn network_root_carries_no_name() {
        let tag = NbtTag::Compound(NbtCompound::new());
        let mut network = Vec::new();
        write_network(&tag, &mut network);
        // Type byte plus the compound's own TAG_End terminator, nothing else.
        assert_eq!(network, vec![TAG_COMPOUND, TAG_END]);

        let mut named = Vec::new();
        write_named("", &tag, &mut named);
        assert_eq!(named, vec![TAG_COMPOUND, 0x00, 0x00, TAG_END]);
    }

    #[test]
    fn modified_utf8_handles_supplementary_characters() {
        // U+1F600 is outside the BMP: modified UTF-8 writes it as a six-byte
        // surrogate pair where plain UTF-8 would use four bytes.
        let text = "hi \u{1F600}!";
        assert_eq!(modified_utf8_len(text), 3 + 6 + 1);
        assert_ne!(modified_utf8_len(text), text.len());

        let tag = NbtTag::String(text.into());
        assert_eq!(round_trip_network(&tag), tag);
    }

    #[test]
    fn modified_utf8_encodes_nul_as_two_bytes() {
        let tag = NbtTag::String("a\u{0}b".into());
        let mut buf = Vec::new();
        write_network(&tag, &mut buf);
        // type, u16 length, then 'a', C0 80, 'b'
        assert_eq!(&buf[1..3], &[0x00, 0x04]);
        assert_eq!(&buf[3..], &[b'a', 0xC0, 0x80, b'b']);
        assert_eq!(round_trip_network(&tag), tag);
    }

    #[test]
    fn unpaired_surrogates_are_rejected() {
        // A lone high surrogate, hand-encoded.
        let bytes = [0xED, 0xA0, 0x80];
        assert!(decode_modified_utf8(&bytes).is_err());
        // A lone low surrogate.
        let bytes = [0xED, 0xB0, 0x80];
        assert!(decode_modified_utf8(&bytes).is_err());
    }

    #[test]
    fn truncated_sequences_are_rejected() {
        assert!(decode_modified_utf8(&[0xC2]).is_err());
        assert!(decode_modified_utf8(&[0xE0, 0xA0]).is_err());
        assert!(decode_modified_utf8(&[0xFF]).is_err());
    }

    #[test]
    fn deep_nesting_is_rejected_instead_of_overflowing_the_stack() {
        // MAX_DEPTH + 2 nested lists, each one element long.
        let mut buf = Vec::new();
        buf.push(TAG_LIST);
        let depth = MAX_DEPTH + 2;
        for _ in 0..depth {
            buf.push(TAG_LIST);
            buf.extend_from_slice(&1i32.to_be_bytes());
        }
        buf.push(TAG_END);
        buf.extend_from_slice(&0i32.to_be_bytes());

        let err = read_network(&mut PacketReader::new(&buf)).unwrap_err();
        assert!(matches!(err, ProtocolError::NbtTooDeep { .. }));
    }

    #[test]
    fn bogus_array_length_does_not_preallocate() {
        // Claims 16 million ints in a 5-byte payload.
        let mut buf = Vec::new();
        buf.push(TAG_INT_ARRAY);
        buf.extend_from_slice(&(MAX_ARRAY_LEN as i32).to_be_bytes());
        let err = read_network(&mut PacketReader::new(&buf)).unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedEof { .. }));

        // Over the cap it is refused outright.
        let mut buf = Vec::new();
        buf.push(TAG_INT_ARRAY);
        buf.extend_from_slice(&i32::MAX.to_be_bytes());
        let err = read_network(&mut PacketReader::new(&buf)).unwrap_err();
        assert!(matches!(err, ProtocolError::LengthLimitExceeded { .. }));
    }

    #[test]
    fn negative_array_length_is_rejected() {
        let mut buf = Vec::new();
        buf.push(TAG_BYTE_ARRAY);
        buf.extend_from_slice(&(-1i32).to_be_bytes());
        let err = read_network(&mut PacketReader::new(&buf)).unwrap_err();
        assert!(matches!(err, ProtocolError::NegativeLength(-1)));
    }

    #[test]
    fn unknown_tag_type_is_rejected() {
        let err = read_network(&mut PacketReader::new(&[99])).unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidEnum { .. }));
    }
}
