//! End-to-end tests over a real TCP socket.
//!
//! These drive the actual connection state machine through the actual codec,
//! so they cover the seams that unit tests cannot: framing across the socket,
//! packet ids, and the state transitions between them.

use std::sync::Arc;

use bytes::BytesMut;
use rukkit_protocol::codec::{Decoder, Encoder};
use rukkit_protocol::packets::status::ServerStatus;
use rukkit_protocol::packets::{handshake, ids, login as login_packets, ServerboundPacket};
use rukkit_protocol::reader::{PacketReader, DEFAULT_MAX_STRING_LEN};
use rukkit_protocol::version;
use rukkit_protocol::writer::PacketWrite;
use rukkit_server::{Server, ServerConfig, ServerContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A minimal client that speaks the framed protocol.
struct TestClient {
    stream: TcpStream,
    encoder: Encoder,
    decoder: Decoder,
    buf: Vec<u8>,
}

impl TestClient {
    async fn connect(addr: std::net::SocketAddr) -> Self {
        Self {
            stream: TcpStream::connect(addr).await.expect("connect"),
            encoder: Encoder::new(),
            decoder: Decoder::new(),
            buf: vec![0; 8192],
        }
    }

    async fn send_raw(&mut self, packet: &[u8]) {
        let mut out = BytesMut::new();
        self.encoder.encode(packet, &mut out).expect("encode");
        self.stream.write_all(&out).await.expect("write");
    }

    async fn handshake(&mut self, protocol: i32, next: handshake::NextState, port: u16) {
        let intention = handshake::Intention {
            protocol_version: protocol,
            server_address: "127.0.0.1".into(),
            server_port: port,
            next_state: next,
        };
        let mut packet = Vec::new();
        packet.write_varint(handshake::Intention::ID);
        intention.encode_body(&mut packet);
        self.send_raw(&packet).await;
    }

    /// Reads one packet, returning `None` if the server closed first.
    async fn recv(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(packet) = self.decoder.decode().expect("decode") {
                return Some(packet.to_vec());
            }
            let read = self.stream.read(&mut self.buf).await.expect("read");
            if read == 0 {
                return None;
            }
            self.decoder.feed(&mut self.buf[..read]);
        }
    }
}

/// Starts a server context on an ephemeral port, serving exactly `connections`
/// clients before the task ends.
async fn spawn_server(config: ServerConfig, connections: usize) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let ctx: Arc<ServerContext> = Server::new(config).expect("valid config").context();

    tokio::spawn(async move {
        for _ in 0..connections {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            let ctx = Arc::clone(&ctx);
            tokio::spawn(rukkit_server::connection::handle(stream, peer, ctx));
        }
    });

    addr
}

fn test_config() -> ServerConfig {
    ServerConfig {
        bind: "127.0.0.1:0".into(),
        motd: "Rukkit integration test".into(),
        max_players: 4,
        ..ServerConfig::default()
    }
}

#[tokio::test]
async fn a_server_list_ping_returns_a_valid_status_document() {
    let addr = spawn_server(test_config(), 1).await;
    let mut client = TestClient::connect(addr).await;

    client
        .handshake(
            version::PROTOCOL_VERSION,
            handshake::NextState::Status,
            addr.port(),
        )
        .await;
    client
        .send_raw(&[ids::status::serverbound::STATUS_REQUEST as u8])
        .await;

    let packet = client.recv().await.expect("a status response");
    let mut r = PacketReader::new(&packet);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::status::clientbound::STATUS_RESPONSE
    );

    let json = r.read_string(DEFAULT_MAX_STRING_LEN).unwrap();
    let status: ServerStatus = serde_json::from_str(&json).expect("valid status JSON");

    assert_eq!(status.version.protocol, version::PROTOCOL_VERSION);
    assert_eq!(status.version.name, version::MINECRAFT_VERSION);
    assert_eq!(status.players.max, 4);
    assert_eq!(status.players.online, 0);
    assert_eq!(
        status.description.text.as_deref(),
        Some("Rukkit integration test")
    );
}

