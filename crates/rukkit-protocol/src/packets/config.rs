//! Configuration state (1.20.2+).
//!
//! Between login and play the server ships the data-driven registries — biomes,
//! dimension types, damage types, and so on — that the client needs before it
//! can render a world. A server can also return here later to reconfigure a
//! player without disconnecting them.

use crate::error::Result;
use crate::nbt::{self, NbtTag};
use crate::packets::{ids, ClientboundPacket, ServerboundPacket};
use crate::reader::{PacketReader, DEFAULT_MAX_STRING_LEN};
use crate::text::Component;
use crate::writer::PacketWrite;

const MAX_LOCALE_LEN: usize = 16;
const MAX_REGISTRY_ENTRIES: usize = 1 << 16;
const MAX_KNOWN_PACKS: usize = 1024;

/// How much chat the client wants to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatMode {
    #[default]
    Enabled,
    CommandsOnly,
    Hidden,
}

impl ChatMode {
    pub fn from_id(id: i32) -> Result<Self> {
        Ok(match id {
            0 => Self::Enabled,
            1 => Self::CommandsOnly,
            2 => Self::Hidden,
            other => {
                return Err(crate::error::ProtocolError::InvalidEnum {
                    kind: "ChatMode",
                    value: i64::from(other),
                })
            }
        })
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// The client's display and privacy settings.
///
/// `view_distance` in particular is a request, not a command: the server clamps
/// it to its own configured maximum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation {
    pub locale: String,
    pub view_distance: i8,
    pub chat_mode: ChatMode,
    pub chat_colors: bool,
    pub displayed_skin_parts: u8,
    /// 0 = left, 1 = right.
    pub main_hand: i32,
    pub text_filtering_enabled: bool,
    /// Whether the player may appear in the server list sample.
    pub allows_listing: bool,
    /// Particle detail level (1.21.2+).
    pub particle_status: i32,
}

impl ClientInformation {
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_string(&self.locale);
        out.write_i8(self.view_distance);
        out.write_varint(self.chat_mode.id());
        out.write_bool(self.chat_colors);
        out.write_u8(self.displayed_skin_parts);
        out.write_varint(self.main_hand);
        out.write_bool(self.text_filtering_enabled);
        out.write_bool(self.allows_listing);
        out.write_varint(self.particle_status);
    }
}

impl ServerboundPacket for ClientInformation {
    const ID: i32 = ids::configuration::serverbound::CLIENT_INFORMATION;
    const NAME: &'static str = "ClientInformation";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            locale: r.read_string(MAX_LOCALE_LEN)?,
            view_distance: r.read_i8()?,
            chat_mode: ChatMode::from_id(r.read_varint()?)?,
            chat_colors: r.read_bool()?,
            displayed_skin_parts: r.read_u8()?,
            main_hand: r.read_varint()?,
            text_filtering_enabled: r.read_bool()?,
            allows_listing: r.read_bool()?,
            particle_status: r.read_varint()?,
        })
    }
}

/// Client signals it has finished configuring and is ready for play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishConfigurationAck;

impl ServerboundPacket for FinishConfigurationAck {
    const ID: i32 = ids::configuration::serverbound::FINISH_CONFIGURATION;
    const NAME: &'static str = "FinishConfigurationAck";

    fn decode_body(_r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self)
    }
}

/// Server tells the client that configuration is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishConfiguration;

impl ClientboundPacket for FinishConfiguration {
    const ID: i32 = ids::configuration::clientbound::FINISH_CONFIGURATION;
    const NAME: &'static str = "FinishConfiguration";

    fn encode_body(&self, _out: &mut Vec<u8>) {}
}

/// Liveness probe, echoed by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

impl ClientboundPacket for KeepAlive {
    const ID: i32 = ids::configuration::clientbound::KEEP_ALIVE;
    const NAME: &'static str = "KeepAlive";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_i64(self.id);
    }
}

impl ServerboundPacket for KeepAlive {
    const ID: i32 = ids::configuration::serverbound::KEEP_ALIVE;
    const NAME: &'static str = "KeepAlive";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self { id: r.read_i64()? })
    }
}

