//! Rukkit server runtime.
//!
//! Ties the protocol and world crates to a Tokio runtime: an accept loop, a
//! per-connection state machine, a fixed-rate tick loop, and the bookkeeping
//! around them.

pub mod config;
pub mod connection;
pub mod player;
pub mod server;
pub mod status;
pub mod tick;

pub use config::ServerConfig;
pub use player::{offline_uuid, PlayerInfo, PlayerRegistry};
pub use server::{Server, ServerContext};
pub use tick::TickMetrics;
