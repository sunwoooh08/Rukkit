//! Packet ids, gathered in one place.
//!
//! Minecraft renumbers packet ids nearly every release, so isolating them here
//! makes a version bump a single-file edit instead of a hunt through the packet
//! structs. This mirrors how Paper handles the same problem with generated
//! mappings.
//!
//! # Verification status
//!
//! The handshaking, status, login and configuration tables have been stable
//! across the 1.20.2 → 26.x line and are treated as settled.
//!
//! The **play** table is marked provisional. Play ids are the ones that churn
//! between releases, and they cannot be confirmed from the wire format alone —
//! they have to be read out of the version's own packet report (or observed
//! against a real 26.2 client). The values below carry forward from the
//! preceding release line and must be validated before the play state is
//! trusted; [`play::PROVISIONAL`] records that, and the server logs a warning
//! when a connection reaches play.

pub mod handshaking {
    pub mod serverbound {
        pub const INTENTION: i32 = 0x00;
    }
}

pub mod status {
    pub mod serverbound {
        pub const STATUS_REQUEST: i32 = 0x00;
        pub const PING_REQUEST: i32 = 0x01;
    }
    pub mod clientbound {
        pub const STATUS_RESPONSE: i32 = 0x00;
        pub const PONG_RESPONSE: i32 = 0x01;
    }
}

pub mod login {
    pub mod serverbound {
        pub const HELLO: i32 = 0x00;
        pub const ENCRYPTION_RESPONSE: i32 = 0x01;
        pub const PLUGIN_RESPONSE: i32 = 0x02;
        pub const LOGIN_ACKNOWLEDGED: i32 = 0x03;
        pub const COOKIE_RESPONSE: i32 = 0x04;
    }
    pub mod clientbound {
        pub const LOGIN_DISCONNECT: i32 = 0x00;
        pub const ENCRYPTION_REQUEST: i32 = 0x01;
        pub const LOGIN_SUCCESS: i32 = 0x02;
        pub const SET_COMPRESSION: i32 = 0x03;
        pub const PLUGIN_REQUEST: i32 = 0x04;
        pub const COOKIE_REQUEST: i32 = 0x05;
    }
}

pub mod configuration {
    pub mod serverbound {
        pub const CLIENT_INFORMATION: i32 = 0x00;
        pub const COOKIE_RESPONSE: i32 = 0x01;
        pub const PLUGIN_MESSAGE: i32 = 0x02;
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const KEEP_ALIVE: i32 = 0x04;
        pub const PONG: i32 = 0x05;
        pub const RESOURCE_PACK_RESPONSE: i32 = 0x06;
        pub const SELECT_KNOWN_PACKS: i32 = 0x07;
    }
    pub mod clientbound {
        pub const COOKIE_REQUEST: i32 = 0x00;
        pub const PLUGIN_MESSAGE: i32 = 0x01;
        pub const DISCONNECT: i32 = 0x02;
        pub const FINISH_CONFIGURATION: i32 = 0x03;
        pub const KEEP_ALIVE: i32 = 0x04;
        pub const PING: i32 = 0x05;
        pub const RESET_CHAT: i32 = 0x06;
        pub const REGISTRY_DATA: i32 = 0x07;
        pub const REMOVE_RESOURCE_PACK: i32 = 0x08;
        pub const ADD_RESOURCE_PACK: i32 = 0x09;
        pub const STORE_COOKIE: i32 = 0x0A;
        pub const TRANSFER: i32 = 0x0B;
        pub const UPDATE_ENABLED_FEATURES: i32 = 0x0C;
        pub const UPDATE_TAGS: i32 = 0x0D;
        pub const SELECT_KNOWN_PACKS: i32 = 0x0E;
        pub const CUSTOM_REPORT_DETAILS: i32 = 0x0F;
        pub const SERVER_LINKS: i32 = 0x10;
    }
}

