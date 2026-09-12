//! Server assembly: the accept loop, the tick loop, and shutdown.

use std::sync::Arc;
use std::time::Instant;

use rukkit_protocol::text::Component;
use rukkit_protocol::{version, ChunkPos};
use rukkit_world::generator::{ChunkGenerator, FlatGenerator, VoidGenerator};
use rukkit_world::{BlockStates, HeightLimits};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::config::ServerConfig;
use crate::connection;
use crate::player::PlayerRegistry;
use crate::status::StatusCache;
use crate::tick::TickMetrics;

/// How many tick samples to keep: one minute at 20 Hz.
const TICK_WINDOW: usize = 1200;

/// How often to log tick health.
const TICK_REPORT_INTERVAL: u64 = 20 * 30;

/// Everything a connection task needs, shared immutably.
#[derive(Debug)]
pub struct ServerContext {
    pub config: ServerConfig,
    pub players: PlayerRegistry,
    pub status: StatusCache,
    pub states: BlockStates,
    pub limits: HeightLimits,
}

/// The server itself.
#[derive(Debug)]
pub struct Server {
    ctx: Arc<ServerContext>,
}

impl Server {
    /// Builds a server from validated configuration.
    pub fn new(config: ServerConfig) -> Result<Self, crate::config::ConfigError> {
        config.validate()?;
        let status = StatusCache::new(Component::text(config.motd.clone()), config.max_players);
        Ok(Self {
            ctx: Arc::new(ServerContext {
                config,
                players: PlayerRegistry::new(),
                status,
                states: BlockStates::placeholder(),
                limits: HeightLimits::OVERWORLD,
            }),
        })
    }

    #[must_use]
    pub fn context(&self) -> Arc<ServerContext> {
        Arc::clone(&self.ctx)
    }

    /// Runs until interrupted.
    pub async fn run(self) -> anyhow::Result<()> {
        let addr = self.ctx.config.socket_addr()?;
        let listener = TcpListener::bind(addr).await?;

        tracing::info!(
            "Rukkit listening on {addr} for Minecraft {} (protocol {})",
            version::MINECRAFT_VERSION,
            version::PROTOCOL_VERSION
        );
        self.report_spawn_chunk();

        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        let tick_ctx = Arc::clone(&self.ctx);
        let tick_shutdown = shutdown_tx.subscribe();
        let ticker = tokio::spawn(async move { tick_loop(tick_ctx, tick_shutdown).await });

        let mut accept_shutdown = shutdown_tx.subscribe();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let ctx = Arc::clone(&self.ctx);
                            // One task per connection. A panic in a malformed
                            // packet path takes down that task only, which is
                            // why the release profile unwinds rather than aborts.
                            tokio::spawn(connection::handle(stream, peer, ctx));
                        }
                        Err(e) => {
                            // Per-connection accept errors (a peer that hung up
                            // mid-handshake, a transient fd limit) must not end
                            // the server.
                            tracing::warn!("accept failed: {e}");
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("shutdown requested");
                    let _ = shutdown_tx.send(());
                    break;
                }
                _ = accept_shutdown.recv() => break,
            }
        }

        let _ = ticker.await;
        tracing::info!("server stopped");
        Ok(())
    }

    /// Generates the spawn chunk once at startup.
    ///
    /// Cheap, and it surfaces the world layer's real numbers in the log rather
    /// than leaving them to be guessed at.
    fn report_spawn_chunk(&self) {
        let started = Instant::now();
        let chunk = if self.ctx.config.flat_world {
            FlatGenerator::default().generate(
                ChunkPos::new(0, 0),
                self.ctx.limits,
                &self.ctx.states,
            )
        } else {
            VoidGenerator.generate(ChunkPos::new(0, 0), self.ctx.limits, &self.ctx.states)
        };
        let elapsed = started.elapsed();

        let mut encoded = Vec::new();
        chunk.write_sections(&self.ctx.states, &mut encoded);
        tracing::info!(
            "spawn chunk generated in {:.2?}, {} sections serialize to {} bytes",
            elapsed,
            chunk.sections().len(),
            encoded.len()
        );
    }
}

async fn tick_loop(ctx: Arc<ServerContext>, mut shutdown: broadcast::Receiver<()>) {
    let target = ctx.config.tick_duration();
    let mut interval = tokio::time::interval(target);
    // After a lag spike, catch up by skipping rather than by running a burst of
    // back-to-back ticks, which would only deepen the spike.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut metrics = TickMetrics::new(target, TICK_WINDOW);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let started = Instant::now();
                tick(&ctx);
                metrics.record(started.elapsed());

                if metrics.total_ticks() % TICK_REPORT_INTERVAL == 0 {
                    tracing::info!("{}", metrics.summary());
                }
            }
            _ = shutdown.recv() => {
                tracing::info!("tick loop stopping: {}", metrics.summary());
                return;
            }
        }
    }
}

/// One server tick.
///
/// Deliberately empty of game logic for now: the loop, its timing and its
/// health reporting are what this build provides, and filling it in is the
/// next layer's job rather than something to fake here.
fn tick(_ctx: &ServerContext) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ServerConfig {
        ServerConfig {
            bind: "127.0.0.1:0".into(),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn a_server_can_be_built_from_defaults() {
        let server = Server::new(test_config()).expect("valid config");
        assert_eq!(server.context().players.count(), 0);
    }

    #[test]
    fn an_invalid_config_is_refused_at_construction() {
        let config = ServerConfig {
            tick_rate: 0,
            ..test_config()
        };
        assert!(Server::new(config).is_err());
    }

    #[test]
    fn the_status_document_reflects_the_configured_motd() {
        let config = ServerConfig {
            motd: "Rukkit test".into(),
            ..test_config()
        };
        let server = Server::new(config).unwrap();
        let json = server.context().status.json(0, &[]);
        assert!(json.contains("Rukkit test"), "{json}");
    }

    #[tokio::test]
    async fn the_listener_binds_and_accepts() {
        // Proves the accept path works end to end without needing a real client.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = tokio::spawn(async move { listener.accept().await.map(|(_, peer)| peer) });

        let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let peer = accepted.await.unwrap().unwrap();
        assert_eq!(peer.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn the_tick_loop_runs_and_stops_on_shutdown() {
        let config = ServerConfig {
            tick_rate: 200, // fast ticks so the test stays brief
            ..test_config()
        };
        let server = Server::new(config).unwrap();
        let ctx = server.context();

        let (tx, rx) = broadcast::channel::<()>(1);
        let handle = tokio::spawn(tick_loop(ctx, rx));

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("tick loop should stop promptly")
            .expect("tick loop should not panic");
    }
}
