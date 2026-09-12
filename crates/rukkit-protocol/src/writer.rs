//! Encoding half of the wire format.
//!
//! Implemented as a blanket extension trait over [`bytes::BufMut`], so the same
//! code writes into a `Vec<u8>`, a `BytesMut`, or a borrowed slice of an
//! existing send buffer without copying between them.

use bytes::BufMut;
use uuid::Uuid;

use crate::types::{angle_to_byte, BlockPos, ChunkPos};
use crate::varint;

pub trait PacketWrite: BufMut + Sized {
    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.put_u8(value);
    }

    #[inline]
    fn write_i8(&mut self, value: i8) {
        self.put_i8(value);
    }

    #[inline]
    fn write_bool(&mut self, value: bool) {
        self.put_u8(u8::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.put_u16(value);
    }

    #[inline]
    fn write_i16(&mut self, value: i16) {
        self.put_i16(value);
    }

    #[inline]
    fn write_i32(&mut self, value: i32) {
        self.put_i32(value);
    }

    #[inline]
    fn write_i64(&mut self, value: i64) {
        self.put_i64(value);
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.put_u64(value);
    }

    #[inline]
    fn write_f32(&mut self, value: f32) {
        self.put_f32(value);
    }

    #[inline]
    fn write_f64(&mut self, value: f64) {
        self.put_f64(value);
    }

    #[inline]
    fn write_varint(&mut self, value: i32) {
        varint::write_varint(self, value);
    }

    #[inline]
    fn write_varlong(&mut self, value: i64) {
        varint::write_varlong(self, value);
    }

    /// Writes a length-prefixed UTF-8 string.
    #[inline]
    fn write_string(&mut self, value: &str) {
        self.write_varint(value.len() as i32);
        self.put_slice(value.as_bytes());
    }

    /// Writes a namespaced identifier. Identical on the wire to a string; the
    /// separate name keeps packet definitions self-documenting.
    #[inline]
    fn write_identifier(&mut self, value: &str) {
        self.write_string(value);
    }

    /// Writes a 128-bit UUID, most significant half first.
    #[inline]
    fn write_uuid(&mut self, value: Uuid) {
        self.put_slice(&value.as_u128().to_be_bytes());
    }

    /// Writes raw bytes with no length prefix.
    #[inline]
    fn write_bytes(&mut self, value: &[u8]) {
        self.put_slice(value);
    }

    /// Writes a VarInt-prefixed byte array.
    #[inline]
    fn write_byte_array(&mut self, value: &[u8]) {
        self.write_varint(value.len() as i32);
        self.put_slice(value);
    }

    #[inline]
    fn write_position(&mut self, value: BlockPos) {
        self.put_i64(value.encode());
    }

    #[inline]
    fn write_chunk_pos(&mut self, value: ChunkPos) {
        self.put_i64(value.to_long());
    }

    /// Writes a rotation as a single byte covering a full turn.
    #[inline]
    fn write_angle(&mut self, degrees: f32) {
        self.put_u8(angle_to_byte(degrees));
    }

    /// Writes an optional value as a presence boolean plus the payload.
    #[inline]
    fn write_option<T>(&mut self, value: Option<T>, f: impl FnOnce(&mut Self, T)) {
        match value {
            Some(v) => {
                self.write_bool(true);
                f(self, v);
            }
            None => self.write_bool(false),
        }
    }

    /// Writes a VarInt-prefixed list.
    #[inline]
    fn write_array<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) {
        self.write_varint(items.len() as i32);
        for item in items {
            f(self, item);
        }
    }

    /// Writes a bitset as a VarInt-prefixed array of longs, the encoding used
    /// for chunk section masks and similar fields.
    #[inline]
    fn write_long_array(&mut self, values: &[i64]) {
        self.write_varint(values.len() as i32);
        for &v in values {
            self.put_i64(v);
        }
    }
}

impl<T: BufMut> PacketWrite for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{PacketReader, DEFAULT_MAX_STRING_LEN};

    #[test]
    fn string_is_byte_length_prefixed() {
        let mut buf = Vec::new();
        buf.write_string("가"); // one character, three UTF-8 bytes
        assert_eq!(buf[0], 3);
        assert_eq!(buf.len(), 4);
    }

    fn fill(out: &mut impl PacketWrite) {
        out.write_varint(42);
        out.write_string("rukkit");
    }

    #[test]
    fn works_over_bytes_mut_as_well_as_vec() {
        let mut vec = Vec::new();
        let mut bytes = bytes::BytesMut::new();
        fill(&mut vec);
        fill(&mut bytes);
        assert_eq!(&vec[..], &bytes[..]);

        let mut r = PacketReader::new(&vec);
        assert_eq!(r.read_varint().unwrap(), 42);
        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "rukkit");
        assert!(r.is_empty());
    }

    #[test]
    fn long_array_round_trips() {
        let values = [1i64, -1, i64::MAX, i64::MIN];
        let mut buf = Vec::new();
        buf.write_long_array(&values);

        let mut r = PacketReader::new(&buf);
        let decoded = r.read_array(16, |r| r.read_i64()).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn position_round_trips_through_the_writer() {
        let pos = BlockPos::new(-1234, -60, 5678);
        let mut buf = Vec::new();
        buf.write_position(pos);
        assert_eq!(PacketReader::new(&buf).read_position().unwrap(), pos);
    }
}