#[tokio::test]
async fn a_ping_payload_is_echoed_exactly() {
    let addr = spawn_server(test_config(), 1).await;
    let mut client = TestClient::connect(addr).await;

    client
        .handshake(
            version::PROTOCOL_VERSION,
            handshake::NextState::Status,
            addr.port(),
        )
        .await;
    client
        .send_raw(&[ids::status::serverbound::STATUS_REQUEST as u8])
        .await;
    client.recv().await.expect("status response");

    let payload: i64 = -0x0123_4567_89AB_CDEF;
    let mut packet = Vec::new();
    packet.write_varint(ids::status::serverbound::PING_REQUEST);
    // Disambiguated: tokio's AsyncWriteExt also defines `write_i64` on Vec<u8>.
    PacketWrite::write_i64(&mut packet, payload);
    client.send_raw(&packet).await;

    let response = client.recv().await.expect("a pong");
    let mut r = PacketReader::new(&response);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::status::clientbound::PONG_RESPONSE
    );
    assert_eq!(r.read_i64().unwrap(), payload);
}

#[tokio::test]
async fn a_ping_without_a_status_request_still_gets_a_pong() {
    // Some server-list implementations skip straight to the ping.
    let addr = spawn_server(test_config(), 1).await;
    let mut client = TestClient::connect(addr).await;

    client
        .handshake(
            version::PROTOCOL_VERSION,
            handshake::NextState::Status,
            addr.port(),
        )
        .await;

    let mut packet = Vec::new();
    packet.write_varint(ids::status::serverbound::PING_REQUEST);
    PacketWrite::write_i64(&mut packet, 99);
    client.send_raw(&packet).await;

    let response = client.recv().await.expect("a pong");
    let mut r = PacketReader::new(&response);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::status::clientbound::PONG_RESPONSE
    );
    assert_eq!(r.read_i64().unwrap(), 99);
}

#[tokio::test]
async fn a_mismatched_protocol_version_is_told_why() {
    let addr = spawn_server(test_config(), 1).await;
    let mut client = TestClient::connect(addr).await;

    client
        .handshake(
            version::PROTOCOL_VERSION - 1,
            handshake::NextState::Login,
            addr.port(),
        )
        .await;

    let packet = client.recv().await.expect("a disconnect");
    let mut r = PacketReader::new(&packet);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::login::clientbound::LOGIN_DISCONNECT
    );
    let reason = r.read_string(DEFAULT_MAX_STRING_LEN).unwrap();
    assert!(reason.contains("26.2"), "{reason}");
    assert!(
        reason.contains(&version::PROTOCOL_VERSION.to_string()),
        "{reason}"
    );
}

