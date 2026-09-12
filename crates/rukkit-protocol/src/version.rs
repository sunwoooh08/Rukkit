//! The game version this build speaks.
//!
//! Minecraft moved to year-based version numbers in 2026: `26.1` was the first
//! game drop of that year (replacing what would have been `1.22`), and `26.2`
//! followed on 2026-06-16.

/// Network protocol number advertised in the handshake and status response.
pub const PROTOCOL_VERSION: i32 = 776;

/// Human-readable version shown in the server list.
pub const MINECRAFT_VERSION: &str = "26.2";

/// World storage data version, written into chunk and level NBT so that vanilla
/// and other tools can tell how to read worlds this server saved.
pub const DATA_VERSION: i32 = 4903;

/// Name reported in the server brand plugin message (`minecraft:brand`).
pub const SERVER_BRAND: &str = "Rukkit";

/// True when `protocol` is the exact version this build implements.
///
/// The Minecraft protocol carries no compatibility guarantees between
/// versions — packet ids are renumbered nearly every release — so anything
/// other than an exact match has to be rejected at handshake time.
#[inline]
pub const fn is_supported(protocol: i32) -> bool {
    protocol == PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_targeted_protocol_is_accepted() {
        assert!(is_supported(PROTOCOL_VERSION));
        assert!(!is_supported(PROTOCOL_VERSION - 1));
        assert!(!is_supported(PROTOCOL_VERSION + 1));
        assert!(!is_supported(0));
    }
}