/// Kick during configuration. Carries NBT, unlike the login-state disconnect.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    pub reason: Component,
}

impl ClientboundPacket for Disconnect {
    const ID: i32 = ids::configuration::clientbound::DISCONNECT;
    const NAME: &'static str = "Disconnect";

    fn encode_body(&self, out: &mut Vec<u8>) {
        nbt::write_network(&self.reason.to_nbt(), out);
    }
}

/// One registry entry: an id and, optionally, its definition.
///
/// Omitting the data means "use the client's built-in definition", which is how
/// a vanilla-identical server keeps this packet small.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
    pub entry_id: String,
    pub data: Option<NbtTag>,
}

/// A whole registry, sent once per registry during configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryData {
    pub registry_id: String,
    pub entries: Vec<RegistryEntry>,
}

impl ClientboundPacket for RegistryData {
    const ID: i32 = ids::configuration::clientbound::REGISTRY_DATA;
    const NAME: &'static str = "RegistryData";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_identifier(&self.registry_id);
        out.write_varint(self.entries.len() as i32);
        for entry in &self.entries {
            out.write_identifier(&entry.entry_id);
            match &entry.data {
                Some(tag) => {
                    out.write_bool(true);
                    nbt::write_network(tag, out);
                }
                None => out.write_bool(false),
            }
        }
    }
}

impl RegistryData {
    /// Decoding exists for tests and proxy use; a server only sends this.
    pub fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        let registry_id = r.read_identifier()?;
        let entries = r.read_array(MAX_REGISTRY_ENTRIES, |r| {
            let entry_id = r.read_identifier()?;
            let data = if r.read_bool()? {
                Some(nbt::read_network(r)?)
            } else {
                None
            };
            Ok(RegistryEntry { entry_id, data })
        })?;
        Ok(Self {
            registry_id,
            entries,
        })
    }
}

/// A data pack both sides claim to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

/// Negotiates which built-in data packs the client already has, so their
/// registry contents need not be transmitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectKnownPacks {
    pub packs: Vec<KnownPack>,
}

impl SelectKnownPacks {
    /// The vanilla core pack for the targeted version.
    #[must_use]
    pub fn vanilla() -> Self {
        Self {
            packs: vec![KnownPack {
                namespace: "minecraft".into(),
                id: "core".into(),
                version: crate::version::MINECRAFT_VERSION.into(),
            }],
        }
    }
}

impl ClientboundPacket for SelectKnownPacks {
    const ID: i32 = ids::configuration::clientbound::SELECT_KNOWN_PACKS;
    const NAME: &'static str = "SelectKnownPacks";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_varint(self.packs.len() as i32);
        for pack in &self.packs {
            out.write_string(&pack.namespace);
            out.write_string(&pack.id);
            out.write_string(&pack.version);
        }
    }
}

impl ServerboundPacket for SelectKnownPacks {
    const ID: i32 = ids::configuration::serverbound::SELECT_KNOWN_PACKS;
    const NAME: &'static str = "SelectKnownPacks";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            packs: r.read_array(MAX_KNOWN_PACKS, |r| {
                Ok(KnownPack {
                    namespace: r.read_string(DEFAULT_MAX_STRING_LEN)?,
                    id: r.read_string(DEFAULT_MAX_STRING_LEN)?,
                    version: r.read_string(DEFAULT_MAX_STRING_LEN)?,
                })
            })?,
        })
    }
}

/// Vendor extension channel. `minecraft:brand` travels here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMessage {
    pub channel: String,
    pub data: Vec<u8>,
}

impl PluginMessage {
    /// The `minecraft:brand` message announcing this server's implementation.
    #[must_use]
    pub fn brand(brand: &str) -> Self {
        let mut data = Vec::new();
        data.write_string(brand);
        Self {
            channel: "minecraft:brand".into(),
            data,
        }
    }
}

