//! Per-connection state machine.
//!
//! Handshake, status, login and configuration are a strict request/response
//! dance, so they run sequentially on one task that owns the socket outright.
//! That keeps the codec's compression and encryption state — which is ordered
//! and must switch at exactly the right byte — in a single place with no
//! synchronisation at all.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use rukkit_protocol::codec::{Decoder, Encoder};
use rukkit_protocol::packets::{
    self, config as config_packets, handshake, login as login_packets, play as play_packets,
    status as status_packets, ClientboundPacket, ServerboundPacket,
};
use rukkit_protocol::text::{Component, NamedColor};
use rukkit_protocol::{varint, version, ProtocolError, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::player::{offline_uuid, JoinError, PlayerInfo};
use crate::server::ServerContext;

/// How long a client may stay silent before being dropped, prior to play.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Socket read buffer. Sized to hold a typical burst without repeated syscalls.
const READ_BUFFER: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConnError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    #[error("peer closed the connection")]
    Closed,

    #[error("timed out waiting for the client")]
    TimedOut,

    #[error("unsupported protocol version {got} (this server speaks {expected})")]
    UnsupportedVersion { got: i32, expected: i32 },

    #[error("unexpected packet {id:#04x} in state {state}")]
    UnexpectedPacket { state: &'static str, id: i32 },

    #[error("{0}")]
    Rejected(String),
}

/// A framed connection to one client.
#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
    addr: SocketAddr,
    decoder: Decoder,
    encoder: Encoder,
    read_buf: Vec<u8>,
    out: BytesMut,
    scratch: Vec<u8>,
}

impl Connection {
    #[must_use]
    pub fn new(stream: TcpStream, addr: SocketAddr) -> Self {
        Self {
            stream,
            addr,
            decoder: Decoder::new(),
            encoder: Encoder::new(),
            read_buf: vec![0; READ_BUFFER],
            out: BytesMut::with_capacity(READ_BUFFER),
            scratch: Vec::with_capacity(1024),
        }
    }

    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Reads until a whole packet is available.
    pub async fn read_packet(&mut self) -> Result<Bytes, ConnError> {
        loop {
            if let Some(packet) = self.decoder.decode()? {
                return Ok(packet);
            }
            let read = self.stream.read(&mut self.read_buf).await?;
            if read == 0 {
                return Err(ConnError::Closed);
            }
            self.decoder.feed(&mut self.read_buf[..read]);
        }
    }

    /// Reads a packet, giving up after `HANDSHAKE_TIMEOUT`.
    ///
    /// Without this a client can open a socket, send nothing, and hold a task
    /// forever — the cheapest denial of service there is.
    pub async fn read_packet_timeout(&mut self) -> Result<Bytes, ConnError> {
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, self.read_packet()).await {
            Ok(result) => result,
            Err(_) => Err(ConnError::TimedOut),
        }
    }

    /// Frames and writes one packet.
    pub async fn send<P: ClientboundPacket>(&mut self, packet: &P) -> Result<(), ConnError> {
        packets::encode(packet, &mut self.scratch);
        self.out.clear();
        self.encoder.encode(&self.scratch, &mut self.out)?;
        self.stream.write_all(&self.out).await?;
        Ok(())
    }

    /// Enables compression on both directions.
    ///
    /// Must be called only after `SetCompression` has been written, since that
    /// packet itself travels uncompressed.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.encoder.enable_compression(threshold);
        self.decoder.enable_compression(threshold);
    }

    pub async fn shutdown(mut self) {
        let _ = self.stream.shutdown().await;
    }
}

/// Splits a packet body into its id and the rest.
fn split_id(packet: &[u8]) -> Result<(i32, &[u8]), ProtocolError> {
    let (id, used) = varint::read_varint(packet)?;
    Ok((id, &packet[used..]))
}

/// Drives one connection from its first byte to its last.
pub async fn handle(stream: TcpStream, addr: SocketAddr, ctx: Arc<ServerContext>) {
    // Disabling Nagle matters here: the protocol is a long series of small
    // packets, and 40 ms of coalescing delay is visible as input lag.
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!(%addr, "could not disable Nagle: {e}");
    }

    let mut conn = Connection::new(stream, addr);
    match run(&mut conn, &ctx).await {
        Ok(()) => tracing::debug!(%addr, "connection closed"),
        Err(ConnError::Closed) => tracing::debug!(%addr, "client disconnected"),
        Err(e) => tracing::debug!(%addr, "connection ended: {e}"),
    }
    conn.shutdown().await;
}

