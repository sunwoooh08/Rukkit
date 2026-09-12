//! Login state: identity, optional encryption, and compression setup.

use uuid::Uuid;

use crate::error::Result;
use crate::packets::{ids, ClientboundPacket, ServerboundPacket};
use crate::reader::PacketReader;
use crate::writer::PacketWrite;

/// Vanilla's username length cap.
pub const MAX_USERNAME_LEN: usize = 16;
/// Length cap for the (legacy, now always empty) server id field.
pub const MAX_SERVER_ID_LEN: usize = 20;
/// Generous cap for RSA keys and verify tokens.
const MAX_KEY_LEN: usize = 1024;

/// First packet of the login flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    /// The client's own idea of its UUID. In online mode the server replaces
    /// this with the value Mojang's session server returns, so it must never be
    /// trusted as an identity.
    pub profile_id: Uuid,
}

impl Hello {
    pub fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_string(&self.name);
        out.write_uuid(self.profile_id);
    }
}

impl ServerboundPacket for Hello {
    const ID: i32 = ids::login::serverbound::HELLO;
    const NAME: &'static str = "Hello";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            name: r.read_string(MAX_USERNAME_LEN)?,
            profile_id: r.read_uuid()?,
        })
    }
}

/// The client's half of the key exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionResponse {
    /// AES key, RSA-encrypted with the server's public key.
    pub shared_secret: Vec<u8>,
    /// The server's verify token, encrypted the same way.
    pub verify_token: Vec<u8>,
}

impl ServerboundPacket for EncryptionResponse {
    const ID: i32 = ids::login::serverbound::ENCRYPTION_RESPONSE;
    const NAME: &'static str = "EncryptionResponse";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            shared_secret: r.read_byte_array(MAX_KEY_LEN)?.to_vec(),
            verify_token: r.read_byte_array(MAX_KEY_LEN)?.to_vec(),
        })
    }
}

/// Sent once the client has processed `LoginSuccess`; moves to configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginAcknowledged;

impl ServerboundPacket for LoginAcknowledged {
    const ID: i32 = ids::login::serverbound::LOGIN_ACKNOWLEDGED;
    const NAME: &'static str = "LoginAcknowledged";

    fn decode_body(_r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self)
    }
}

/// Kick during login. Still carries JSON, not NBT, unlike the play-state
/// disconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDisconnect {
    pub reason_json: String,
}

impl ClientboundPacket for LoginDisconnect {
    const ID: i32 = ids::login::clientbound::LOGIN_DISCONNECT;
    const NAME: &'static str = "LoginDisconnect";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_string(&self.reason_json);
    }
}

/// The server's half of the key exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionRequest {
    /// Always empty since 1.7; kept because the field is still on the wire.
    pub server_id: String,
    /// DER-encoded RSA public key.
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
    /// Whether the client should contact the session server.
    pub should_authenticate: bool,
}

impl ClientboundPacket for EncryptionRequest {
    const ID: i32 = ids::login::clientbound::ENCRYPTION_REQUEST;
    const NAME: &'static str = "EncryptionRequest";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_string(&self.server_id);
        out.write_byte_array(&self.public_key);
        out.write_byte_array(&self.verify_token);
        out.write_bool(self.should_authenticate);
    }
}

/// A signed profile property, such as the player's skin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// Confirms the resolved identity and ends the login exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    pub profile_id: Uuid,
    pub username: String,
    pub properties: Vec<ProfileProperty>,
}

impl ClientboundPacket for LoginSuccess {
    const ID: i32 = ids::login::clientbound::LOGIN_SUCCESS;
    const NAME: &'static str = "LoginSuccess";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_uuid(self.profile_id);
        out.write_string(&self.username);
        out.write_varint(self.properties.len() as i32);
        for property in &self.properties {
            out.write_string(&property.name);
            out.write_string(&property.value);
            out.write_option(property.signature.as_deref(), |o, s| o.write_string(s));
        }
    }
}