impl ClientboundPacket for PluginMessage {
    const ID: i32 = ids::configuration::clientbound::PLUGIN_MESSAGE;
    const NAME: &'static str = "PluginMessage";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_identifier(&self.channel);
        // No length prefix: the payload runs to the end of the packet.
        out.write_bytes(&self.data);
    }
}

impl ServerboundPacket for PluginMessage {
    const ID: i32 = ids::configuration::serverbound::PLUGIN_MESSAGE;
    const NAME: &'static str = "PluginMessage";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            channel: r.read_identifier()?,
            data: r.read_remaining().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtCompound;
    use crate::packets::{decode_exact, encode};

    fn sample_client_info() -> ClientInformation {
        ClientInformation {
            locale: "ko_kr".into(),
            view_distance: 12,
            chat_mode: ChatMode::Enabled,
            chat_colors: true,
            displayed_skin_parts: 0x7F,
            main_hand: 1,
            text_filtering_enabled: false,
            allows_listing: true,
            particle_status: 0,
        }
    }

    #[test]
    fn client_information_round_trips() {
        let original = sample_client_info();
        let mut body = Vec::new();
        original.encode_body(&mut body);
        assert_eq!(
            decode_exact::<ClientInformation>(&body, "configuration").unwrap(),
            original
        );
    }

    #[test]
    fn unknown_chat_mode_is_rejected() {
        let mut body = Vec::new();
        body.write_string("en_us");
        body.write_i8(8);
        body.write_varint(9); // not a chat mode
        assert!(decode_exact::<ClientInformation>(&body, "configuration").is_err());
    }

    #[test]
    fn keep_alive_round_trips_in_both_directions() {
        let id = -0x1234_5678_9ABC_DEF0;
        let mut out = Vec::new();
        encode(&KeepAlive { id }, &mut out);

        let mut r = PacketReader::new(&out);
        assert_eq!(
            r.read_varint().unwrap(),
            <KeepAlive as ClientboundPacket>::ID
        );
        let body = r.read_remaining();
        assert_eq!(
            decode_exact::<KeepAlive>(body, "configuration").unwrap().id,
            id
        );
    }

    #[test]
    fn registry_data_round_trips_with_and_without_definitions() {
        let mut definition = NbtCompound::new();
        definition
            .insert("has_skylight", true)
            .insert("height", 384i32);

        let original = RegistryData {
            registry_id: "minecraft:dimension_type".into(),
            entries: vec![
                RegistryEntry {
                    entry_id: "minecraft:overworld".into(),
                    data: Some(NbtTag::Compound(definition)),
                },
                RegistryEntry {
                    entry_id: "minecraft:the_nether".into(),
                    data: None,
                },
            ],
        };

        let mut body = Vec::new();
        original.encode_body(&mut body);
        let decoded = RegistryData::decode_body(&mut PacketReader::new(&body)).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn known_packs_round_trip() {
        let original = SelectKnownPacks::vanilla();
        assert_eq!(original.packs[0].version, crate::version::MINECRAFT_VERSION);

        let mut body = Vec::new();
        original.encode_body(&mut body);
        assert_eq!(
            decode_exact::<SelectKnownPacks>(&body, "configuration").unwrap(),
            original
        );
    }

    #[test]
    fn plugin_message_payload_runs_to_the_end_of_the_packet() {
        let original = PluginMessage::brand("Rukkit");
        let mut body = Vec::new();
        original.encode_body(&mut body);
        let decoded = decode_exact::<PluginMessage>(&body, "configuration").unwrap();
        assert_eq!(decoded, original);

        // The brand payload is itself a length-prefixed string.
        let mut r = PacketReader::new(&decoded.data);
        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "Rukkit");
    }

    #[test]
    fn disconnect_reason_is_nbt_not_json() {
        let packet = Disconnect {
            reason: Component::text("bye"),
        };
        let mut body = Vec::new();
        packet.encode_body(&mut body);
        // First byte is the NBT compound tag id, not '{'.
        assert_eq!(body[0], nbt::TAG_COMPOUND);
        let decoded = nbt::read_network(&mut PacketReader::new(&body)).unwrap();
        assert_eq!(decoded, packet.reason.to_nbt());
    }
}