pub mod play {
    /// Set while the play ids below are unverified against 26.2.
    ///
    /// See the module documentation: these carry forward from the previous
    /// release line and need checking against the version's packet report.
    pub const PROVISIONAL: bool = true;

    pub mod serverbound {
        pub const ACCEPT_TELEPORTATION: i32 = 0x00;
        pub const CHAT_COMMAND: i32 = 0x05;
        pub const CHAT: i32 = 0x07;
        pub const CLIENT_TICK_END: i32 = 0x0B;
        pub const PLUGIN_MESSAGE: i32 = 0x15;
        pub const KEEP_ALIVE: i32 = 0x1A;
        pub const MOVE_PLAYER_POS: i32 = 0x1C;
        pub const MOVE_PLAYER_POS_ROT: i32 = 0x1D;
        pub const MOVE_PLAYER_ROT: i32 = 0x1E;
        pub const MOVE_PLAYER_STATUS_ONLY: i32 = 0x1F;
    }

    pub mod clientbound {
        pub const KEEP_ALIVE: i32 = 0x26;
        pub const CHUNK_DATA: i32 = 0x27;
        pub const GAME_EVENT: i32 = 0x22;
        pub const LOGIN: i32 = 0x2C;
        pub const DISCONNECT: i32 = 0x1D;
        pub const PLAYER_POSITION: i32 = 0x42;
        pub const SET_CHUNK_CACHE_CENTER: i32 = 0x58;
        pub const SET_DEFAULT_SPAWN_POSITION: i32 = 0x5A;
        pub const SYSTEM_CHAT: i32 = 0x73;
    }
}

#[cfg(test)]
mod tests {
    /// Two packets sharing an id in the same direction and state would make one
    /// of them silently unreachable, which is a very hard bug to spot at
    /// runtime. Check each table instead.
    fn assert_unique(name: &str, ids: &[(&str, i32)]) {
        let mut seen: Vec<(&str, i32)> = Vec::new();
        for &(packet, id) in ids {
            if let Some((other, _)) = seen.iter().find(|(_, other_id)| *other_id == id) {
                panic!("{name}: {packet} and {other} share id {id:#04x}");
            }
            seen.push((packet, id));
        }
    }

    #[test]
    fn login_ids_are_unique() {
        use super::login::*;
        assert_unique(
            "login serverbound",
            &[
                ("HELLO", serverbound::HELLO),
                ("ENCRYPTION_RESPONSE", serverbound::ENCRYPTION_RESPONSE),
                ("PLUGIN_RESPONSE", serverbound::PLUGIN_RESPONSE),
                ("LOGIN_ACKNOWLEDGED", serverbound::LOGIN_ACKNOWLEDGED),
                ("COOKIE_RESPONSE", serverbound::COOKIE_RESPONSE),
            ],
        );
        assert_unique(
            "login clientbound",
            &[
                ("LOGIN_DISCONNECT", clientbound::LOGIN_DISCONNECT),
                ("ENCRYPTION_REQUEST", clientbound::ENCRYPTION_REQUEST),
                ("LOGIN_SUCCESS", clientbound::LOGIN_SUCCESS),
                ("SET_COMPRESSION", clientbound::SET_COMPRESSION),
                ("PLUGIN_REQUEST", clientbound::PLUGIN_REQUEST),
                ("COOKIE_REQUEST", clientbound::COOKIE_REQUEST),
            ],
        );
    }