/// Enables compression for every subsequent packet.
///
/// This packet itself is sent uncompressed; the codec must be switched only
/// after it has been written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCompression {
    /// Packets at or above this size are compressed. Negative disables.
    pub threshold: i32,
}

impl ClientboundPacket for SetCompression {
    const ID: i32 = ids::login::clientbound::SET_COMPRESSION;
    const NAME: &'static str = "SetCompression";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_varint(self.threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{decode_exact, encode};
    use crate::reader::DEFAULT_MAX_STRING_LEN;

    #[test]
    fn hello_round_trips() {
        let original = Hello {
            name: "Notch".into(),
            profile_id: Uuid::from_u128(42),
        };
        let mut body = Vec::new();
        original.encode_body(&mut body);
        assert_eq!(decode_exact::<Hello>(&body, "login").unwrap(), original);
    }

    #[test]
    fn username_longer_than_sixteen_is_rejected() {
        let mut body = Vec::new();
        body.write_string(&"x".repeat(17));
        body.write_uuid(Uuid::nil());
        assert!(decode_exact::<Hello>(&body, "login").is_err());

        let mut body = Vec::new();
        body.write_string(&"x".repeat(16));
        body.write_uuid(Uuid::nil());
        assert!(decode_exact::<Hello>(&body, "login").is_ok());
    }

    #[test]
    fn encryption_response_round_trips() {
        let mut body = Vec::new();
        body.write_byte_array(&[1, 2, 3, 4]);
        body.write_byte_array(&[9, 8]);
        let packet = decode_exact::<EncryptionResponse>(&body, "login").unwrap();
        assert_eq!(packet.shared_secret, [1, 2, 3, 4]);
        assert_eq!(packet.verify_token, [9, 8]);
    }

    #[test]
    fn absurd_key_length_is_rejected() {
        let mut body = Vec::new();
        body.write_varint(1_000_000);
        assert!(decode_exact::<EncryptionResponse>(&body, "login").is_err());
    }

    #[test]
    fn login_acknowledged_is_empty() {
        assert!(decode_exact::<LoginAcknowledged>(&[], "login").is_ok());
        assert!(decode_exact::<LoginAcknowledged>(&[1], "login").is_err());
    }

    #[test]
    fn login_success_encodes_properties_with_optional_signatures() {
        let packet = LoginSuccess {
            profile_id: Uuid::from_u128(7),
            username: "player".into(),
            properties: vec![
                ProfileProperty {
                    name: "textures".into(),
                    value: "base64".into(),
                    signature: Some("sig".into()),
                },
                ProfileProperty {
                    name: "other".into(),
                    value: "v".into(),
                    signature: None,
                },
            ],
        };
        let mut out = Vec::new();
        encode(&packet, &mut out);

        let mut r = PacketReader::new(&out);
        assert_eq!(r.read_varint().unwrap(), LoginSuccess::ID);
        assert_eq!(r.read_uuid().unwrap(), packet.profile_id);
        assert_eq!(r.read_string(MAX_USERNAME_LEN).unwrap(), "player");
        assert_eq!(r.read_varint().unwrap(), 2);

        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "textures");
        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "base64");
        assert_eq!(
            r.read_option(|r| r.read_string(DEFAULT_MAX_STRING_LEN))
                .unwrap(),
            Some("sig".to_owned())
        );

        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "other");
        assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "v");
        assert_eq!(
            r.read_option(|r| r.read_string(DEFAULT_MAX_STRING_LEN))
                .unwrap(),
            None
        );
        assert!(r.is_empty());
    }

    #[test]
    fn set_compression_carries_the_threshold() {
        let mut out = Vec::new();
        encode(&SetCompression { threshold: 256 }, &mut out);
        let mut r = PacketReader::new(&out);
        assert_eq!(r.read_varint().unwrap(), SetCompression::ID);
        assert_eq!(r.read_varint().unwrap(), 256);
    }
}
