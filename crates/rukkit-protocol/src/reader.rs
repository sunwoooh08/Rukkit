//! Zero-copy reader over a decoded packet body.
//!
//! A [`PacketReader`] borrows the frame buffer and walks it with a cursor, so
//! decoding a packet allocates only for the fields that genuinely need to own
//! their data (`String`, `Vec`). Every read is bounds-checked and returns an
//! error rather than panicking: the bytes come from the network.

use uuid::Uuid;

use crate::error::{ProtocolError, Result};
use crate::types::{BlockPos, ChunkPos};
use crate::varint;

/// Vanilla's default cap for `readUtf()` with no explicit limit.
pub const DEFAULT_MAX_STRING_LEN: usize = 32767;

/// Upper bound applied to every length-prefixed collection that has no tighter
/// limit of its own, so a bogus prefix cannot make us preallocate wildly.
pub const DEFAULT_MAX_COLLECTION_LEN: usize = 1 << 20;

#[derive(Debug, Clone)]
pub struct PacketReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> PacketReader<'a> {
    #[inline]
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    #[inline]
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// The not-yet-consumed tail, without advancing.
    #[inline]
    #[must_use]
    pub fn peek_rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(ProtocolError::UnexpectedEof {
                needed: n,
                available: self.remaining(),
            })?;
        if end > self.buf.len() {
            return Err(ProtocolError::UnexpectedEof {
                needed: n,
                available: self.remaining(),
            });
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    #[inline]
    fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    #[inline]
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    #[inline]
    pub fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Reads a boolean. Any non-zero byte is `true`, matching vanilla.
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    #[inline]
    pub fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take_array::<2>()?))
    }

    #[inline]
    pub fn read_i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take_array::<2>()?))
    }

    #[inline]
    pub fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take_array::<4>()?))
    }

    #[inline]
    pub fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take_array::<8>()?))
    }

    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take_array::<8>()?))
    }

    #[inline]
    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take_array::<4>()?))
    }

    #[inline]
    pub fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.take_array::<8>()?))
    }

    #[inline]
    pub fn read_varint(&mut self) -> Result<i32> {
        let (value, used) = varint::read_varint(&self.buf[self.pos..])?;
        self.pos += used;
        Ok(value)
    }

    #[inline]
    pub fn read_varlong(&mut self) -> Result<i64> {
        let (value, used) = varint::read_varlong(&self.buf[self.pos..])?;
        self.pos += used;
        Ok(value)
    }

    /// Reads a length prefix that must be a non-negative count no larger than
    /// `max`.
    #[inline]
    pub fn read_length(&mut self, max: usize) -> Result<usize> {
        let raw = self.read_varint()?;
        if raw < 0 {
            return Err(ProtocolError::NegativeLength(raw));
        }
        let len = raw as usize;
        if len > max {
            return Err(ProtocolError::LengthLimitExceeded { len, max });
        }
        Ok(len)
    }

    /// Reads a UTF-8 string with a maximum length in characters.
    ///
    /// Mirrors vanilla's two-stage check: the byte prefix may not exceed three
    /// bytes per permitted character, and the decoded string may not exceed
    /// `max_chars` characters.
    pub fn read_string(&mut self, max_chars: usize) -> Result<String> {
        let max_bytes = max_chars.saturating_mul(3);
        let len = self.read_length(max_bytes)?;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| ProtocolError::InvalidUtf8)?;
        let chars = s.chars().count();
        if chars > max_chars {
            return Err(ProtocolError::StringTooManyChars {
                len: chars,
                max: max_chars,
            });
        }
        Ok(s.to_owned())
    }

    /// Reads a namespaced identifier such as `minecraft:stone`.
    #[inline]
    pub fn read_identifier(&mut self) -> Result<String> {
        self.read_string(DEFAULT_MAX_STRING_LEN)
    }

    /// Reads a 128-bit UUID, most significant half first.
    #[inline]
    pub fn read_uuid(&mut self) -> Result<Uuid> {
        Ok(Uuid::from_u128(u128::from_be_bytes(
            self.take_array::<16>()?,
        )))
    }

    /// Borrows exactly `n` bytes.
    #[inline]
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }

    /// Reads a VarInt-prefixed byte array.
    #[inline]
    pub fn read_byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.read_length(max)?;
        self.take(len)
    }

    /// Consumes and returns everything left in the packet.
    #[inline]
    pub fn read_remaining(&mut self) -> &'a [u8] {
        let rest = &self.buf[self.pos..];
        self.pos = self.buf.len();
        rest
    }

    #[inline]
    pub fn read_position(&mut self) -> Result<BlockPos> {
        Ok(BlockPos::decode(self.read_i64()?))
    }

    #[inline]
    pub fn read_chunk_pos(&mut self) -> Result<ChunkPos> {
        Ok(ChunkPos::from_long(self.read_i64()?))
    }

    /// Reads an `Option<T>`, encoded as a presence boolean followed by the
    /// value when present.
    #[inline]
    pub fn read_option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.read_bool()? {
            Ok(Some(f(self)?))
        } else {
            Ok(None)
        }
    }

    /// Reads a VarInt-prefixed list.
    ///
    /// Capacity is reserved lazily via [`Vec::with_capacity`] only after the
    /// length has been validated against `max`, so an absurd prefix cannot be
    /// turned into an allocation.
    pub fn read_array<T>(
        &mut self,
        max: usize,
        mut f: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let len = self.read_length(max)?;
        // Even a validated length can exceed what is actually left in the
        // frame, so cap the eager allocation by the bytes on hand.
        let mut out = Vec::with_capacity(len.min(self.remaining()));
        for _ in 0..len {
            out.push(f(self)?);
        }
        Ok(out)
    }

    /// Errors unless the packet has been fully consumed.
    ///
    /// Vanilla treats leftover bytes as a decoding fault, and so do we: it
    /// almost always means the packet layout drifted from the client's.
    #[inline]
    pub fn expect_finished(&self, state: &'static str, id: i32) -> Result<()> {
        if self.remaining() != 0 {
            return Err(ProtocolError::TrailingBytes {
                state,
                id,
                trailing: self.remaining(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::PacketWrite;

    #[test]
    fn reads_primitives_in_order() {
        let mut buf = Vec::new();
        buf.write_bool(true);
        buf.write_i8(-5);
        buf.write_u16(65535);
        buf.write_i32(-123456);
        buf.write_i64(i64::MIN);
        buf.write_f32(1.5);
        buf.write_f64(-2.25);
        buf.write_varint(25565);

        let mut r = PacketReader::new(&buf);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_i8().unwrap(), -5);
        assert_eq!(r.read_u16().unwrap(), 65535);
        assert_eq!(r.read_i32().unwrap(), -123456);
        assert_eq!(r.read_i64().unwrap(), i64::MIN);
        assert_eq!(r.read_f32().unwrap(), 1.5);
        assert_eq!(r.read_f64().unwrap(), -2.25);
        assert_eq!(r.read_varint().unwrap(), 25565);
        assert!(r.is_empty());
    }

    #[test]
    fn reading_past_the_end_errors_instead_of_panicking() {
        let mut r = PacketReader::new(&[0x01]);
        assert_eq!(r.read_u8().unwrap(), 1);
        assert!(matches!(
            r.read_u8().unwrap_err(),
            ProtocolError::UnexpectedEof { .. }
        ));
        assert!(matches!(
            PacketReader::new(&[0, 0]).read_i32().unwrap_err(),
            ProtocolError::UnexpectedEof { .. }
        ));
    }

    #[test]
    fn strings_round_trip_including_multibyte() {
        for original in ["", "hello", "서버", "emoji \u{1F600} ok"] {
            let mut buf = Vec::new();
            buf.write_string(original);
            let mut r = PacketReader::new(&buf);
            assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), original);
            assert!(r.is_empty());
        }
    }

    #[test]
    fn string_limit_accepts_multibyte_up_to_the_character_count() {
        // Four 3-byte characters: 12 bytes but only 4 characters. A byte-based
        // limit would wrongly reject this at a limit of 4.
        let mut buf = Vec::new();
        buf.write_string("가나다라");
        assert_eq!(PacketReader::new(&buf).read_string(4).unwrap(), "가나다라");
        // Over the limit it is refused — here by the byte prefix check, which
        // runs first exactly as vanilla's does.
        assert!(PacketReader::new(&buf).read_string(3).is_err());
    }

    #[test]
    fn string_char_limit_catches_what_the_byte_limit_misses() {
        // Five ASCII characters are 5 bytes, comfortably under the 4*3 byte
        // allowance, so only the character count can reject them.
        let mut buf = Vec::new();
        buf.write_string("abcde");
        let err = PacketReader::new(&buf).read_string(4).unwrap_err();
        assert!(
            matches!(err, ProtocolError::StringTooManyChars { len: 5, max: 4 }),
            "{err:?}"
        );
        assert_eq!(PacketReader::new(&buf).read_string(5).unwrap(), "abcde");
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut buf = Vec::new();
        buf.write_varint(2);
        buf.extend_from_slice(&[0xFF, 0xFE]);
        let err = PacketReader::new(&buf)
            .read_string(DEFAULT_MAX_STRING_LEN)
            .unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidUtf8));
    }

    #[test]
    fn negative_and_oversized_lengths_are_rejected() {
        let mut buf = Vec::new();
        buf.write_varint(-1);
        assert!(matches!(
            PacketReader::new(&buf).read_length(16).unwrap_err(),
            ProtocolError::NegativeLength(-1)
        ));

        let mut buf = Vec::new();
        buf.write_varint(9999);
        assert!(matches!(
            PacketReader::new(&buf).read_length(16).unwrap_err(),
            ProtocolError::LengthLimitExceeded { .. }
        ));
    }

    #[test]
    fn huge_length_prefix_does_not_preallocate() {
        // A 3-byte packet claiming a million elements must fail on the element
        // read, not by reserving a million slots.
        let mut buf = Vec::new();
        buf.write_varint(1_000_000);
        let mut r = PacketReader::new(&buf);
        let err = r
            .read_array(DEFAULT_MAX_COLLECTION_LEN, |r| r.read_i64())
            .unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedEof { .. }));
    }

    #[test]
    fn uuid_round_trips() {
        let id = Uuid::from_u128(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF);
        let mut buf = Vec::new();
        buf.write_uuid(id);
        assert_eq!(buf.len(), 16);
        assert_eq!(PacketReader::new(&buf).read_uuid().unwrap(), id);
    }

    #[test]
    fn options_and_arrays_round_trip() {
        let mut buf = Vec::new();
        buf.write_option(Some(7i32), |b, v| b.write_varint(v));
        buf.write_option(None::<i32>, |b, v| b.write_varint(v));
        buf.write_array(&[1i32, 2, 3], |b, v| b.write_varint(*v));

        let mut r = PacketReader::new(&buf);
        assert_eq!(r.read_option(|r| r.read_varint()).unwrap(), Some(7));
        assert_eq!(r.read_option(|r| r.read_varint()).unwrap(), None);
        assert_eq!(
            r.read_array(16, |r| r.read_varint()).unwrap(),
            vec![1, 2, 3]
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trailing_bytes_are_flagged() {
        let r = PacketReader::new(&[0x00, 0x01]);
        assert!(matches!(
            r.expect_finished("play", 0).unwrap_err(),
            ProtocolError::TrailingBytes { trailing: 2, .. }
        ));
        assert!(PacketReader::new(&[]).expect_finished("play", 0).is_ok());
    }
}
