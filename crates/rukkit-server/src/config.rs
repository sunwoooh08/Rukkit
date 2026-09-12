//! Server configuration, loaded from `rukkit.toml`.

use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Everything an operator can tune.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address to listen on.
    pub bind: String,
    /// Message shown in the server list.
    pub motd: String,
    pub max_players: i32,
    /// Chunks sent around each player.
    pub view_distance: u8,
    /// Chunks ticked around each player. Clamped to `view_distance`.
    pub simulation_distance: u8,
    /// Packets at or above this size are compressed. Negative disables.
    pub compression_threshold: i32,
    /// Whether to verify identities against Mojang's session servers.
    pub online_mode: bool,
    /// Target ticks per second.
    pub tick_rate: u32,
    /// How often to probe clients for liveness.
    pub keepalive_interval_secs: u64,
    /// Drop a client that has not answered a keep-alive within this long.
    pub keepalive_timeout_secs: u64,
    /// Generate a superflat world rather than an empty one.
    pub flat_world: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:25565".to_owned(),
            motd: "A Rukkit server".to_owned(),
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            compression_threshold: 256,
            // Offline by default: online mode needs session-server access, and
            // an operator should opt into it knowingly.
            online_mode: false,
            tick_rate: 20,
            keepalive_interval_secs: 15,
            keepalive_timeout_secs: 30,
            flat_world: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

impl ServerConfig {
    /// Loads from `path`, writing out the defaults if it does not exist.
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let display = path.display().to_string();

        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
                    path: display,
                    source,
                })?;
                config.validate()?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Self::default();
                let text = toml::to_string_pretty(&config)
                    .expect("the default config is always serializable");
                std::fs::write(path, text).map_err(|source| ConfigError::Write {
                    path: display,
                    source,
                })?;
                Ok(config)
            }
            Err(source) => Err(ConfigError::Read {
                path: display,
                source,
            }),
        }
    }

    /// Rejects settings that would misbehave at runtime rather than letting
    /// them fail obscurely later.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.socket_addr()?;
        if self.tick_rate == 0 {
            return Err(ConfigError::Invalid("tick_rate must be at least 1".into()));
        }
        if self.view_distance < 2 {
            return Err(ConfigError::Invalid(
                "view_distance must be at least 2".into(),
            ));
        }
        if self.max_players < 0 {
            return Err(ConfigError::Invalid(
                "max_players cannot be negative".into(),
            ));
        }
        if self.keepalive_timeout_secs <= self.keepalive_interval_secs {
            return Err(ConfigError::Invalid(
                "keepalive_timeout_secs must exceed keepalive_interval_secs".into(),
            ));
        }
        Ok(())
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.bind
            .parse()
            .map_err(|_| ConfigError::Invalid(format!("`{}` is not a valid address", self.bind)))
    }

    /// Simulation distance never usefully exceeds view distance.
    #[must_use]
    pub fn effective_simulation_distance(&self) -> u8 {
        self.simulation_distance.min(self.view_distance)
    }

    /// Compression threshold as an optional size, `None` when disabled.
    #[must_use]
    pub fn compression(&self) -> Option<usize> {
        if self.compression_threshold < 0 {
            None
        } else {
            Some(self.compression_threshold as usize)
        }
    }

    /// Duration of one tick at the configured rate.
    #[must_use]
    pub fn tick_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / f64::from(self.tick_rate.max(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        ServerConfig::default()
            .validate()
            .expect("defaults must pass");
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let original = ServerConfig::default();
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: ServerConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_partial_file_fills_in_defaults() {
        let parsed: ServerConfig = toml::from_str(r#"motd = "custom""#).unwrap();
        assert_eq!(parsed.motd, "custom");
        assert_eq!(parsed.max_players, ServerConfig::default().max_players);
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_silently_ignored() {
        // A typo'd key that quietly did nothing would be worse than an error.
        let err = toml::from_str::<ServerConfig>(r#"motdd = "typo""#);
        assert!(err.is_err());
    }

    #[test]
    fn bad_bind_address_is_caught_by_validation() {
        let mut config = ServerConfig {
            bind: "not-an-address".into(),
            ..ServerConfig::default()
        };
        assert!(config.validate().is_err());

        config.bind = "127.0.0.1:25565".into();
        assert!(config.validate().is_ok());
        assert_eq!(config.socket_addr().unwrap().port(), 25565);
    }

    #[test]
    fn zero_tick_rate_is_rejected() {
        let config = ServerConfig {
            tick_rate: 0,
            ..ServerConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn keepalive_timeout_must_exceed_the_interval() {
        let defaults = ServerConfig::default();
        let mut config = ServerConfig {
            keepalive_timeout_secs: defaults.keepalive_interval_secs,
            ..defaults
        };
        assert!(config.validate().is_err());

        config.keepalive_timeout_secs = config.keepalive_interval_secs + 1;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn simulation_distance_is_clamped_to_view_distance() {
        let mut config = ServerConfig {
            view_distance: 6,
            simulation_distance: 16,
            ..ServerConfig::default()
        };
        assert_eq!(config.effective_simulation_distance(), 6);

        config.simulation_distance = 4;
        assert_eq!(config.effective_simulation_distance(), 4);
    }

    #[test]
    fn negative_threshold_disables_compression() {
        let mut config = ServerConfig::default();
        assert_eq!(config.compression(), Some(256), "the default threshold");
        config.compression_threshold = -1;
        assert_eq!(config.compression(), None);
    }

    #[test]
    fn tick_duration_matches_the_rate() {
        let mut config = ServerConfig::default();
        assert_eq!(config.tick_duration().as_millis(), 50);
        config.tick_rate = 40;
        assert_eq!(config.tick_duration().as_millis(), 25);
    }
}
