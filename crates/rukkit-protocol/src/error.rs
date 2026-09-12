//! Error type shared by every decoding and encoding path.

/// Anything that can go wrong while reading or writing the Minecraft wire
/// protocol.
///
/// Decoding errors are *expected* at runtime: a malicious or buggy client can
/// send anything at all, so every variant here must be recoverable by dropping
/// the offending connection rather than by panicking.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("unexpected end of packet: needed {needed} more byte(s), had {available}")]
    UnexpectedEof { needed: usize, available: usize },

    #[error("VarInt/VarLong longer than the {max}-byte maximum")]
    VarIntTooLong { max: usize },

    #[error("string is {len} bytes, over the {max}-byte limit")]
    StringTooLong { len: usize, max: usize },

    #[error("string is {len} characters, over the {max}-character limit")]
    StringTooManyChars { len: usize, max: usize },

    #[error("invalid UTF-8 in string")]
    InvalidUtf8,

    #[error("negative length prefix: {0}")]
    NegativeLength(i32),

    #[error("length prefix {len} exceeds the {max} element limit")]
    LengthLimitExceeded { len: usize, max: usize },

    #[error("frame is {len} bytes, over the {max}-byte limit")]
    FrameTooLarge { len: usize, max: usize },

    #[error("{value} is not a valid {kind}")]
    InvalidEnum { kind: &'static str, value: i64 },

    #[error("unknown packet id {id:#04x} in state {state}")]
    UnknownPacket { state: &'static str, id: i32 },

    #[error("{trailing} trailing byte(s) after packet {id:#04x} in state {state}")]
    TrailingBytes {
        state: &'static str,
        id: i32,
        trailing: usize,
    },

    #[error("malformed NBT: {0}")]
    Nbt(&'static str),

    #[error("NBT nested more than {max} levels deep")]
    NbtTooDeep { max: usize },

    #[error("zlib error: {0}")]
    Compression(String),

    /// The peer claimed a decompressed size that we refuse to allocate. Without
    /// this check a 5-byte packet could ask us to reserve 2 GiB (a zip bomb).
    #[error("declared uncompressed size {len} exceeds the {max}-byte limit")]
    DecompressedTooLarge { len: usize, max: usize },

    /// Vanilla requires that anything below the compression threshold is sent
    /// uncompressed, so a compressed frame under it is a protocol violation.
    #[error(
        "packet of {size} bytes was compressed despite being under the {threshold}-byte threshold"
    )]
    BadlyCompressed { size: usize, threshold: usize },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, ProtocolError>;

impl From<std::io::Error> for ProtocolError {
    fn from(value: std::io::Error) -> Self {
        ProtocolError::Compression(value.to_string())
    }
}
