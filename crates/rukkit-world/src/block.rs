//! Block state identifiers.
//!
//! Block *state* ids are assigned by the game's own registry and change between
//! versions, so they are data, not code. This module keeps the small set the
//! built-in generators need behind a struct that a real registry load can
//! replace wholesale, rather than scattering magic numbers through the world
//! code.

use crate::palette::bits_for;

/// The block states the built-in generators refer to, plus the registry size
/// that fixes the direct-palette width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStates {
    pub air: u32,
    pub stone: u32,
    pub dirt: u32,
    pub grass_block: u32,
    pub bedrock: u32,
    /// Total number of block states in the registry.
    pub total_states: u32,
}

impl BlockStates {
    /// Placeholder ids for running without a loaded registry.
    ///
    /// `air` is 0, which the format genuinely guarantees — an all-zero section
    /// is an empty one. The rest are sequential stand-ins: they exercise every
    /// storage path correctly, but a real client needs ids from the version's
    /// block report to render the right blocks.
    #[must_use]
    pub const fn placeholder() -> Self {
        Self {
            air: 0,
            stone: 1,
            dirt: 2,
            grass_block: 3,
            bedrock: 4,
            // Roughly the order of magnitude of a modern registry, which is
            // what decides the direct palette width (15 bits).
            total_states: 27_000,
        }
    }

    /// Bit width the direct palette uses, from the registry size.
    #[must_use]
    pub fn direct_bits(&self) -> u32 {
        bits_for(self.total_states as usize)
    }

    /// Whether a state is air, i.e. does not count toward a section's block
    /// count and does not block light.
    #[inline]
    #[must_use]
    pub const fn is_air(&self, state: u32) -> bool {
        state == self.air
    }
}

impl Default for BlockStates {
    fn default() -> Self {
        Self::placeholder()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_is_zero_so_empty_sections_are_all_zero_bits() {
        assert_eq!(BlockStates::placeholder().air, 0);
    }

    #[test]
    fn direct_width_covers_the_registry() {
        let states = BlockStates::placeholder();
        let bits = states.direct_bits();
        assert_eq!(bits, 15);
        assert!(
            1u32 << bits >= states.total_states,
            "{bits} bits cannot index {} states",
            states.total_states
        );
    }

    #[test]
    fn is_air_only_matches_air() {
        let states = BlockStates::placeholder();
        assert!(states.is_air(states.air));
        assert!(!states.is_air(states.stone));
        assert!(!states.is_air(states.bedrock));
    }
}