async fn run(conn: &mut Connection, ctx: &ServerContext) -> Result<(), ConnError> {
    let packet = conn.read_packet_timeout().await?;
    let (id, body) = split_id(&packet)?;
    if id != handshake::Intention::ID {
        return Err(ConnError::UnexpectedPacket {
            state: State::Handshaking.name(),
            id,
        });
    }
    let intention: handshake::Intention = packets::decode_exact(body, State::Handshaking.name())?;

    match intention.next_state {
        handshake::NextState::Status => status_phase(conn, ctx).await,
        handshake::NextState::Login | handshake::NextState::Transfer => {
            if !version::is_supported(intention.protocol_version) {
                // Tell the client why rather than dropping it silently; this is
                // the message players actually see when versions mismatch.
                let reason = Component::text(format!(
                    "This server is running Minecraft {} (protocol {}). Your client speaks protocol {}.",
                    version::MINECRAFT_VERSION,
                    version::PROTOCOL_VERSION,
                    intention.protocol_version
                ))
                .color(NamedColor::Red);
                let _ = conn
                    .send(&login_packets::LoginDisconnect {
                        reason_json: reason.to_json(),
                    })
                    .await;
                return Err(ConnError::UnsupportedVersion {
                    got: intention.protocol_version,
                    expected: version::PROTOCOL_VERSION,
                });
            }
            login_phase(conn, ctx).await
        }
    }
}

/// Server-list ping. Never authenticates, so it stays strictly bounded.
async fn status_phase(conn: &mut Connection, ctx: &ServerContext) -> Result<(), ConnError> {
    loop {
        let packet = match conn.read_packet_timeout().await {
            Ok(packet) => packet,
            // A client that pings and hangs up without asking for a pong is
            // normal behaviour, not an error worth logging.
            Err(ConnError::Closed) => return Ok(()),
            Err(e) => return Err(e),
        };
        let (id, body) = split_id(&packet)?;

        match id {
            status_packets::StatusRequest::ID => {
                packets::decode_exact::<status_packets::StatusRequest>(body, State::Status.name())?;
                let online = ctx.players.count();
                let sample = ctx.players.sample_names(12);
                let json = ctx.status.json(online as i32, &sample);
                conn.send(&status_packets::StatusResponse {
                    json: json.to_string(),
                })
                .await?;
            }
            status_packets::PingRequest::ID => {
                let ping: status_packets::PingRequest =
                    packets::decode_exact(body, State::Status.name())?;
                conn.send(&status_packets::PongResponse {
                    payload: ping.payload,
                })
                .await?;
                // The pong ends the exchange.
                return Ok(());
            }
            other => {
                return Err(ConnError::UnexpectedPacket {
                    state: State::Status.name(),
                    id: other,
                })
            }
        }
    }
}

async fn login_phase(conn: &mut Connection, ctx: &ServerContext) -> Result<(), ConnError> {
    let packet = conn.read_packet_timeout().await?;
    let (id, body) = split_id(&packet)?;
    if id != login_packets::Hello::ID {
        return Err(ConnError::UnexpectedPacket {
            state: State::Login.name(),
            id,
        });
    }
    let hello: login_packets::Hello = packets::decode_exact(body, State::Login.name())?;

    if ctx.config.online_mode {
        // Encryption and session-server verification are not implemented yet;
        // refusing plainly beats silently letting anyone in under a real name.
        let reason = Component::text(
            "This server has online-mode enabled, which this build does not yet support.",
        )
        .color(NamedColor::Red);
        conn.send(&login_packets::LoginDisconnect {
            reason_json: reason.to_json(),
        })
        .await?;
        return Err(ConnError::Rejected("online mode is not implemented".into()));
    }

    let uuid = offline_uuid(&hello.name);

    if let Some(threshold) = ctx.config.compression() {
        conn.send(&login_packets::SetCompression {
            threshold: threshold as i32,
        })
        .await?;
        // Only now, with that packet already on the wire uncompressed.
        conn.enable_compression(threshold);
    }

    conn.send(&login_packets::LoginSuccess {
        profile_id: uuid,
        username: hello.name.clone(),
        properties: Vec::new(),
    })
    .await?;

    let packet = conn.read_packet_timeout().await?;
    let (id, body) = split_id(&packet)?;
    if id != login_packets::LoginAcknowledged::ID {
        return Err(ConnError::UnexpectedPacket {
            state: State::Login.name(),
            id,
        });
    }
    packets::decode_exact::<login_packets::LoginAcknowledged>(body, State::Login.name())?;

    let info = PlayerInfo {
        uuid,
        name: hello.name.clone(),
        addr: conn.addr(),
    };
    let max = ctx.config.max_players.max(0) as usize;
    if let Err(why) = ctx.players.try_insert(info, max) {
        let reason = match why {
            JoinError::Full => Component::text("The server is full").color(NamedColor::Red),
            JoinError::NameTaken => {
                Component::text("Someone is already playing under that name").color(NamedColor::Red)
            }
        };
        conn.send(&config_packets::Disconnect { reason }).await?;
        return Err(ConnError::Rejected(why.to_string()));
    }

    tracing::info!(player = %hello.name, %uuid, addr = %conn.addr(), "player logged in");

    let result = configuration_phase(conn, ctx, &hello.name).await;

    ctx.players.remove(uuid);
    tracing::info!(player = %hello.name, "player disconnected");
    result
}

