//! Handshaking state: a single packet that selects what happens next.

use crate::error::{ProtocolError, Result};
use crate::packets::{ids, ServerboundPacket};
use crate::reader::PacketReader;
use crate::writer::PacketWrite;

/// Maximum length vanilla allows for the address field.
pub const MAX_ADDRESS_LEN: usize = 255;

/// What the client wants to do after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
    /// A transfer from another server (1.20.5+). Handled like a login, but the
    /// server may reject it when `accepts-transfers` is off.
    Transfer,
}

impl NextState {
    pub fn from_id(id: i32) -> Result<Self> {
        Ok(match id {
            1 => Self::Status,
            2 => Self::Login,
            3 => Self::Transfer,
            other => {
                return Err(ProtocolError::InvalidEnum {
                    kind: "NextState",
                    value: i64::from(other),
                })
            }
        })
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Status => 1,
            Self::Login => 2,
            Self::Transfer => 3,
        }
    }
}

/// The opening packet of every connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intention {
    pub protocol_version: i32,
    /// Hostname the client connected to. Proxies append their own data here,
    /// so it is not trustworthy on its own.
    pub server_address: String,
    pub server_port: u16,
    pub next_state: NextState,
}

impl Intention {
    /// Writes the body. Present for tests and for client-side tooling; the
    /// server only ever decodes this packet.
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_varint(self.protocol_version);
        out.write_string(&self.server_address);
        out.write_u16(self.server_port);
        out.write_varint(self.next_state.id());
    }
}

impl ServerboundPacket for Intention {
    const ID: i32 = ids::handshaking::serverbound::INTENTION;
    const NAME: &'static str = "Intention";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            protocol_version: r.read_varint()?,
            server_address: r.read_string(MAX_ADDRESS_LEN)?,
            server_port: r.read_u16()?,
            next_state: NextState::from_id(r.read_varint()?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::decode_exact;

    #[test]
    fn round_trips() {
        let original = Intention {
            protocol_version: crate::version::PROTOCOL_VERSION,
            server_address: "mc.example.com".into(),
            server_port: 25565,
            next_state: NextState::Login,
        };
        let mut body = Vec::new();
        original.encode_body(&mut body);
        assert_eq!(
            decode_exact::<Intention>(&body, "handshaking").unwrap(),
            original
        );
    }

    #[test]
    fn all_next_states_round_trip() {
        for state in [NextState::Status, NextState::Login, NextState::Transfer] {
            assert_eq!(NextState::from_id(state.id()).unwrap(), state);
        }
    }

    #[test]
    fn unknown_next_state_is_rejected() {
        assert!(NextState::from_id(0).is_err());
        assert!(NextState::from_id(4).is_err());
    }

    #[test]
    fn overlong_address_is_rejected() {
        let mut body = Vec::new();
        body.write_varint(776);
        body.write_string(&"a".repeat(MAX_ADDRESS_LEN + 1));
        body.write_u16(25565);
        body.write_varint(1);
        assert!(decode_exact::<Intention>(&body, "handshaking").is_err());
    }
}
