//! The `rukkit` binary.

use std::path::PathBuf;

use rukkit_server::{Server, ServerConfig};
use tracing_subscriber::EnvFilter;

const DEFAULT_CONFIG: &str = "rukkit.toml";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let config_path = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_CONFIG), PathBuf::from);

    let config = ServerConfig::load_or_create(&config_path)?;
    tracing::info!("loaded configuration from {}", config_path.display());

    // The runtime is built by hand rather than via the attribute macro so the
    // worker count can be reported, which is the first thing to check when
    // throughput looks wrong.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("rukkit-worker")
        .build()?;
    tracing::info!(
        "tokio runtime started on {} worker threads",
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    );

    runtime.block_on(async move { Server::new(config)?.run().await })
}
