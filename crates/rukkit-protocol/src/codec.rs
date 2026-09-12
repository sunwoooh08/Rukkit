//! Frame codec: length prefixing, optional zlib compression, optional
//! encryption.
//!
//! A frame is a VarInt length followed by that many bytes. Once compression is
//! negotiated the body gains a second VarInt holding the uncompressed size,
//! where `0` means "this one was sent as-is because it was under the
//! threshold". Once encryption is negotiated the entire byte stream — length
//! prefix included — is enciphered.
//!
//! Ordering matters and is the usual source of bugs: the `Set Compression`
//! packet itself is sent *uncompressed*, and the first encrypted byte is the
//! one immediately after `Encryption Response`. Callers therefore enable each
//! feature only after the switching packet has been handed to the codec.

use bytes::{Buf, Bytes, BytesMut};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

use crate::crypt::{Decryptor, Encryptor};
use crate::error::{ProtocolError, Result};
use crate::varint::{self, varint_len};
use crate::writer::PacketWrite;

/// Hard cap on a single frame, matching vanilla's packet size limit.
pub const MAX_FRAME_LEN: usize = 2 * 1024 * 1024;

/// Threshold vanilla uses by default: packets at or above this are compressed.
pub const DEFAULT_COMPRESSION_THRESHOLD: usize = 256;

/// zlib level. Level 6 is zlib's own default and the same trade-off vanilla
/// makes; the marginal ratio above it costs far more CPU than the bandwidth is
/// worth on a chunk-heavy send path.
const COMPRESSION_LEVEL: u32 = 6;

/// Splits an incoming byte stream into packet bodies.
#[derive(Debug)]
pub struct Decoder {
    buffer: BytesMut,
    compression: Option<usize>,
    decryptor: Option<Decryptor>,
    decompressor: Decompress,
    max_frame: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::with_capacity(8 * 1024),
            compression: None,
            decryptor: None,
            decompressor: Decompress::new(true),
            max_frame: MAX_FRAME_LEN,
        }
    }

    /// Turns on compression with the negotiated threshold.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.compression = Some(threshold);
    }

    /// Turns on decryption. Every byte fed in after this call is deciphered.
    pub fn enable_encryption(&mut self, decryptor: Decryptor) {
        self.decryptor = Some(decryptor);
    }

    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.decryptor.is_some()
    }

    /// Bytes buffered but not yet forming a complete frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Feeds freshly read socket bytes, deciphering them in place first.
    pub fn feed(&mut self, data: &mut [u8]) {
        if let Some(decryptor) = &mut self.decryptor {
            decryptor.decrypt(data);
        }
        self.buffer.extend_from_slice(data);
    }

    /// Pops the next complete packet body, or `Ok(None)` if more bytes are
    /// needed.
    ///
    /// The returned buffer starts at the packet id.
    pub fn decode(&mut self) -> Result<Option<Bytes>> {
        let (frame_len, header_len) = match varint::read_varint(&self.buffer) {
            Ok(v) => v,
            // A partial length prefix simply means "read more from the socket".
            Err(ProtocolError::UnexpectedEof { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };

        if frame_len < 0 {
            return Err(ProtocolError::NegativeLength(frame_len));
        }
        let frame_len = frame_len as usize;
        if frame_len > self.max_frame {
            return Err(ProtocolError::FrameTooLarge {
                len: frame_len,
                max: self.max_frame,
            });
        }
        if self.buffer.len() < header_len + frame_len {
            return Ok(None);
        }

        self.buffer.advance(header_len);
        let mut frame = self.buffer.split_to(frame_len);

        let Some(threshold) = self.compression else {
            return Ok(Some(frame.freeze()));
        };

        let (data_len, used) = varint::read_varint(&frame)?;
        if data_len == 0 {
            // Sent uncompressed; hand back the same allocation.
            frame.advance(used);
            return Ok(Some(frame.freeze()));
        }
        if data_len < 0 {
            return Err(ProtocolError::NegativeLength(data_len));
        }
        let data_len = data_len as usize;
        if data_len < threshold {
            return Err(ProtocolError::BadlyCompressed {
                size: data_len,
                threshold,
            });
        }
        if data_len > self.max_frame {
            return Err(ProtocolError::DecompressedTooLarge {
                len: data_len,
                max: self.max_frame,
            });
        }

        let mut out = Vec::new();
        decompress_exact(&mut self.decompressor, &frame[used..], data_len, &mut out)?;
        Ok(Some(Bytes::from(out)))
    }
}

/// Serializes packet bodies into framed, optionally compressed and encrypted
/// bytes.
#[derive(Debug)]
pub struct Encoder {
    compression: Option<usize>,
    encryptor: Option<Encryptor>,
    compressor: Compress,
    scratch: Vec<u8>,
    max_frame: usize,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            compression: None,
            encryptor: None,
            compressor: Compress::new(Compression::new(COMPRESSION_LEVEL), true),
            scratch: Vec::with_capacity(8 * 1024),
            max_frame: MAX_FRAME_LEN,
        }
    }

    /// Turns on compression. Call this *after* writing `Set Compression`.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.compression = Some(threshold);
    }

    /// Turns on encryption. Call this *after* writing `Encryption Request`.
    pub fn enable_encryption(&mut self, encryptor: Encryptor) {
        self.encryptor = Some(encryptor);
    }

    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.encryptor.is_some()
    }

    /// Appends one framed packet to `out`.
    ///
    /// `packet` must already contain the VarInt packet id followed by the body.
    pub fn encode(&mut self, packet: &[u8], out: &mut BytesMut) -> Result<()> {
        let start = out.len();

        match self.compression {
            None => {
                if packet.len() > self.max_frame {
                    return Err(ProtocolError::FrameTooLarge {
                        len: packet.len(),
                        max: self.max_frame,
                    });
                }
                out.write_varint(packet.len() as i32);
                out.write_bytes(packet);
            }
            Some(threshold) if packet.len() < threshold => {
                // Below the threshold the body is passed through, marked by a
                // zero uncompressed-size field (one byte as a VarInt).
                out.write_varint((1 + packet.len()) as i32);
                out.write_varint(0);
                out.write_bytes(packet);
            }
            Some(_) => {
                // `scratch` is moved out so the compressor can borrow `self`
                // mutably alongside it; it is always put straight back.
                let mut scratch = std::mem::take(&mut self.scratch);
                let result = compress_into(&mut self.compressor, packet, &mut scratch);
                if result.is_err() {
                    self.scratch = scratch;
                    result?;
                    unreachable!();
                }

                let frame_len = varint_len(packet.len() as i32) + scratch.len();
                if frame_len > self.max_frame {
                    self.scratch = scratch;
                    return Err(ProtocolError::FrameTooLarge {
                        len: frame_len,
                        max: self.max_frame,
                    });
                }
                out.write_varint(frame_len as i32);
                out.write_varint(packet.len() as i32);
                out.write_bytes(&scratch);
                self.scratch = scratch;
            }
        }

        if let Some(encryptor) = &mut self.encryptor {
            encryptor.encrypt(&mut out[start..]);
        }
        Ok(())
    }
}

