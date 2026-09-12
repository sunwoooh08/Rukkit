//! Play state.
//!
//! # Provisional
//!
//! Play packet ids and field layouts change between releases more than any
//! other part of the protocol, and they cannot be derived from the wire format
//! — they come from the version's own packet report. The definitions here carry
//! forward from the preceding release line; see [`super::ids::play`]. The frame,
//! compression, encryption, NBT and chunk layers below them are version-stable
//! and fully exercised, so validating this module against a real 26.2 client is
//! the remaining step for play, not a rewrite.

use crate::error::Result;
use crate::nbt;
use crate::packets::{ids, ClientboundPacket, ServerboundPacket};
use crate::reader::PacketReader;
use crate::text::Component;
use crate::types::{BlockPos, GameMode};
use crate::writer::PacketWrite;

const MAX_COMMAND_LEN: usize = 256;
const MAX_DIMENSIONS: usize = 1024;

// ---------------------------------------------------------------------------
// Clientbound
// ---------------------------------------------------------------------------

/// Join game: everything the client needs to start rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub entity_id: i32,
    pub hardcore: bool,
    pub dimension_names: Vec<String>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub do_limited_crafting: bool,
    /// Index into the `minecraft:dimension_type` registry sent in configuration.
    pub dimension_type: i32,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub game_mode: GameMode,
    /// `-1` when there is no previous mode.
    pub previous_game_mode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub enforces_secure_chat: bool,
}

impl ClientboundPacket for Login {
    const ID: i32 = ids::play::clientbound::LOGIN;
    const NAME: &'static str = "Login";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_i32(self.entity_id);
        out.write_bool(self.hardcore);
        out.write_varint(self.dimension_names.len() as i32);
        for name in &self.dimension_names {
            out.write_identifier(name);
        }
        out.write_varint(self.max_players);
        out.write_varint(self.view_distance);
        out.write_varint(self.simulation_distance);
        out.write_bool(self.reduced_debug_info);
        out.write_bool(self.enable_respawn_screen);
        out.write_bool(self.do_limited_crafting);
        out.write_varint(self.dimension_type);
        out.write_identifier(&self.dimension_name);
        out.write_i64(self.hashed_seed);
        out.write_u8(self.game_mode.id());
        out.write_i8(self.previous_game_mode);
        out.write_bool(self.is_debug);
        out.write_bool(self.is_flat);
        match &self.death_location {
            Some((dimension, pos)) => {
                out.write_bool(true);
                out.write_identifier(dimension);
                out.write_position(*pos);
            }
            None => out.write_bool(false),
        }
        out.write_varint(self.portal_cooldown);
        out.write_varint(self.sea_level);
        out.write_bool(self.enforces_secure_chat);
    }
}

impl Login {
    /// Decoding exists for round-trip tests and proxy use.
    pub fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        let entity_id = r.read_i32()?;
        let hardcore = r.read_bool()?;
        let dimension_names = r.read_array(MAX_DIMENSIONS, PacketReader::read_identifier)?;
        Ok(Self {
            entity_id,
            hardcore,
            dimension_names,
            max_players: r.read_varint()?,
            view_distance: r.read_varint()?,
            simulation_distance: r.read_varint()?,
            reduced_debug_info: r.read_bool()?,
            enable_respawn_screen: r.read_bool()?,
            do_limited_crafting: r.read_bool()?,
            dimension_type: r.read_varint()?,
            dimension_name: r.read_identifier()?,
            hashed_seed: r.read_i64()?,
            game_mode: GameMode::from_id(i32::from(r.read_u8()?))?,
            previous_game_mode: r.read_i8()?,
            is_debug: r.read_bool()?,
            is_flat: r.read_bool()?,
            death_location: r.read_option(|r| {
                let dimension = r.read_identifier()?;
                let pos = r.read_position()?;
                Ok((dimension, pos))
            })?,
            portal_cooldown: r.read_varint()?,
            sea_level: r.read_varint()?,
            enforces_secure_chat: r.read_bool()?,
        })
    }
}

/// Liveness probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

