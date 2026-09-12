//! Small value types that appear directly in packet fields.

use crate::error::{ProtocolError, Result};

/// A block position in world space.
///
/// On the wire this is packed into a single `i64` as 26 bits of X, 26 bits of
/// Z and 12 bits of Y, in that order from the most significant bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Packs into the wire representation.
    #[inline]
    #[must_use]
    pub const fn encode(self) -> i64 {
        ((self.x as i64 & 0x3FF_FFFF) << 38)
            | ((self.z as i64 & 0x3FF_FFFF) << 12)
            | (self.y as i64 & 0xFFF)
    }

    /// Unpacks from the wire representation, sign-extending each field.
    #[inline]
    #[must_use]
    pub const fn decode(packed: i64) -> Self {
        Self {
            x: (packed >> 38) as i32,
            y: ((packed << 52) >> 52) as i32,
            z: ((packed << 26) >> 38) as i32,
        }
    }

    /// The chunk column containing this block.
    #[inline]
    #[must_use]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x >> 4,
            z: self.z >> 4,
        }
    }
}

/// A chunk column coordinate, i.e. a block position divided by 16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    #[inline]
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Packs both coordinates into one `i64`, the form used as a hash key for
    /// chunk maps (and by vanilla's `ChunkPos.toLong`).
    #[inline]
    #[must_use]
    pub const fn to_long(self) -> i64 {
        (self.x as i64 & 0xFFFF_FFFF) | ((self.z as i64 & 0xFFFF_FFFF) << 32)
    }

    #[inline]
    #[must_use]
    pub const fn from_long(packed: i64) -> Self {
        Self {
            x: packed as i32,
            z: (packed >> 32) as i32,
        }
    }

    /// Chebyshev distance in chunks, which is how view distance is measured.
    #[inline]
    #[must_use]
    pub const fn chebyshev_distance(self, other: Self) -> i32 {
        let dx = (self.x - other.x).abs();
        let dz = (self.z - other.z).abs();
        if dx > dz {
            dx
        } else {
            dz
        }
    }
}

/// Rotation encoded as a single byte covering a full turn (256 steps).
#[inline]
#[must_use]
pub fn angle_to_byte(degrees: f32) -> u8 {
    (degrees * (256.0 / 360.0)).round() as i32 as u8
}

/// Inverse of [`angle_to_byte`].
#[inline]
#[must_use]
pub fn byte_to_angle(byte: u8) -> f32 {
    f32::from(byte) * (360.0 / 256.0)
}

/// A player's interaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum GameMode {
    #[default]
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    #[inline]
    pub fn from_id(id: i32) -> Result<Self> {
        Ok(match id {
            0 => Self::Survival,
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            other => {
                return Err(ProtocolError::InvalidEnum {
                    kind: "GameMode",
                    value: i64::from(other),
                })
            }
        })
    }

    #[inline]
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

/// World difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Difficulty {
    Peaceful = 0,
    #[default]
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    #[inline]
    pub fn from_id(id: u8) -> Result<Self> {
        Ok(match id {
            0 => Self::Peaceful,
            1 => Self::Easy,
            2 => Self::Normal,
            3 => Self::Hard,
            other => {
                return Err(ProtocolError::InvalidEnum {
                    kind: "Difficulty",
                    value: i64::from(other),
                })
            }
        })
    }

    #[inline]
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_pos_round_trips_including_negatives() {
        let cases = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 2, 3),
            BlockPos::new(-1, -1, -1),
            BlockPos::new(33_554_431, 2047, 33_554_431),
            BlockPos::new(-33_554_432, -2048, -33_554_432),
            BlockPos::new(1_000_000, -64, -1_000_000),
        ];
        for pos in cases {
            assert_eq!(BlockPos::decode(pos.encode()), pos, "round trip {pos:?}");
        }
    }

    #[test]
    fn block_pos_packing_matches_vanilla_layout() {
        // x=1, y=2, z=3 packs to the documented bit pattern.
        let packed = BlockPos::new(1, 2, 3).encode();
        assert_eq!(packed, (1i64 << 38) | (3i64 << 12) | 2);
    }

    #[test]
    fn chunk_pos_long_round_trips() {
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-1, 5),
            ChunkPos::new(i32::MIN, i32::MAX),
        ] {
            assert_eq!(ChunkPos::from_long(pos.to_long()), pos);
        }
    }

    #[test]
    fn block_to_chunk_floors_toward_negative_infinity() {
        assert_eq!(BlockPos::new(0, 0, 0).chunk(), ChunkPos::new(0, 0));
        assert_eq!(BlockPos::new(15, 0, 15).chunk(), ChunkPos::new(0, 0));
        assert_eq!(BlockPos::new(16, 0, 16).chunk(), ChunkPos::new(1, 1));
        // -1 must land in chunk -1, not chunk 0.
        assert_eq!(BlockPos::new(-1, 0, -1).chunk(), ChunkPos::new(-1, -1));
        assert_eq!(BlockPos::new(-16, 0, -16).chunk(), ChunkPos::new(-1, -1));
        assert_eq!(BlockPos::new(-17, 0, -17).chunk(), ChunkPos::new(-2, -2));
    }

    #[test]
    fn view_distance_uses_chebyshev_not_euclidean() {
        let origin = ChunkPos::new(0, 0);
        assert_eq!(origin.chebyshev_distance(ChunkPos::new(3, 4)), 4);
        assert_eq!(origin.chebyshev_distance(ChunkPos::new(-5, 2)), 5);
    }

    #[test]
    fn angles_survive_a_byte_round_trip() {
        for degrees in [0.0f32, 90.0, 180.0, 270.0] {
            let back = byte_to_angle(angle_to_byte(degrees));
            assert!((back - degrees).abs() < 1.5, "{degrees} -> {back}");
        }
    }

    #[test]
    fn invalid_enum_ids_are_rejected() {
        assert!(GameMode::from_id(4).is_err());
        assert!(GameMode::from_id(-1).is_err());
        assert!(Difficulty::from_id(9).is_err());
    }
}
