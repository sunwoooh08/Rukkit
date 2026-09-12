//! VarInt and VarLong, the protocol's LEB128-style variable-width integers.
//!
//! These are the single hottest routines in the whole server: every packet
//! begins with a VarInt length and a VarInt id, and a chunk payload contains
//! thousands more. Everything here is branch-light and allocation-free.

use bytes::BufMut;

use crate::error::{ProtocolError, Result};

/// A VarInt encodes at most 32 bits in 7-bit groups, so 5 bytes.
pub const MAX_VARINT_BYTES: usize = 5;

/// A VarLong encodes at most 64 bits in 7-bit groups, so 10 bytes.
pub const MAX_VARLONG_BYTES: usize = 10;

/// Number of bytes `value` occupies when written as a VarInt.
///
/// Computed from the bit width rather than by trial encoding, so reserving
/// space for a length prefix costs one `lzcnt` instead of a loop.
#[inline]
#[must_use]
pub const fn varint_len(value: i32) -> usize {
    let bits = u32::BITS - (value as u32).leading_zeros();
    (bits.saturating_sub(1) / 7 + 1) as usize
}

/// Number of bytes `value` occupies when written as a VarLong.
#[inline]
#[must_use]
pub const fn varlong_len(value: i64) -> usize {
    let bits = u64::BITS - (value as u64).leading_zeros();
    (bits.saturating_sub(1) / 7 + 1) as usize
}

/// Writes `value` as a VarInt.
#[inline]
pub fn write_varint(out: &mut impl BufMut, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.put_u8(v as u8);
            return;
        }
        out.put_u8((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

/// Writes `value` as a VarLong.
#[inline]
pub fn write_varlong(out: &mut impl BufMut, value: i64) {
    let mut v = value as u64;
    loop {
        if v & !0x7F == 0 {
            out.put_u8(v as u8);
            return;
        }
        out.put_u8((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

/// Reads a VarInt from the front of `buf`, returning it with the number of
/// bytes consumed.
#[inline]
pub fn read_varint(buf: &[u8]) -> Result<(i32, usize)> {
    let mut value: u32 = 0;
    let mut shift: u32 = 0;

    let limit = if buf.len() < MAX_VARINT_BYTES {
        buf.len()
    } else {
        MAX_VARINT_BYTES
    };

    for i in 0..limit {
        let byte = buf[i];
        value |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok((value as i32, i + 1));
        }
        shift += 7;
    }

    if buf.len() < MAX_VARINT_BYTES {
        // Truncated rather than malformed: the caller may simply need to read
        // more from the socket before trying again.
        Err(ProtocolError::UnexpectedEof {
            needed: 1,
            available: 0,
        })
    } else {
        Err(ProtocolError::VarIntTooLong {
            max: MAX_VARINT_BYTES,
        })
    }
}

/// Reads a VarLong from the front of `buf`, returning it with the number of
/// bytes consumed.
#[inline]
pub fn read_varlong(buf: &[u8]) -> Result<(i64, usize)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;

    let limit = if buf.len() < MAX_VARLONG_BYTES {
        buf.len()
    } else {
        MAX_VARLONG_BYTES
    };

    for i in 0..limit {
        let byte = buf[i];
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((value as i64, i + 1));
        }
        shift += 7;
    }

    if buf.len() < MAX_VARLONG_BYTES {
        Err(ProtocolError::UnexpectedEof {
            needed: 1,
            available: 0,
        })
    } else {
        Err(ProtocolError::VarIntTooLong {
            max: MAX_VARLONG_BYTES,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodings taken from the vanilla protocol documentation's sample table.
    const VARINT_VECTORS: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (25565, &[0xDD, 0xC7, 0x01]),
        (2097151, &[0xFF, 0xFF, 0x7F]),
        (2147483647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    const VARLONG_VECTORS: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (2147483647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        (
            9223372036854775807,
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
        ),
        (
            -1,
            &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
        ),
        (
            -2147483648,
            &[0x80, 0x80, 0x80, 0x80, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
        ),
    ];

    #[test]
    fn varint_matches_reference_encodings() {
        for &(value, expected) in VARINT_VECTORS {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            assert_eq!(out, expected, "encoding {value}");

            let (decoded, used) = read_varint(expected).unwrap();
            assert_eq!(decoded, value, "decoding {expected:?}");
            assert_eq!(used, expected.len());
        }
    }

    #[test]
    fn varlong_matches_reference_encodings() {
        for &(value, expected) in VARLONG_VECTORS {
            let mut out = Vec::new();
            write_varlong(&mut out, value);
            assert_eq!(out, expected, "encoding {value}");

            let (decoded, used) = read_varlong(expected).unwrap();
            assert_eq!(decoded, value, "decoding {expected:?}");
            assert_eq!(used, expected.len());
        }
    }

    #[test]
    fn len_agrees_with_the_encoder() {
        for &(value, expected) in VARINT_VECTORS {
            assert_eq!(varint_len(value), expected.len(), "len of {value}");
        }
        for &(value, expected) in VARLONG_VECTORS {
            assert_eq!(varlong_len(value), expected.len(), "len of {value}");
        }
    }

    #[test]
    fn len_agrees_with_the_encoder_across_bit_widths() {
        // Walk every power-of-two boundary, where the byte count changes.
        for shift in 0..32 {
            for value in [1i32 << shift, (1i32 << shift).wrapping_sub(1)] {
                let mut out = Vec::new();
                write_varint(&mut out, value);
                assert_eq!(varint_len(value), out.len(), "len of {value}");
            }
        }
        for shift in 0..64 {
            for value in [1i64 << shift, (1i64 << shift).wrapping_sub(1)] {
                let mut out = Vec::new();
                write_varlong(&mut out, value);
                assert_eq!(varlong_len(value), out.len(), "len of {value}");
            }
        }
    }

    #[test]
    fn truncated_varint_is_reported_as_eof() {
        // Every byte has the continuation bit set but the buffer runs out.
        let err = read_varint(&[0x80, 0x80]).unwrap_err();
        assert!(matches!(err, ProtocolError::UnexpectedEof { .. }));
        assert!(matches!(
            read_varint(&[]).unwrap_err(),
            ProtocolError::UnexpectedEof { .. }
        ));
    }

    #[test]
    fn oversized_varint_is_rejected() {
        // Six continuation bytes: a malicious client trying to stall the parser.
        let err = read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).unwrap_err();
        assert!(matches!(err, ProtocolError::VarIntTooLong { .. }));

        let err = read_varlong(&[0x80; 11]).unwrap_err();
        assert!(matches!(err, ProtocolError::VarIntTooLong { .. }));
    }

    #[test]
    fn round_trips_over_the_whole_i32_range() {
        let mut out = Vec::with_capacity(MAX_VARINT_BYTES);
        // Step by a large prime so the sweep hits every byte-length class and
        // both signs without testing four billion values.
        let mut value = i32::MIN;
        loop {
            out.clear();
            write_varint(&mut out, value);
            let (decoded, used) = read_varint(&out).unwrap();
            assert_eq!(decoded, value);
            assert_eq!(used, out.len());
            match value.checked_add(7_654_321) {
                Some(next) => value = next,
                None => break,
            }
        }
    }
}