#[tokio::test]
async fn login_reaches_configuration_and_enables_compression() {
    let addr = spawn_server(test_config(), 1).await;
    let mut client = TestClient::connect(addr).await;

    client
        .handshake(
            version::PROTOCOL_VERSION,
            handshake::NextState::Login,
            addr.port(),
        )
        .await;

    let hello = login_packets::Hello {
        name: "tester".into(),
        profile_id: uuid::Uuid::nil(),
    };
    let mut packet = Vec::new();
    packet.write_varint(login_packets::Hello::ID);
    hello.encode_body(&mut packet);
    client.send_raw(&packet).await;

    // Compression is negotiated first, and applies from the next packet on.
    let set_compression = client.recv().await.expect("set compression");
    let mut r = PacketReader::new(&set_compression);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::login::clientbound::SET_COMPRESSION
    );
    let threshold = r.read_varint().unwrap();
    assert_eq!(threshold, 256);
    client.decoder.enable_compression(threshold as usize);
    client.encoder.enable_compression(threshold as usize);

    // The very next packet arrives compressed; decoding it proves the switch
    // happened on exactly the right byte on both sides.
    let success = client.recv().await.expect("login success");
    let mut r = PacketReader::new(&success);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::login::clientbound::LOGIN_SUCCESS
    );
    let uuid = r.read_uuid().unwrap();
    let name = r.read_string(16).unwrap();
    assert_eq!(name, "tester");
    assert_eq!(uuid, rukkit_server::offline_uuid("tester"));
    assert_eq!(r.read_varint().unwrap(), 0, "no skin properties");
    // The default config appends the trailing flag.
    assert_eq!(r.remaining(), 1, "expected the trailing flag");
    assert!(!r.read_bool().unwrap());

    // Acknowledge, and the server should move us into configuration.
    client
        .send_raw(&[ids::login::serverbound::LOGIN_ACKNOWLEDGED as u8])
        .await;

    let brand = client.recv().await.expect("brand plugin message");
    let mut r = PacketReader::new(&brand);
    assert_eq!(
        r.read_varint().unwrap(),
        ids::configuration::clientbound::PLUGIN_MESSAGE
    );
    assert_eq!(
        r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(),
        "minecraft:brand"
    );
    assert_eq!(r.read_string(DEFAULT_MAX_STRING_LEN).unwrap(), "Rukkit");
}

#[tokio::test]
async fn the_login_finished_trailing_flag_follows_the_config() {
    // A client that disagrees about this field rejects login_finished outright,
    // so both layouts have to be reachable from configuration alone.
    use rukkit_server::config::LoginFinishedFlag;

    for (flag, expected_trailing) in [
        (LoginFinishedFlag::Omit, 0usize),
        (LoginFinishedFlag::SendFalse, 1),
        (LoginFinishedFlag::SendTrue, 1),
    ] {
        let config = ServerConfig {
            login_finished_flag: flag,
            // Keep it uncompressed so the framing stays trivial here.
            compression_threshold: -1,
            ..test_config()
        };
        let addr = spawn_server(config, 1).await;
        let mut client = TestClient::connect(addr).await;

        client
            .handshake(
                version::PROTOCOL_VERSION,
                handshake::NextState::Login,
                addr.port(),
            )
            .await;

        let mut packet = Vec::new();
        packet.write_varint(login_packets::Hello::ID);
        login_packets::Hello {
            name: "tester".into(),
            profile_id: uuid::Uuid::nil(),
        }
        .encode_body(&mut packet);
        client.send_raw(&packet).await;

        let finished = client.recv().await.expect("login_finished");
        let mut r = PacketReader::new(&finished);
        assert_eq!(
            r.read_varint().unwrap(),
            ids::login::clientbound::LOGIN_SUCCESS
        );
        r.read_uuid().unwrap();
        assert_eq!(r.read_string(16).unwrap(), "tester");
        assert_eq!(r.read_varint().unwrap(), 0);
        assert_eq!(r.remaining(), expected_trailing, "layout for {flag:?}");

        if expected_trailing == 1 {
            assert_eq!(
                r.read_bool().unwrap(),
                flag == LoginFinishedFlag::SendTrue,
                "flag value for {flag:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_garbage_first_packet_is_dropped_without_taking_the_server_down() {
    let addr = spawn_server(test_config(), 2).await;

    {
        let mut client = TestClient::connect(addr).await;
        // A valid frame carrying a packet id that means nothing in handshaking.
        client.send_raw(&[0x7F, 0x01, 0x02]).await;
        // The server hangs up rather than answering.
        assert!(client.recv().await.is_none());
    }

    // The next client is served normally, proving the failure was contained.
    let mut client = TestClient::connect(addr).await;
    client
        .handshake(
            version::PROTOCOL_VERSION,
            handshake::NextState::Status,
            addr.port(),
        )
        .await;
    client
        .send_raw(&[ids::status::serverbound::STATUS_REQUEST as u8])
        .await;
    assert!(
        client.recv().await.is_some(),
        "server should still be serving"
    );
}