async fn configuration_phase(
    conn: &mut Connection,
    ctx: &ServerContext,
    name: &str,
) -> Result<(), ConnError> {
    conn.send(&config_packets::PluginMessage::brand(version::SERVER_BRAND))
        .await?;
    conn.send(&config_packets::SelectKnownPacks::vanilla())
        .await?;

    loop {
        let packet = conn.read_packet_timeout().await?;
        let (id, body) = split_id(&packet)?;

        match id {
            config_packets::ClientInformation::ID => {
                let info: config_packets::ClientInformation =
                    packets::decode_exact(body, State::Configuration.name())?;
                // The client's view distance is a request; the server's own
                // setting is the ceiling.
                let effective =
                    i32::from(info.view_distance.max(0)).min(i32::from(ctx.config.view_distance));
                tracing::debug!(
                    player = %name,
                    locale = %info.locale,
                    requested = info.view_distance,
                    effective,
                    "client settings received"
                );
            }
            id if id == <config_packets::SelectKnownPacks as ServerboundPacket>::ID => {
                let packs: config_packets::SelectKnownPacks =
                    packets::decode_exact(body, State::Configuration.name())?;
                tracing::debug!(player = %name, packs = packs.packs.len(), "known packs");

                // Registry contents (dimension types, biomes, damage types, ...)
                // come from the version's own data files. Until those are
                // loaded there is nothing truthful to send, and a client cannot
                // enter play without them.
                let reason = Component::text(
                    "Rukkit can accept your login, but cannot yet enter the world: the 26.2 registry data is not loaded. Server-list ping and login work; see the README roadmap.",
                )
                .color(NamedColor::Gold);
                conn.send(&config_packets::Disconnect { reason }).await?;
                return Err(ConnError::Rejected("registry data not loaded".into()));
            }
            id if id == <config_packets::PluginMessage as ServerboundPacket>::ID => {
                let message: config_packets::PluginMessage =
                    packets::decode_exact(body, State::Configuration.name())?;
                tracing::trace!(player = %name, channel = %message.channel, "plugin message");
            }
            id if id == <config_packets::KeepAlive as ServerboundPacket>::ID => {
                packets::decode_exact::<config_packets::KeepAlive>(
                    body,
                    State::Configuration.name(),
                )?;
            }
            config_packets::FinishConfigurationAck::ID => {
                packets::decode_exact::<config_packets::FinishConfigurationAck>(
                    body,
                    State::Configuration.name(),
                )?;
                if rukkit_protocol::packets::ids::play::PROVISIONAL {
                    tracing::warn!(
                        player = %name,
                        "reached play state, whose packet ids are still provisional for 26.2"
                    );
                }
                let _ = play_packets::GameEvent::start_waiting_for_chunks();
                return Ok(());
            }
            other => {
                tracing::debug!(player = %name, id = other, "ignoring configuration packet");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rukkit_protocol::writer::PacketWrite;

    #[test]
    fn split_id_separates_the_id_from_the_body() {
        let mut packet = Vec::new();
        packet.write_varint(0x2C);
        packet.write_bytes(b"body");
        let (id, body) = split_id(&packet).unwrap();
        assert_eq!(id, 0x2C);
        assert_eq!(body, b"body");
    }

    #[test]
    fn split_id_handles_multibyte_ids() {
        let mut packet = Vec::new();
        packet.write_varint(300);
        packet.write_bytes(b"x");
        let (id, body) = split_id(&packet).unwrap();
        assert_eq!(id, 300);
        assert_eq!(body, b"x");
    }

    #[test]
    fn split_id_rejects_an_empty_packet() {
        assert!(split_id(&[]).is_err());
    }

    #[test]
    fn an_id_with_no_body_is_fine() {
        let (id, body) = split_id(&[0x00]).unwrap();
        assert_eq!(id, 0);
        assert!(body.is_empty());
    }
}