fn compress_into(compressor: &mut Compress, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
    compressor.reset();
    out.clear();
    out.reserve(input.len() / 2 + 64);

    loop {
        let consumed = compressor.total_in() as usize;
        let before = out.len();
        let status = compressor
            .compress_vec(&input[consumed..], out, FlushCompress::Finish)
            .map_err(|e| ProtocolError::Compression(e.to_string()))?;

        if status == Status::StreamEnd {
            return Ok(());
        }
        if out.len() == before {
            // Output buffer is full; give it more room and continue.
            let grow = out.capacity().max(64);
            out.reserve(grow);
        }
    }
}

/// Inflates `input` and requires the result to be exactly `expected` bytes.
///
/// The output is capped at exactly the declared size, so a stream that tries to
/// expand beyond what the sender promised is rejected rather than allowed to
/// exhaust memory.
fn decompress_exact(
    decompressor: &mut Decompress,
    input: &[u8],
    expected: usize,
    out: &mut Vec<u8>,
) -> Result<()> {
    decompressor.reset(true);
    out.clear();
    out.reserve_exact(expected);

    loop {
        let consumed = decompressor.total_in() as usize;
        if consumed > input.len() {
            return Err(ProtocolError::Compression(
                "zlib stream consumed more than was supplied".to_owned(),
            ));
        }
        let before = out.len();
        let status = decompressor
            .decompress_vec(&input[consumed..], out, FlushDecompress::Finish)
            .map_err(|e| ProtocolError::Compression(e.to_string()))?;

        if status == Status::StreamEnd {
            break;
        }
        if out.len() == before {
            // No forward progress with no room left means the sender lied about
            // the uncompressed size, or the stream is truncated.
            return Err(ProtocolError::Compression(
                "truncated zlib stream, or larger than its declared size".to_owned(),
            ));
        }
    }

    if out.len() != expected {
        return Err(ProtocolError::Compression(format!(
            "zlib stream produced {} bytes, declared {expected}",
            out.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(encoder: &mut Encoder, decoder: &mut Decoder, packet: &[u8]) -> Vec<u8> {
        let mut wire = BytesMut::new();
        encoder.encode(packet, &mut wire).expect("encode");
        let mut bytes = wire.to_vec();
        decoder.feed(&mut bytes);
        let decoded = decoder.decode().expect("decode").expect("complete frame");
        decoded.to_vec()
    }

    #[test]
    fn uncompressed_round_trip() {
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        let packet = b"\x00hello world";
        assert_eq!(round_trip(&mut enc, &mut dec, packet), packet);
    }

    #[test]
    fn frame_layout_is_length_then_body() {
        let mut enc = Encoder::new();
        let mut out = BytesMut::new();
        enc.encode(b"\x00abc", &mut out).unwrap();
        assert_eq!(&out[..], &[4, 0x00, b'a', b'b', b'c']);
    }

    #[test]
    fn partial_frames_are_buffered_until_complete() {
        let mut enc = Encoder::new();
        let mut out = BytesMut::new();
        enc.encode(b"\x00some payload here", &mut out).unwrap();
        let wire = out.to_vec();

        let mut dec = Decoder::new();
        // Feed one byte at a time; only the final byte may yield a packet.
        for (i, byte) in wire.iter().enumerate() {
            let mut chunk = [*byte];
            dec.feed(&mut chunk);
            let got = dec.decode().unwrap();
            if i + 1 == wire.len() {
                assert_eq!(got.unwrap().to_vec(), b"\x00some payload here");
            } else {
                assert!(got.is_none(), "yielded a packet after {} bytes", i + 1);
            }
        }
    }

    #[test]
    fn multiple_packets_in_one_read_are_all_decoded() {
        let mut enc = Encoder::new();
        let mut out = BytesMut::new();
        for payload in [&b"\x00one"[..], b"\x01two", b"\x02three"] {
            enc.encode(payload, &mut out).unwrap();
        }

        let mut dec = Decoder::new();
        let mut bytes = out.to_vec();
        dec.feed(&mut bytes);

        assert_eq!(dec.decode().unwrap().unwrap().to_vec(), b"\x00one");
        assert_eq!(dec.decode().unwrap().unwrap().to_vec(), b"\x01two");
        assert_eq!(dec.decode().unwrap().unwrap().to_vec(), b"\x02three");
        assert!(dec.decode().unwrap().is_none());
    }

    #[test]
    fn small_packets_pass_through_uncompressed() {
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        enc.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);

        let packet = b"\x00tiny";
        let mut out = BytesMut::new();
        enc.encode(packet, &mut out).unwrap();
        // frame length, then a zero uncompressed-size marker, then the body.
        assert_eq!(out[1], 0);
        assert_eq!(
            round_trip(&mut Encoder::new(), &mut Decoder::new(), packet),
            packet
        );
    }

    #[test]
    fn large_packets_are_compressed_and_shrink() {
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        enc.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);

        // Highly repetitive, as chunk data is in practice.
        let mut packet = vec![0x22u8];
        packet.extend(std::iter::repeat_n(b'A', 4096));

        let mut out = BytesMut::new();
        enc.encode(&packet, &mut out).unwrap();
        assert!(
            out.len() < packet.len() / 4,
            "compressed {} bytes to {}",
            packet.len(),
            out.len()
        );

        let mut bytes = out.to_vec();
        dec.feed(&mut bytes);
        assert_eq!(dec.decode().unwrap().unwrap().to_vec(), packet);
    }

    #[test]
    fn compressed_codec_handles_a_long_sequence() {
        // Exercises compressor/decompressor reset across many packets, mixing
        // both sides of the threshold.
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        enc.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);

        for i in 0..200usize {
            let len = if i % 2 == 0 { 10 } else { 1000 };
            let packet: Vec<u8> = (0..len).map(|n| (n % 251) as u8).collect();
            assert_eq!(
                round_trip(&mut enc, &mut dec, &packet),
                packet,
                "packet {i}"
            );
        }
    }

    #[test]
    fn encrypted_round_trip_across_many_packets() {
        let secret = [7u8; 16];
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        enc.enable_encryption(Encryptor::new(&secret).unwrap());
        dec.enable_encryption(Decryptor::new(&secret).unwrap());

        for i in 0..50u8 {
            let packet = vec![i; 64];
            assert_eq!(
                round_trip(&mut enc, &mut dec, &packet),
                packet,
                "packet {i}"
            );
        }
    }

    #[test]
    fn encryption_and_compression_compose() {
        let secret = [3u8; 16];
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        enc.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        enc.enable_encryption(Encryptor::new(&secret).unwrap());
        dec.enable_encryption(Decryptor::new(&secret).unwrap());

        for i in 0..20usize {
            let packet: Vec<u8> = (0..500).map(|n| ((n + i) % 251) as u8).collect();
            assert_eq!(
                round_trip(&mut enc, &mut dec, &packet),
                packet,
                "packet {i}"
            );
        }
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let mut dec = Decoder::new();
        let mut bytes = Vec::new();
        varint::write_varint(&mut bytes, (MAX_FRAME_LEN + 1) as i32);
        dec.feed(&mut bytes);
        assert!(matches!(
            dec.decode().unwrap_err(),
            ProtocolError::FrameTooLarge { .. }
        ));
    }

    #[test]
    fn negative_length_prefix_is_rejected() {
        let mut dec = Decoder::new();
        let mut bytes = Vec::new();
        varint::write_varint(&mut bytes, -1);
        dec.feed(&mut bytes);
        assert!(matches!(
            dec.decode().unwrap_err(),
            ProtocolError::NegativeLength(-1)
        ));
    }

    #[test]
    fn packet_compressed_below_the_threshold_is_rejected() {
        // Vanilla requires sub-threshold packets to be sent raw; a peer that
        // compresses them anyway is violating the protocol.
        let mut dec = Decoder::new();
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);

        let body = [0x78u8, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
        let mut inner = Vec::new();
        inner.write_varint(10); // declared uncompressed size, under 256
        inner.extend_from_slice(&body);

        let mut bytes = Vec::new();
        bytes.write_varint(inner.len() as i32);
        bytes.extend_from_slice(&inner);

        dec.feed(&mut bytes);
        assert!(matches!(
            dec.decode().unwrap_err(),
            ProtocolError::BadlyCompressed { .. }
        ));
    }

    #[test]
    fn zip_bomb_declaring_a_huge_size_is_rejected() {
        let mut dec = Decoder::new();
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);

        let mut inner = Vec::new();
        inner.write_varint((MAX_FRAME_LEN + 1) as i32);
        inner.extend_from_slice(&[0x78, 0x9c, 0x01]);

        let mut bytes = Vec::new();
        bytes.write_varint(inner.len() as i32);
        bytes.extend_from_slice(&inner);

        dec.feed(&mut bytes);
        assert!(matches!(
            dec.decode().unwrap_err(),
            ProtocolError::DecompressedTooLarge { .. }
        ));
    }

    #[test]
    fn stream_smaller_than_its_declared_size_is_rejected() {
        // Compress 300 bytes but claim 4096 on the wire.
        let real: Vec<u8> = std::iter::repeat_n(b'Z', 300).collect();
        let mut compressed = Vec::new();
        compress_into(
            &mut Compress::new(Compression::new(COMPRESSION_LEVEL), true),
            &real,
            &mut compressed,
        )
        .unwrap();

        let mut inner = Vec::new();
        inner.write_varint(4096);
        inner.extend_from_slice(&compressed);

        let mut bytes = Vec::new();
        bytes.write_varint(inner.len() as i32);
        bytes.extend_from_slice(&inner);

        let mut dec = Decoder::new();
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.feed(&mut bytes);
        assert!(matches!(
            dec.decode().unwrap_err(),
            ProtocolError::Compression(_)
        ));
    }

    #[test]
    fn corrupt_zlib_payload_is_rejected() {
        let mut inner = Vec::new();
        inner.write_varint(1024);
        inner.extend_from_slice(&[0xFF; 32]);

        let mut bytes = Vec::new();
        bytes.write_varint(inner.len() as i32);
        bytes.extend_from_slice(&inner);

        let mut dec = Decoder::new();
        dec.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        dec.feed(&mut bytes);
        assert!(dec.decode().is_err());
    }

    #[test]
    fn empty_packet_round_trips() {
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        assert_eq!(round_trip(&mut enc, &mut dec, b""), b"");
    }
}