    #[test]
    fn configuration_ids_are_unique() {
        use super::configuration::*;
        assert_unique(
            "configuration serverbound",
            &[
                ("CLIENT_INFORMATION", serverbound::CLIENT_INFORMATION),
                ("COOKIE_RESPONSE", serverbound::COOKIE_RESPONSE),
                ("PLUGIN_MESSAGE", serverbound::PLUGIN_MESSAGE),
                ("FINISH_CONFIGURATION", serverbound::FINISH_CONFIGURATION),
                ("KEEP_ALIVE", serverbound::KEEP_ALIVE),
                ("PONG", serverbound::PONG),
                (
                    "RESOURCE_PACK_RESPONSE",
                    serverbound::RESOURCE_PACK_RESPONSE,
                ),
                ("SELECT_KNOWN_PACKS", serverbound::SELECT_KNOWN_PACKS),
            ],
        );
        assert_unique(
            "configuration clientbound",
            &[
                ("COOKIE_REQUEST", clientbound::COOKIE_REQUEST),
                ("PLUGIN_MESSAGE", clientbound::PLUGIN_MESSAGE),
                ("DISCONNECT", clientbound::DISCONNECT),
                ("FINISH_CONFIGURATION", clientbound::FINISH_CONFIGURATION),
                ("KEEP_ALIVE", clientbound::KEEP_ALIVE),
                ("PING", clientbound::PING),
                ("RESET_CHAT", clientbound::RESET_CHAT),
                ("REGISTRY_DATA", clientbound::REGISTRY_DATA),
                ("REMOVE_RESOURCE_PACK", clientbound::REMOVE_RESOURCE_PACK),
                ("ADD_RESOURCE_PACK", clientbound::ADD_RESOURCE_PACK),
                ("STORE_COOKIE", clientbound::STORE_COOKIE),
                ("TRANSFER", clientbound::TRANSFER),
                (
                    "UPDATE_ENABLED_FEATURES",
                    clientbound::UPDATE_ENABLED_FEATURES,
                ),
                ("UPDATE_TAGS", clientbound::UPDATE_TAGS),
                ("SELECT_KNOWN_PACKS", clientbound::SELECT_KNOWN_PACKS),
                ("CUSTOM_REPORT_DETAILS", clientbound::CUSTOM_REPORT_DETAILS),
                ("SERVER_LINKS", clientbound::SERVER_LINKS),
            ],
        );
    }

    #[test]
    fn play_ids_are_unique() {
        use super::play::*;
        assert_unique(
            "play serverbound",
            &[
                ("ACCEPT_TELEPORTATION", serverbound::ACCEPT_TELEPORTATION),
                ("CHAT_COMMAND", serverbound::CHAT_COMMAND),
                ("CHAT", serverbound::CHAT),
                ("CLIENT_TICK_END", serverbound::CLIENT_TICK_END),
                ("PLUGIN_MESSAGE", serverbound::PLUGIN_MESSAGE),
                ("KEEP_ALIVE", serverbound::KEEP_ALIVE),
                ("MOVE_PLAYER_POS", serverbound::MOVE_PLAYER_POS),
                ("MOVE_PLAYER_POS_ROT", serverbound::MOVE_PLAYER_POS_ROT),
                ("MOVE_PLAYER_ROT", serverbound::MOVE_PLAYER_ROT),
                (
                    "MOVE_PLAYER_STATUS_ONLY",
                    serverbound::MOVE_PLAYER_STATUS_ONLY,
                ),
            ],
        );
        assert_unique(
            "play clientbound",
            &[
                ("KEEP_ALIVE", clientbound::KEEP_ALIVE),
                ("CHUNK_DATA", clientbound::CHUNK_DATA),
                ("GAME_EVENT", clientbound::GAME_EVENT),
                ("LOGIN", clientbound::LOGIN),
                ("DISCONNECT", clientbound::DISCONNECT),
                ("PLAYER_POSITION", clientbound::PLAYER_POSITION),
                (
                    "SET_CHUNK_CACHE_CENTER",
                    clientbound::SET_CHUNK_CACHE_CENTER,
                ),
                (
                    "SET_DEFAULT_SPAWN_POSITION",
                    clientbound::SET_DEFAULT_SPAWN_POSITION,
                ),
                ("SYSTEM_CHAT", clientbound::SYSTEM_CHAT),
            ],
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn play_table_is_still_flagged_provisional() {
        // A reminder in test form: clear this flag in the same change that
        // verifies the play ids against 26.2, not before.
        assert!(super::play::PROVISIONAL);
    }
}