impl ClientboundPacket for KeepAlive {
    const ID: i32 = ids::play::clientbound::KEEP_ALIVE;
    const NAME: &'static str = "KeepAlive";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_i64(self.id);
    }
}

impl ServerboundPacket for KeepAlive {
    const ID: i32 = ids::play::serverbound::KEEP_ALIVE;
    const NAME: &'static str = "KeepAlive";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self { id: r.read_i64()? })
    }
}

/// Kick with a reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    pub reason: Component,
}

impl ClientboundPacket for Disconnect {
    const ID: i32 = ids::play::clientbound::DISCONNECT;
    const NAME: &'static str = "Disconnect";

    fn encode_body(&self, out: &mut Vec<u8>) {
        nbt::write_network(&self.reason.to_nbt(), out);
    }
}

/// A server message, shown in chat or as an action-bar overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemChat {
    pub content: Component,
    pub overlay: bool,
}

impl ClientboundPacket for SystemChat {
    const ID: i32 = ids::play::clientbound::SYSTEM_CHAT;
    const NAME: &'static str = "SystemChat";

    fn encode_body(&self, out: &mut Vec<u8>) {
        nbt::write_network(&self.content.to_nbt(), out);
        out.write_bool(self.overlay);
    }
}

/// Moves the centre of the client's chunk cache, so it knows which chunks to
/// keep. Must be sent before the surrounding chunk data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetChunkCacheCenter {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl ClientboundPacket for SetChunkCacheCenter {
    const ID: i32 = ids::play::clientbound::SET_CHUNK_CACHE_CENTER;
    const NAME: &'static str = "SetChunkCacheCenter";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_varint(self.chunk_x);
        out.write_varint(self.chunk_z);
    }
}

/// Compass target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetDefaultSpawnPosition {
    pub position: BlockPos,
    pub angle: f32,
}

impl ClientboundPacket for SetDefaultSpawnPosition {
    const ID: i32 = ids::play::clientbound::SET_DEFAULT_SPAWN_POSITION;
    const NAME: &'static str = "SetDefaultSpawnPosition";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_position(self.position);
        out.write_f32(self.angle);
    }
}

/// State changes such as "start waiting for chunks" and weather transitions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    pub event: u8,
    pub value: f32,
}

impl GameEvent {
    /// Tells the client to show the terrain-loading screen until chunks arrive.
    pub const START_WAITING_FOR_CHUNKS: u8 = 13;

    /// Changes the player's game mode; the value is the mode id.
    pub const CHANGE_GAME_MODE: u8 = 3;

    #[must_use]
    pub const fn start_waiting_for_chunks() -> Self {
        Self {
            event: Self::START_WAITING_FOR_CHUNKS,
            value: 0.0,
        }
    }
}

impl ClientboundPacket for GameEvent {
    const ID: i32 = ids::play::clientbound::GAME_EVENT;
    const NAME: &'static str = "GameEvent";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_u8(self.event);
        out.write_f32(self.value);
    }
}

/// Teleports the player and resynchronises their position.
///
/// The client must echo `teleport_id` back in `AcceptTeleportation` before the
/// server accepts movement again; until then, incoming positions are stale and
/// should be dropped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub velocity_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Bit flags marking which fields are relative rather than absolute.
    pub flags: i32,
}

impl PlayerPosition {
    /// An absolute teleport with no velocity change.
    #[must_use]
    pub const fn absolute(teleport_id: i32, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Self {
        Self {
            teleport_id,
            x,
            y,
            z,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw,
            pitch,
            flags: 0,
        }
    }
}

impl ClientboundPacket for PlayerPosition {
    const ID: i32 = ids::play::clientbound::PLAYER_POSITION;
    const NAME: &'static str = "PlayerPosition";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_varint(self.teleport_id);
        out.write_f64(self.x);
        out.write_f64(self.y);
        out.write_f64(self.z);
        out.write_f64(self.velocity_x);
        out.write_f64(self.velocity_y);
        out.write_f64(self.velocity_z);
        out.write_f32(self.yaw);
        out.write_f32(self.pitch);
        out.write_i32(self.flags);
    }
}

