//! Packet definitions, grouped by connection state.
//!
//! Each packet is a plain struct with an id and a body codec. There is no
//! registry of boxed trait objects: the server matches on the id and decodes
//! into a concrete type, so the whole path monomorphises and a packet that is
//! never used costs nothing.

pub mod config;
pub mod handshake;
pub mod ids;
pub mod login;
pub mod play;
pub mod status;

use crate::error::Result;
use crate::reader::PacketReader;
use crate::writer::PacketWrite;

/// A packet this server sends.
pub trait ClientboundPacket {
    /// VarInt packet id within its state.
    const ID: i32;
    /// Name used in logs and errors.
    const NAME: &'static str;

    /// Writes the body, excluding the id.
    fn encode_body(&self, out: &mut Vec<u8>);
}

/// A packet this server receives.
pub trait ServerboundPacket: Sized {
    const ID: i32;
    const NAME: &'static str;

    /// Reads the body, with the id already consumed.
    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self>;
}

/// Serializes a packet into `out` as id followed by body.
///
/// `out` is cleared first so a single scratch buffer can be reused for the life
/// of a connection, which keeps the send path allocation-free in steady state.
pub fn encode<P: ClientboundPacket>(packet: &P, out: &mut Vec<u8>) {
    out.clear();
    out.write_varint(P::ID);
    packet.encode_body(out);
}

/// Decodes a packet body and requires it to be consumed exactly.
pub fn decode_exact<P: ServerboundPacket>(body: &[u8], state: &'static str) -> Result<P> {
    let mut r = PacketReader::new(body);
    let packet = P::decode_body(&mut r)?;
    r.expect_finished(state, P::ID)?;
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::handshake::Intention;

    #[test]
    fn encode_clears_the_scratch_buffer() {
        // A reused send buffer must not leak bytes from the previous packet.
        let mut scratch = vec![0xFFu8; 32];
        encode(&config::FinishConfiguration, &mut scratch);
        assert_eq!(
            scratch,
            vec![config::FinishConfiguration::ID as u8],
            "stale bytes survived encode"
        );
    }

    #[test]
    fn decode_exact_rejects_trailing_bytes() {
        let mut body = Vec::new();
        let original = Intention {
            protocol_version: 776,
            server_address: "localhost".into(),
            server_port: 25565,
            next_state: handshake::NextState::Status,
        };
        original.encode_body(&mut body);
        assert_eq!(
            decode_exact::<Intention>(&body, "handshaking").unwrap(),
            original
        );

        body.push(0);
        assert!(decode_exact::<Intention>(&body, "handshaking").is_err());
    }
}
