//! Minecraft Java Edition wire protocol for 26.2 (protocol 776).
//!
//! This crate is deliberately free of I/O and of any game logic: it turns bytes
//! into packet structs and back. That keeps the hot decode path testable in
//! isolation and lets the server crate decide its own threading model.

pub mod codec;
pub mod crypt;
pub mod error;
pub mod nbt;
pub mod packets;
pub mod reader;
pub mod text;
pub mod types;
pub mod varint;
pub mod version;
pub mod writer;

pub use error::{ProtocolError, Result};
pub use reader::PacketReader;
pub use text::Component;
pub use types::{BlockPos, ChunkPos, Difficulty, GameMode};
pub use version::{DATA_VERSION, MINECRAFT_VERSION, PROTOCOL_VERSION};
pub use writer::PacketWrite;

/// Connection state, which determines how a packet id is interpreted.
///
/// The client and server must agree on the current state at all times; a
/// mismatch shows up as a nonsensical packet id rather than as a clean error,
/// which is why every transition is driven explicitly by an acknowledgement
/// packet rather than inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum State {
    #[default]
    Handshaking,
    Status,
    Login,
    /// Added in 1.20.2: registries and resource packs are negotiated here,
    /// before play begins and whenever the server needs to reconfigure.
    Configuration,
    Play,
}

impl State {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Handshaking => "handshaking",
            Self::Status => "status",
            Self::Login => "login",
            Self::Configuration => "configuration",
            Self::Play => "play",
        }
    }
}