/// A chunk column and its lighting.
///
/// The body after the coordinates is produced by the world layer, which owns
/// the paletted encoding; keeping it opaque here avoids a dependency cycle
/// between the protocol and world crates.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkData {
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// Heightmaps, section data, block entities and light, already serialized.
    pub payload: Vec<u8>,
}

impl ClientboundPacket for ChunkData {
    const ID: i32 = ids::play::clientbound::CHUNK_DATA;
    const NAME: &'static str = "ChunkData";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_i32(self.chunk_x);
        out.write_i32(self.chunk_z);
        out.write_bytes(&self.payload);
    }
}

// ---------------------------------------------------------------------------
// Serverbound
// ---------------------------------------------------------------------------

/// Confirms a teleport, quoting the id the server sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptTeleportation {
    pub teleport_id: i32,
}

impl ServerboundPacket for AcceptTeleportation {
    const ID: i32 = ids::play::serverbound::ACCEPT_TELEPORTATION;
    const NAME: &'static str = "AcceptTeleportation";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            teleport_id: r.read_varint()?,
        })
    }
}

/// Movement flags: bit 0 is "on ground", bit 1 is "pushing against a wall".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveFlags(pub u8);

impl MoveFlags {
    #[must_use]
    pub const fn on_ground(self) -> bool {
        self.0 & 0x01 != 0
    }

    #[must_use]
    pub const fn horizontal_collision(self) -> bool {
        self.0 & 0x02 != 0
    }
}

/// Position-only movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub flags: MoveFlags,
}

impl ServerboundPacket for MovePlayerPos {
    const ID: i32 = ids::play::serverbound::MOVE_PLAYER_POS;
    const NAME: &'static str = "MovePlayerPos";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            x: r.read_f64()?,
            y: r.read_f64()?,
            z: r.read_f64()?,
            flags: MoveFlags(r.read_u8()?),
        })
    }
}

/// Position and rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerPosRot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MoveFlags,
}

impl ServerboundPacket for MovePlayerPosRot {
    const ID: i32 = ids::play::serverbound::MOVE_PLAYER_POS_ROT;
    const NAME: &'static str = "MovePlayerPosRot";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            x: r.read_f64()?,
            y: r.read_f64()?,
            z: r.read_f64()?,
            yaw: r.read_f32()?,
            pitch: r.read_f32()?,
            flags: MoveFlags(r.read_u8()?),
        })
    }
}

/// Rotation only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayerRot {
    pub yaw: f32,
    pub pitch: f32,
    pub flags: MoveFlags,
}

impl ServerboundPacket for MovePlayerRot {
    const ID: i32 = ids::play::serverbound::MOVE_PLAYER_ROT;
    const NAME: &'static str = "MovePlayerRot";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            yaw: r.read_f32()?,
            pitch: r.read_f32()?,
            flags: MoveFlags(r.read_u8()?),
        })
    }
}

/// Neither position nor rotation changed, only the ground flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovePlayerStatusOnly {
    pub flags: MoveFlags,
}

impl ServerboundPacket for MovePlayerStatusOnly {
    const ID: i32 = ids::play::serverbound::MOVE_PLAYER_STATUS_ONLY;
    const NAME: &'static str = "MovePlayerStatusOnly";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            flags: MoveFlags(r.read_u8()?),
        })
    }
}

/// A slash command, without the leading slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCommand {
    pub command: String,
}

impl ServerboundPacket for ChatCommand {
    const ID: i32 = ids::play::serverbound::CHAT_COMMAND;
    const NAME: &'static str = "ChatCommand";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            command: r.read_string(MAX_COMMAND_LEN)?,
        })
    }
}

/// Marks the end of a client tick (1.21.2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientTickEnd;

impl ServerboundPacket for ClientTickEnd {
    const ID: i32 = ids::play::serverbound::CLIENT_TICK_END;
    const NAME: &'static str = "ClientTickEnd";

