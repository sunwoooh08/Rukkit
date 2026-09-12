//! The server-list response, cached.
//!
//! Pings are unauthenticated and cheap to send, so a public server answers far
//! more of them than logins — and every one of them would otherwise re-serialize
//! the same JSON. The document only changes when the player count does, so it is
//! built once per distinct count and shared as an `Arc<str>` from there on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use rukkit_protocol::packets::status::{PlayersInfo, SamplePlayer, ServerStatus, VersionInfo};
use rukkit_protocol::text::Component;
use uuid::Uuid;

/// Maximum names listed in the hover sample.
const SAMPLE_LIMIT: usize = 12;

#[derive(Debug)]
struct Cached {
    online: i32,
    sample: Vec<String>,
    json: Arc<str>,
}

/// Builds and caches the status document.
#[derive(Debug)]
pub struct StatusCache {
    description: Component,
    max_players: i32,
    favicon: Option<String>,
    enforces_secure_chat: bool,
    cached: RwLock<Option<Cached>>,
    rebuilds: AtomicU64,
}

impl StatusCache {
    #[must_use]
    pub fn new(description: Component, max_players: i32) -> Self {
        Self {
            description,
            max_players,
            favicon: None,
            enforces_secure_chat: false,
            cached: RwLock::new(None),
            rebuilds: AtomicU64::new(0),
        }
    }

    /// Attaches a `data:image/png;base64,...` icon.
    #[must_use]
    pub fn with_favicon(mut self, favicon: Option<String>) -> Self {
        self.favicon = favicon;
        self
    }

    /// How many times the JSON has actually been serialized.
    #[must_use]
    pub fn rebuild_count(&self) -> u64 {
        self.rebuilds.load(Ordering::Relaxed)
    }

    /// The status JSON for the given player count and sample.
    pub fn json(&self, online: i32, sample: &[String]) -> Arc<str> {
        if let Some(cached) = self.cached.read().as_ref() {
            if cached.online == online && cached.sample == sample {
                return Arc::clone(&cached.json);
            }
        }

        let mut guard = self.cached.write();
        // Another thread may have rebuilt it while the write lock was contended.
        if let Some(cached) = guard.as_ref() {
            if cached.online == online && cached.sample == sample {
                return Arc::clone(&cached.json);
            }
        }

        let json: Arc<str> = Arc::from(self.build(online, sample).to_json());
        self.rebuilds.fetch_add(1, Ordering::Relaxed);
        *guard = Some(Cached {
            online,
            sample: sample.to_vec(),
            json: Arc::clone(&json),
        });
        json
    }

    fn build(&self, online: i32, sample: &[String]) -> ServerStatus {
        ServerStatus {
            version: VersionInfo::default(),
            players: PlayersInfo {
                max: self.max_players,
                online,
                sample: sample
                    .iter()
                    .take(SAMPLE_LIMIT)
                    .map(|name| SamplePlayer {
                        name: name.clone(),
                        // The sample entry's id is cosmetic; a stable offline
                        // id keeps it consistent between pings.
                        id: crate::player::offline_uuid(name).to_string(),
                    })
                    .collect(),
            },
            description: self.description.clone(),
            favicon: self.favicon.clone(),
            enforces_secure_chat: self.enforces_secure_chat,
        }
    }
}

/// Parses a favicon file into the data URI the protocol expects.
///
/// Vanilla clients require exactly 64x64 PNG; anything else is ignored rather
/// than shown broken, so an unreadable file is a warning, not an error.
#[must_use]
pub fn favicon_data_uri(png: &[u8]) -> Option<String> {
    if !png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return None;
    }
    Some(format!("data:image/png;base64,{}", base64_encode(png)))
}

/// Minimal base64, to avoid a dependency for one 4 KiB icon.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

/// A deterministic id for a sample entry, exposed for tests.
#[must_use]
pub fn sample_id(name: &str) -> Uuid {
    crate::player::offline_uuid(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> StatusCache {
        StatusCache::new(Component::text("A Rukkit server"), 20)
    }

    #[test]
    fn reports_the_targeted_version_and_counts() {
        let cache = cache();
        let json = cache.json(3, &[]);
        assert!(json.contains(r#""protocol":776"#), "{json}");
        assert!(json.contains(r#""name":"26.2""#), "{json}");
        assert!(json.contains(r#""online":3"#), "{json}");
        assert!(json.contains(r#""max":20"#), "{json}");
    }

    #[test]
    fn repeated_pings_reuse_one_serialization() {
        let cache = cache();
        for _ in 0..100 {
            let _ = cache.json(0, &[]);
        }
        assert_eq!(cache.rebuild_count(), 1, "cache should have held");
    }

    #[test]
    fn a_changed_player_count_rebuilds() {
        let cache = cache();
        let _ = cache.json(0, &[]);
        let _ = cache.json(1, &[]);
        let _ = cache.json(1, &[]);
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn a_changed_sample_rebuilds() {
        let cache = cache();
        let _ = cache.json(1, &["alice".to_owned()]);
        let _ = cache.json(1, &["bob".to_owned()]);
        assert_eq!(cache.rebuild_count(), 2);
    }

    #[test]
    fn cached_documents_are_shared_not_copied() {
        let cache = cache();
        let a = cache.json(0, &[]);
        let b = cache.json(0, &[]);
        assert!(Arc::ptr_eq(&a, &b), "should hand out the same allocation");
    }

    #[test]
    fn the_sample_is_capped() {
        let cache = cache();
        let names: Vec<String> = (0..50).map(|i| format!("player{i}")).collect();
        let json = cache.json(50, &names);
        let parsed: ServerStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.players.sample.len(), SAMPLE_LIMIT);
        assert_eq!(parsed.players.online, 50);
    }

    #[test]
    fn an_absent_favicon_is_omitted_entirely() {
        let json = cache().json(0, &[]);
        assert!(!json.contains("favicon"), "{json}");
    }

    #[test]
    fn a_favicon_is_embedded_as_a_data_uri() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let uri = favicon_data_uri(&png).expect("valid PNG header");
        let cache = cache().with_favicon(Some(uri.clone()));
        let json = cache.json(0, &[]);
        assert!(json.contains("data:image/png;base64,"), "{json}");
        assert!(
            uri.starts_with("data:image/png;base64,iVBORw0KGgo"),
            "{uri}"
        );
    }

    #[test]
    fn a_non_png_favicon_is_refused() {
        assert!(favicon_data_uri(b"not a png").is_none());
        assert!(favicon_data_uri(&[]).is_none());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn sample_ids_are_stable() {
        assert_eq!(sample_id("alice"), sample_id("alice"));
    }

    #[test]
    fn the_document_parses_back_as_valid_json() {
        let cache = cache();
        let json = cache.json(7, &["alice".to_owned()]);
        let parsed: ServerStatus = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed.players.online, 7);
        assert_eq!(parsed.players.sample[0].name, "alice");
    }
}
