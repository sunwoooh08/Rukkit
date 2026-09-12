//! Connected-player bookkeeping.

use std::collections::HashMap;
use std::net::SocketAddr;

use md5::{Digest, Md5};
use parking_lot::RwLock;
use uuid::Uuid;

/// A player's identity for the life of their connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub uuid: Uuid,
    pub name: String,
    pub addr: SocketAddr,
}

/// Why a login was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    #[error("The server is full")]
    Full,
    #[error("A player with that name is already connected")]
    NameTaken,
}

/// The set of players currently connected.
///
/// Reads (status pings, broadcasts) vastly outnumber writes (joins and quits),
/// so this is an `RwLock` rather than a `Mutex`, and the lock is never held
/// across an await point — every method returns owned data.
#[derive(Debug, Default)]
pub struct PlayerRegistry {
    players: RwLock<HashMap<Uuid, PlayerInfo>>,
}

impl PlayerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Admits a player, or explains why not.
    ///
    /// The capacity and duplicate-name checks happen under one write lock, so
    /// two simultaneous logins cannot both pass a check that only one should.
    pub fn try_insert(&self, info: PlayerInfo, max_players: usize) -> Result<(), JoinError> {
        let mut players = self.players.write();
        if players.len() >= max_players {
            return Err(JoinError::Full);
        }
        if players.values().any(|p| p.name == info.name) {
            return Err(JoinError::NameTaken);
        }
        players.insert(info.uuid, info);
        Ok(())
    }

    pub fn remove(&self, uuid: Uuid) -> Option<PlayerInfo> {
        self.players.write().remove(&uuid)
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.players.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.players.read().is_empty()
    }

    #[must_use]
    pub fn get(&self, uuid: Uuid) -> Option<PlayerInfo> {
        self.players.read().get(&uuid).cloned()
    }

    #[must_use]
    pub fn contains_name(&self, name: &str) -> bool {
        self.players.read().values().any(|p| p.name == name)
    }

    /// A snapshot of everyone connected.
    #[must_use]
    pub fn snapshot(&self) -> Vec<PlayerInfo> {
        self.players.read().values().cloned().collect()
    }

    /// Up to `limit` names, for the server list sample.
    #[must_use]
    pub fn sample_names(&self, limit: usize) -> Vec<String> {
        self.players
            .read()
            .values()
            .take(limit)
            .map(|p| p.name.clone())
            .collect()
    }
}

/// The UUID an offline-mode player is given.
///
/// Reproduces Java's `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")` exactly —
/// an MD5 digest with the version-3 and IETF variant bits forced — so a world
/// created here keeps working on a vanilla or Paper server and vice versa.
/// Getting this subtly wrong orphans every player's inventory.
#[must_use]
pub fn offline_uuid(name: &str) -> Uuid {
    let digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes: [u8; 16] = digest.into();
    bytes[6] = (bytes[6] & 0x0F) | 0x30;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str) -> PlayerInfo {
        PlayerInfo {
            uuid: offline_uuid(name),
            name: name.to_owned(),
            addr: "127.0.0.1:1234".parse().unwrap(),
        }
    }

    #[test]
    fn offline_uuid_matches_javas_name_uuid_from_bytes() {
        // Cross-checked against Java's UUID.nameUUIDFromBytes on JDK 25, the
        // same values vanilla and Paper assign in offline mode.
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(
            offline_uuid("jeb_").to_string(),
            "a762f560-4fce-3236-812a-b80efff0b62b"
        );
        assert_eq!(
            offline_uuid("anyone").to_string(),
            "7e17b10e-96f1-3ec0-997a-039cf5eb65a5"
        );
    }

    #[test]
    fn offline_uuid_has_version_three_and_the_ietf_variant() {
        let uuid = offline_uuid("anyone");
        assert_eq!(uuid.get_version_num(), 3);
        let bytes = uuid.as_bytes();
        assert_eq!(bytes[8] & 0xC0, 0x80, "variant bits");
    }

    #[test]
    fn offline_uuid_is_stable_and_name_specific() {
        assert_eq!(offline_uuid("player"), offline_uuid("player"));
        assert_ne!(offline_uuid("player"), offline_uuid("Player"));
    }

    #[test]
    fn players_can_join_and_leave() {
        let registry = PlayerRegistry::new();
        assert!(registry.is_empty());

        registry.try_insert(info("alice"), 10).unwrap();
        assert_eq!(registry.count(), 1);
        assert!(registry.contains_name("alice"));

        let removed = registry.remove(offline_uuid("alice")).unwrap();
        assert_eq!(removed.name, "alice");
        assert!(registry.is_empty());
    }

    #[test]
    fn a_full_server_refuses_new_players() {
        let registry = PlayerRegistry::new();
        registry.try_insert(info("alice"), 2).unwrap();
        registry.try_insert(info("bob"), 2).unwrap();
        assert_eq!(
            registry.try_insert(info("carol"), 2).unwrap_err(),
            JoinError::Full
        );
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn duplicate_names_are_refused() {
        let registry = PlayerRegistry::new();
        registry.try_insert(info("alice"), 10).unwrap();
        assert_eq!(
            registry.try_insert(info("alice"), 10).unwrap_err(),
            JoinError::NameTaken
        );
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn capacity_of_zero_admits_nobody() {
        let registry = PlayerRegistry::new();
        assert_eq!(
            registry.try_insert(info("alice"), 0).unwrap_err(),
            JoinError::Full
        );
    }

    #[test]
    fn removing_someone_absent_is_harmless() {
        let registry = PlayerRegistry::new();
        assert!(registry.remove(offline_uuid("ghost")).is_none());
    }

    #[test]
    fn sample_is_bounded_by_the_limit() {
        let registry = PlayerRegistry::new();
        for i in 0..10 {
            registry.try_insert(info(&format!("p{i}")), 20).unwrap();
        }
        assert_eq!(registry.sample_names(3).len(), 3);
        assert_eq!(registry.sample_names(100).len(), 10);
    }

    #[test]
    fn concurrent_joins_cannot_exceed_capacity() {
        use std::sync::Arc;

        let registry = Arc::new(PlayerRegistry::new());
        let max = 8usize;
        let mut handles = Vec::new();

        for i in 0..64 {
            let registry = Arc::clone(&registry);
            handles.push(std::thread::spawn(move || {
                registry.try_insert(info(&format!("p{i}")), max).is_ok()
            }));
        }

        let admitted = handles
            .into_iter()
            .filter(|_| true)
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();

        assert_eq!(admitted, max);
        assert_eq!(registry.count(), max);
    }
}