    fn decode_body(_r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{decode_exact, encode};

    fn sample_login() -> Login {
        Login {
            entity_id: 1,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld".into()],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            do_limited_crafting: false,
            dimension_type: 0,
            dimension_name: "minecraft:overworld".into(),
            hashed_seed: 0,
            game_mode: GameMode::Creative,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: true,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            enforces_secure_chat: false,
        }
    }

    #[test]
    fn login_round_trips() {
        let original = sample_login();
        let mut body = Vec::new();
        original.encode_body(&mut body);
        let decoded = Login::decode_body(&mut PacketReader::new(&body)).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn login_round_trips_with_a_death_location() {
        let mut original = sample_login();
        original.death_location = Some(("minecraft:the_nether".into(), BlockPos::new(-10, 64, 30)));
        let mut body = Vec::new();
        original.encode_body(&mut body);
        assert_eq!(
            Login::decode_body(&mut PacketReader::new(&body)).unwrap(),
            original
        );
    }

    #[test]
    fn keep_alive_round_trips() {
        let mut out = Vec::new();
        encode(&KeepAlive { id: 12345 }, &mut out);
        let mut r = PacketReader::new(&out);
        assert_eq!(
            r.read_varint().unwrap(),
            <KeepAlive as ClientboundPacket>::ID
        );
        assert_eq!(
            decode_exact::<KeepAlive>(r.read_remaining(), "play")
                .unwrap()
                .id,
            12345
        );
    }

    #[test]
    fn movement_flags_decode_bitwise() {
        assert!(!MoveFlags(0).on_ground());
        assert!(MoveFlags(0b01).on_ground());
        assert!(!MoveFlags(0b01).horizontal_collision());
        assert!(MoveFlags(0b11).horizontal_collision());
    }

    #[test]
    fn movement_packets_round_trip() {
        let mut body = Vec::new();
        body.write_f64(1.5);
        body.write_f64(64.0);
        body.write_f64(-2.5);
        body.write_f32(90.0);
        body.write_f32(-45.0);
        body.write_u8(1);

        let packet = decode_exact::<MovePlayerPosRot>(&body, "play").unwrap();
        assert_eq!(packet.x, 1.5);
        assert_eq!(packet.y, 64.0);
        assert_eq!(packet.z, -2.5);
        assert_eq!(packet.yaw, 90.0);
        assert_eq!(packet.pitch, -45.0);
        assert!(packet.flags.on_ground());
    }

    #[test]
    fn overlong_command_is_rejected() {
        let mut body = Vec::new();
        body.write_string(&"a".repeat(MAX_COMMAND_LEN + 1));
        assert!(decode_exact::<ChatCommand>(&body, "play").is_err());
    }

    #[test]
    fn chunk_data_prefixes_coordinates_then_opaque_payload() {
        let packet = ChunkData {
            chunk_x: -3,
            chunk_z: 7,
            payload: vec![0xAA, 0xBB],
        };
        let mut out = Vec::new();
        encode(&packet, &mut out);

        let mut r = PacketReader::new(&out);
        assert_eq!(r.read_varint().unwrap(), ChunkData::ID);
        assert_eq!(r.read_i32().unwrap(), -3);
        assert_eq!(r.read_i32().unwrap(), 7);
        assert_eq!(r.read_remaining(), &[0xAA, 0xBB]);
    }

    #[test]
    fn player_position_helper_zeroes_velocity_and_flags() {
        let packet = PlayerPosition::absolute(1, 0.5, 64.0, 0.5, 0.0, 0.0);
        assert_eq!(packet.velocity_x, 0.0);
        assert_eq!(packet.flags, 0);

        let mut out = Vec::new();
        encode(&packet, &mut out);
        let mut r = PacketReader::new(&out);
        assert_eq!(r.read_varint().unwrap(), PlayerPosition::ID);
        assert_eq!(r.read_varint().unwrap(), 1);
        assert_eq!(r.read_f64().unwrap(), 0.5);
    }

    #[test]
    fn empty_body_packets_reject_trailing_data() {
        assert!(decode_exact::<ClientTickEnd>(&[], "play").is_ok());
        assert!(decode_exact::<ClientTickEnd>(&[0], "play").is_err());
    }
}
