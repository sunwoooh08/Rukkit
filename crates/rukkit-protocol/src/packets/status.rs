//! Status state: the server list ping.
//!
//! This exchange is unauthenticated and unencrypted, and a busy server answers
//! far more pings than logins, so the response JSON is built once and cached by
//! the caller rather than rebuilt per connection.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::packets::{ids, ClientboundPacket, ServerboundPacket};
use crate::reader::PacketReader;
use crate::text::Component;
use crate::version::{MINECRAFT_VERSION, PROTOCOL_VERSION};
use crate::writer::PacketWrite;

/// Empty request asking for the server description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRequest;

impl ServerboundPacket for StatusRequest {
    const ID: i32 = ids::status::serverbound::STATUS_REQUEST;
    const NAME: &'static str = "StatusRequest";

    fn decode_body(_r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self)
    }
}

/// Latency probe. The payload is opaque and must be echoed verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingRequest {
    pub payload: i64,
}

impl ServerboundPacket for PingRequest {
    const ID: i32 = ids::status::serverbound::PING_REQUEST;
    const NAME: &'static str = "PingRequest";

    fn decode_body(r: &mut PacketReader<'_>) -> Result<Self> {
        Ok(Self {
            payload: r.read_i64()?,
        })
    }
}

/// The server description, as a JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResponse {
    pub json: String,
}

impl ClientboundPacket for StatusResponse {
    const ID: i32 = ids::status::clientbound::STATUS_RESPONSE;
    const NAME: &'static str = "StatusResponse";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_string(&self.json);
    }
}

/// Echo of [`PingRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PongResponse {
    pub payload: i64,
}

impl ClientboundPacket for PongResponse {
    const ID: i32 = ids::status::clientbound::PONG_RESPONSE;
    const NAME: &'static str = "PongResponse";

    fn encode_body(&self, out: &mut Vec<u8>) {
        out.write_i64(self.payload);
    }
}

// ---------------------------------------------------------------------------
// Status document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionInfo {
    pub name: String,
    pub protocol: i32,
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            name: MINECRAFT_VERSION.to_owned(),
            protocol: PROTOCOL_VERSION,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SamplePlayer {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayersInfo {
    pub max: i32,
    pub online: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<SamplePlayer>,
}

/// The full status document sent in [`StatusResponse`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerStatus {
    pub version: VersionInfo,
    pub players: PlayersInfo,
    pub description: Component,
    /// A `data:image/png;base64,` URI, 64x64. Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(rename = "enforcesSecureChat")]
    pub enforces_secure_chat: bool,
}

impl ServerStatus {
    #[must_use]
    pub fn new(description: Component, online: i32, max: i32) -> Self {
        Self {
            version: VersionInfo::default(),
            players: PlayersInfo {
                max,
                online,
                sample: Vec::new(),
            },
            description,
            favicon: None,
            enforces_secure_chat: false,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| String::from("{}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{decode_exact, encode};
    use crate::text::NamedColor;

    #[test]
    fn ping_payload_is_echoed_exactly() {
        let payload = -0x0123_4567_89AB_CDEF;
        let mut body = Vec::new();
        body.write_i64(payload);
        let request = decode_exact::<PingRequest>(&body, "status").unwrap();
        assert_eq!(request.payload, payload);

        let mut out = Vec::new();
        encode(&PongResponse { payload }, &mut out);
        assert_eq!(out[0] as i32, PongResponse::ID);
        assert_eq!(&out[1..], &payload.to_be_bytes());
    }

    #[test]
    fn status_request_has_an_empty_body() {
        assert!(decode_exact::<StatusRequest>(&[], "status").is_ok());
        assert!(decode_exact::<StatusRequest>(&[0x00], "status").is_err());
    }

    #[test]
    fn status_document_reports_the_targeted_version() {
        let status = ServerStatus::new(Component::text("A Rukkit server"), 3, 20);
        let json = status.to_json();
        assert!(json.contains(r#""protocol":776"#), "{json}");
        assert!(json.contains(r#""name":"26.2""#), "{json}");
        assert!(json.contains(r#""online":3"#), "{json}");
        assert!(json.contains(r#""max":20"#), "{json}");
        // Absent favicon must be omitted, not sent as null.
        assert!(!json.contains("favicon"), "{json}");
    }

    #[test]
    fn status_document_round_trips() {
        let mut status =
            ServerStatus::new(Component::text("Rukkit").color(NamedColor::Aqua), 0, 100);
        status.players.sample.push(SamplePlayer {
            name: "someone".into(),
            id: "00000000-0000-0000-0000-000000000000".into(),
        });
        let parsed: ServerStatus = serde_json::from_str(&status.to_json()).unwrap();
        assert_eq!(parsed, status);
    }

    #[test]
    fn status_response_body_is_a_single_string() {
        let packet = StatusResponse {
            json: r#"{"x":1}"#.into(),
        };
        let mut out = Vec::new();
        encode(&packet, &mut out);
        // id, then a 7-byte string prefix
        assert_eq!(out[0] as i32, StatusResponse::ID);
        assert_eq!(out[1], 7);
    }
}
