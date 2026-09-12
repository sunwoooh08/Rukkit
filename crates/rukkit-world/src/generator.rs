//! Chunk generation.
//!
//! Only deterministic, cheap generators live here. Real terrain generation is
//! a separate concern that belongs behind this trait, not inside it.

use rukkit_protocol::ChunkPos;

use crate::block::BlockStates;
use crate::chunk::{Chunk, HeightLimits, SECTION_SIZE};

/// Produces chunks on demand.
pub trait ChunkGenerator: Send + Sync {
    fn generate(&self, pos: ChunkPos, limits: HeightLimits, states: &BlockStates) -> Chunk;
}

/// An empty world.
#[derive(Debug, Clone, Copy, Default)]
pub struct VoidGenerator;

impl ChunkGenerator for VoidGenerator {
    fn generate(&self, pos: ChunkPos, limits: HeightLimits, states: &BlockStates) -> Chunk {
        Chunk::empty(pos, limits, states)
    }
}

/// A superflat world: bedrock at the bottom, stone, then dirt and grass.
#[derive(Debug, Clone, Copy)]
pub struct FlatGenerator {
    /// Y of the topmost (grass) layer.
    pub surface_y: i32,
    /// How many dirt layers sit directly under the surface.
    pub dirt_depth: i32,
}

impl Default for FlatGenerator {
    fn default() -> Self {
        Self {
            surface_y: -60,
            dirt_depth: 2,
        }
    }
}

impl FlatGenerator {
    /// The block that belongs at absolute height `y`.
    fn block_at(&self, y: i32, limits: HeightLimits, states: &BlockStates) -> u32 {
        if y == limits.min_y {
            states.bedrock
        } else if y == self.surface_y {
            states.grass_block
        } else if y < self.surface_y && y >= self.surface_y - self.dirt_depth {
            states.dirt
        } else if y < self.surface_y {
            states.stone
        } else {
            states.air
        }
    }
}

impl ChunkGenerator for FlatGenerator {
    fn generate(&self, pos: ChunkPos, limits: HeightLimits, states: &BlockStates) -> Chunk {
        let mut chunk = Chunk::empty(pos, limits, states);

        for index in 0..limits.section_count() {
            let base_y = limits.min_y + (index * SECTION_SIZE) as i32;
            let top_y = base_y + SECTION_SIZE as i32 - 1;

            // A section whose whole range maps to one block is filled in one
            // step instead of 4096 writes. In a superflat world that covers
            // every section except the two or three around the surface.
            let first = self.block_at(base_y, limits, states);
            let uniform = (base_y..=top_y).all(|y| self.block_at(y, limits, states) == first);

            let section = chunk
                .section_mut(index)
                .expect("section index derived from the limits");

            if uniform {
                section.fill_blocks(first);
                continue;
            }

            for local_y in 0..SECTION_SIZE {
                let state = self.block_at(base_y + local_y as i32, limits, states);
                if states.is_air(state) {
                    continue;
                }
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        section.set_block(x, local_y, z, state);
                    }
                }
            }
        }

        chunk.recompute_heightmaps(states);
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states() -> BlockStates {
        BlockStates::placeholder()
    }

    #[test]
    fn void_generator_produces_only_air() {
        let states = states();
        let chunk = VoidGenerator.generate(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        for section in chunk.sections() {
            assert!(section.is_empty(&states));
        }
    }

    #[test]
    fn flat_generator_lays_the_expected_column() {
        let states = states();
        let limits = HeightLimits::OVERWORLD;
        let generator = FlatGenerator::default();
        let chunk = generator.generate(ChunkPos::new(0, 0), limits, &states);

        assert_eq!(chunk.block(0, -64, 0, &states), states.bedrock);
        assert_eq!(chunk.block(0, -63, 0, &states), states.stone);
        assert_eq!(chunk.block(0, -62, 0, &states), states.dirt);
        assert_eq!(chunk.block(0, -61, 0, &states), states.dirt);
        assert_eq!(chunk.block(0, -60, 0, &states), states.grass_block);
        assert_eq!(chunk.block(0, -59, 0, &states), states.air);
        assert_eq!(chunk.block(0, 100, 0, &states), states.air);
    }

    #[test]
    fn flat_generator_is_uniform_across_the_whole_chunk() {
        let states = states();
        let limits = HeightLimits::OVERWORLD;
        let chunk = FlatGenerator::default().generate(ChunkPos::new(3, -7), limits, &states);

        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                assert_eq!(chunk.block(x, -64, z, &states), states.bedrock, "{x},{z}");
                assert_eq!(
                    chunk.block(x, -60, z, &states),
                    states.grass_block,
                    "{x},{z}"
                );
                assert_eq!(chunk.block(x, -59, z, &states), states.air, "{x},{z}");
            }
        }
    }

    #[test]
    fn the_bulk_fill_path_agrees_with_a_per_block_sweep() {
        // The whole-section fast path must be indistinguishable from writing
        // every block individually.
        let states = states();
        let limits = HeightLimits::OVERWORLD;
        let generator = FlatGenerator::default();
        let fast = generator.generate(ChunkPos::new(0, 0), limits, &states);

        let mut slow = Chunk::empty(ChunkPos::new(0, 0), limits, &states);
        for y in limits.min_y..=limits.max_y() {
            let state = generator.block_at(y, limits, &states);
            if states.is_air(state) {
                continue;
            }
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    slow.set_block(x, y, z, state, &states);
                }
            }
        }
        slow.recompute_heightmaps(&states);

        for y in limits.min_y..=limits.max_y() {
            assert_eq!(
                fast.block(0, y, 0, &states),
                slow.block(0, y, 0, &states),
                "y={y}"
            );
        }
        assert_eq!(fast.heightmaps(), slow.heightmaps());
    }

    #[test]
    fn heightmaps_are_populated_by_generation() {
        let states = states();
        let limits = HeightLimits::OVERWORLD;
        let chunk = FlatGenerator::default().generate(ChunkPos::new(0, 0), limits, &states);
        // Surface at y=-60 is 4 above min_y, so the height is 5.
        assert_eq!(chunk.heightmaps().world_surface.get(0), 5);
    }

    #[test]
    fn a_flat_chunk_still_serializes_small() {
        let states = states();
        let chunk = FlatGenerator::default().generate(
            ChunkPos::new(0, 0),
            HeightLimits::OVERWORLD,
            &states,
        );
        let mut out = Vec::new();
        chunk.write_sections(&states, &mut out);
        // Only the bottom section carries real data; the rest stay single-valued.
        assert!(
            out.len() < 3_000,
            "flat chunk serialized to {} bytes",
            out.len()
        );
    }
}
