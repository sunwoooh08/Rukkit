//! Chunk columns and their 16³ sections.
//!
//! A column is a fixed stack of sections covering the world's height range,
//! plus heightmaps. Sections are always present rather than lazily allocated:
//! an empty section is a single-value paletted container, which already costs
//! nothing, and keeping the stack dense removes a bounds-and-null check from
//! every block access.

use rukkit_protocol::nbt::{NbtCompound, NbtTag};
use rukkit_protocol::writer::PacketWrite;
use rukkit_protocol::ChunkPos;

use crate::bitstorage::BitStorage;
use crate::block::BlockStates;
use crate::palette::{ContainerKind, PalettedContainer};

/// Blocks along each edge of a section.
pub const SECTION_SIZE: usize = 16;
/// Blocks in a section.
pub const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;
/// Columns in a chunk (16x16).
pub const COLUMNS: usize = SECTION_SIZE * SECTION_SIZE;

/// Vertical extent of a world, in blocks and sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeightLimits {
    pub min_y: i32,
    pub height: u32,
}

impl HeightLimits {
    /// The overworld's range since 1.18: -64 up to 319 inclusive.
    pub const OVERWORLD: Self = Self {
        min_y: -64,
        height: 384,
    };

    #[must_use]
    pub const fn section_count(self) -> usize {
        (self.height / 16) as usize
    }

    #[must_use]
    pub const fn max_y(self) -> i32 {
        self.min_y + self.height as i32 - 1
    }

    #[must_use]
    pub const fn contains(self, y: i32) -> bool {
        y >= self.min_y && y <= self.max_y()
    }

    /// Bits needed to store a heightmap value, which ranges over `height + 1`
    /// possibilities (one extra for "nothing in this column").
    #[must_use]
    pub fn heightmap_bits(self) -> u32 {
        crate::palette::bits_for(self.height as usize + 1)
    }
}

/// Index of a block within a section, in vanilla's Y-Z-X order.
#[inline]
#[must_use]
pub const fn section_index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

/// One 16³ slice of a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSection {
    block_states: PalettedContainer,
    biomes: PalettedContainer,
}

impl ChunkSection {
    #[must_use]
    pub fn filled(states: &BlockStates, block: u32, biome: u32) -> Self {
        let direct_bits = states.direct_bits();
        Self {
            block_states: PalettedContainer::filled(ContainerKind::Blocks, direct_bits, block),
            biomes: PalettedContainer::filled(ContainerKind::Biomes, 6, biome),
        }
    }

    #[inline]
    #[must_use]
    pub fn block(&self, x: usize, y: usize, z: usize) -> u32 {
        self.block_states.get(section_index(x, y, z))
    }

    #[inline]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, state: u32) -> u32 {
        self.block_states.set(section_index(x, y, z), state)
    }

    /// Replaces every block in the section with `state`.
    ///
    /// Collapses the container back to its single-value form, so filling a
    /// section is O(1) rather than 4096 writes — the difference between a
    /// generator that keeps up with player movement and one that does not.
    pub fn fill_blocks(&mut self, state: u32) {
        self.block_states.fill(state);
    }

    #[must_use]
    pub fn block_states(&self) -> &PalettedContainer {
        &self.block_states
    }

    #[must_use]
    pub fn biomes(&self) -> &PalettedContainer {
        &self.biomes
    }

    pub fn biomes_mut(&mut self) -> &mut PalettedContainer {
        &mut self.biomes
    }

    /// Number of non-air blocks, the value the wire format carries.
    #[must_use]
    pub fn non_air_count(&self, states: &BlockStates) -> u16 {
        self.block_states.count(|id| !states.is_air(id)) as u16
    }

    /// True when the section holds nothing but air, and so can be skipped.
    #[must_use]
    pub fn is_empty(&self, states: &BlockStates) -> bool {
        self.non_air_count(states) == 0
    }

    /// Recomputes the minimal representation for both containers.
    pub fn shrink(&mut self) {
        self.block_states.shrink();
        self.biomes.shrink();
    }

    /// Writes the section in the chunk-data wire format: non-air count, then
    /// the block and biome containers.
    pub fn write(&self, states: &BlockStates, out: &mut impl PacketWrite) {
        out.write_i16(self.non_air_count(states) as i16);
        self.block_states.write(out);
        self.biomes.write(out);
    }

    /// Bytes [`ChunkSection::write`] will produce.
    #[must_use]
    pub fn wire_len(&self) -> usize {
        2 + self.block_states.wire_len() + self.biomes.wire_len()
    }
}

/// The two heightmaps the client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heightmaps {
    /// Highest block that blocks motion, per column.
    pub motion_blocking: BitStorage,
    /// Highest non-air block, per column.
    pub world_surface: BitStorage,
}

impl Heightmaps {
    #[must_use]
    pub fn new(limits: HeightLimits) -> Self {
        let bits = limits.heightmap_bits();
        Self {
            motion_blocking: BitStorage::new(COLUMNS, bits),
            world_surface: BitStorage::new(COLUMNS, bits),
        }
    }

    /// Serializes to the NBT compound carried in chunk data.
    #[must_use]
    pub fn to_nbt(&self) -> NbtTag {
        let mut compound = NbtCompound::with_capacity(2);
        compound.insert(
            "MOTION_BLOCKING",
            NbtTag::LongArray(
                self.motion_blocking
                    .data()
                    .iter()
                    .map(|&c| c as i64)
                    .collect(),
            ),
        );
        compound.insert(
            "WORLD_SURFACE",
            NbtTag::LongArray(
                self.world_surface
                    .data()
                    .iter()
                    .map(|&c| c as i64)
                    .collect(),
            ),
        );
        NbtTag::Compound(compound)
    }
}

/// A chunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pos: ChunkPos,
    limits: HeightLimits,
    sections: Vec<ChunkSection>,
    heightmaps: Heightmaps,
}

impl Chunk {
    /// An entirely air-filled column.
    #[must_use]
    pub fn empty(pos: ChunkPos, limits: HeightLimits, states: &BlockStates) -> Self {
        Self {
            pos,
            limits,
            sections: (0..limits.section_count())
                .map(|_| ChunkSection::filled(states, states.air, 0))
                .collect(),
            heightmaps: Heightmaps::new(limits),
        }
    }

    #[must_use]
    pub const fn pos(&self) -> ChunkPos {
        self.pos
    }

    #[must_use]
    pub const fn limits(&self) -> HeightLimits {
        self.limits
    }

    #[must_use]
    pub fn sections(&self) -> &[ChunkSection] {
        &self.sections
    }

    #[must_use]
    pub fn heightmaps(&self) -> &Heightmaps {
        &self.heightmaps
    }

    /// Section holding absolute height `y`, if within the world.
    #[inline]
    #[must_use]
    fn section_of(&self, y: i32) -> Option<usize> {
        if !self.limits.contains(y) {
            return None;
        }
        Some(((y - self.limits.min_y) >> 4) as usize)
    }

    /// Reads a block by chunk-local X/Z and absolute Y.
    ///
    /// Out-of-range heights read as air, matching how the game treats the void
    /// and the space above build height.
    #[must_use]
    pub fn block(&self, x: usize, y: i32, z: usize, states: &BlockStates) -> u32 {
        match self.section_of(y) {
            Some(index) => {
                let local_y = (y - self.limits.min_y) as usize & 0xF;
                self.sections[index].block(x, local_y, z)
            }
            None => states.air,
        }
    }

    /// Writes a block, returning the previous state. Out-of-range writes are
    /// ignored and report air.
    pub fn set_block(
        &mut self,
        x: usize,
        y: i32,
        z: usize,
        state: u32,
        states: &BlockStates,
    ) -> u32 {
        match self.section_of(y) {
            Some(index) => {
                let local_y = (y - self.limits.min_y) as usize & 0xF;
                self.sections[index].set_block(x, local_y, z, state)
            }
            None => states.air,
        }
    }

    pub fn section_mut(&mut self, index: usize) -> Option<&mut ChunkSection> {
        self.sections.get_mut(index)
    }

    /// Recomputes both heightmaps from the current block data.
    ///
    /// Scans each column downward and stops at the first non-air block, so a
    /// mostly empty world costs far less than a full sweep.
    pub fn recompute_heightmaps(&mut self, states: &BlockStates) {
        let bits_max = self.limits.height;
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let mut height = 0u32;
                for y in (self.limits.min_y..=self.limits.max_y()).rev() {
                    let index = ((y - self.limits.min_y) >> 4) as usize;
                    let local_y = (y - self.limits.min_y) as usize & 0xF;
                    if !states.is_air(self.sections[index].block(x, local_y, z)) {
                        // Heightmaps store "one above the highest block", so a
                        // block at min_y reports 1 and an empty column reports 0.
                        height = (y - self.limits.min_y + 1) as u32;
                        break;
                    }
                }
                debug_assert!(height <= bits_max);
                let column = z * SECTION_SIZE + x;
                self.heightmaps.motion_blocking.set(column, height);
                self.heightmaps.world_surface.set(column, height);
            }
        }
    }

    /// Recomputes the minimal representation for every section.
    pub fn shrink(&mut self) {
        for section in &mut self.sections {
            section.shrink();
        }
    }

    /// Writes the section array carried in chunk data.
    ///
    /// This is the part of the chunk packet whose layout has been stable across
    /// releases; the surrounding envelope (heightmap encoding, light arrays)
    /// is assembled by the server, which owns the version-specific details.
    pub fn write_sections(&self, states: &BlockStates, out: &mut Vec<u8>) {
        for section in &self.sections {
            section.write(states, out);
        }
    }

    /// Bytes [`Chunk::write_sections`] will produce.
    #[must_use]
    pub fn sections_wire_len(&self) -> usize {
        self.sections.iter().map(ChunkSection::wire_len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states() -> BlockStates {
        BlockStates::placeholder()
    }

    /// Heightmap index for a chunk-local column.
    fn column(x: usize, z: usize) -> usize {
        z * SECTION_SIZE + x
    }

    #[test]
    fn section_index_uses_vanilla_y_z_x_order() {
        assert_eq!(section_index(0, 0, 0), 0);
        assert_eq!(section_index(1, 0, 0), 1);
        assert_eq!(section_index(0, 0, 1), 16);
        assert_eq!(section_index(0, 1, 0), 256);
        assert_eq!(section_index(15, 15, 15), 4095);
    }

    #[test]
    fn section_indices_are_a_bijection() {
        let mut seen = vec![false; SECTION_VOLUME];
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let index = section_index(x, y, z);
                    assert!(!seen[index], "duplicate index for {x},{y},{z}");
                    seen[index] = true;
                }
            }
        }
        assert!(seen.into_iter().all(|hit| hit));
    }

    #[test]
    fn overworld_limits_match_the_modern_world_height() {
        let limits = HeightLimits::OVERWORLD;
        assert_eq!(limits.min_y, -64);
        assert_eq!(limits.max_y(), 319);
        assert_eq!(limits.section_count(), 24);
        assert!(limits.contains(-64));
        assert!(limits.contains(319));
        assert!(!limits.contains(-65));
        assert!(!limits.contains(320));
        // 385 possible values needs 9 bits.
        assert_eq!(limits.heightmap_bits(), 9);
    }

    #[test]
    fn a_fresh_chunk_is_entirely_air() {
        let states = states();
        let chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        assert_eq!(chunk.sections().len(), 24);
        for section in chunk.sections() {
            assert!(section.is_empty(&states));
            assert_eq!(section.block_states().bits_per_entry(), 0);
        }
        assert_eq!(chunk.block(0, 0, 0, &states), states.air);
    }

    #[test]
    fn blocks_round_trip_across_section_boundaries() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(1, -2), HeightLimits::OVERWORLD, &states);

        // Deliberately straddle section boundaries and the negative Y range.
        let probes = [-64, -49, -48, -1, 0, 15, 16, 63, 318, 319];
        for (i, y) in probes.iter().enumerate() {
            let state = states.stone + i as u32;
            assert_eq!(chunk.set_block(3, *y, 9, state, &states), states.air);
            assert_eq!(chunk.block(3, *y, 9, &states), state, "y={y}");
        }
        // Neighbouring columns are untouched.
        assert_eq!(chunk.block(4, 0, 9, &states), states.air);
    }

    #[test]
    fn out_of_range_heights_read_and_write_as_air() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        assert_eq!(chunk.block(0, -65, 0, &states), states.air);
        assert_eq!(chunk.block(0, 320, 0, &states), states.air);
        assert_eq!(
            chunk.set_block(0, 1000, 0, states.stone, &states),
            states.air
        );
        assert_eq!(chunk.block(0, 1000, 0, &states), states.air);
    }

    #[test]
    fn non_air_count_tracks_writes() {
        let states = states();
        let mut section = ChunkSection::filled(&states, states.air, 0);
        assert_eq!(section.non_air_count(&states), 0);

        section.set_block(0, 0, 0, states.stone);
        assert_eq!(section.non_air_count(&states), 1);

        for x in 0..16 {
            for z in 0..16 {
                section.set_block(x, 0, z, states.stone);
            }
        }
        assert_eq!(section.non_air_count(&states), 256);

        // Overwriting back to air decrements again.
        section.set_block(0, 0, 0, states.air);
        assert_eq!(section.non_air_count(&states), 255);
    }

    #[test]
    fn a_solid_section_counts_every_block() {
        let states = states();
        let section = ChunkSection::filled(&states, states.stone, 0);
        assert_eq!(section.non_air_count(&states), SECTION_VOLUME as u16);
        assert!(!section.is_empty(&states));
    }

    #[test]
    fn heightmaps_report_one_above_the_highest_block() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        chunk.set_block(0, 0, 0, states.stone, &states);
        chunk.set_block(1, 100, 1, states.stone, &states);
        chunk.set_block(2, -64, 2, states.stone, &states);
        chunk.recompute_heightmaps(&states);

        let maps = chunk.heightmaps();
        // y=0 is 64 above min_y, so the recorded height is 65.
        assert_eq!(maps.world_surface.get(0), 65);
        // y=100 -> 100 + 64 + 1
        assert_eq!(maps.world_surface.get(column(1, 1)), 165);
        // A block at the very bottom reports 1.
        assert_eq!(maps.world_surface.get(column(2, 2)), 1);
        // An untouched column reports 0.
        assert_eq!(maps.world_surface.get(200), 0);
    }

    #[test]
    fn heightmaps_take_the_highest_block_not_the_first_found() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        for y in [-10, 40, 200] {
            chunk.set_block(5, y, 5, states.stone, &states);
        }
        chunk.recompute_heightmaps(&states);
        assert_eq!(chunk.heightmaps().world_surface.get(column(5, 5)), 265);
    }

    #[test]
    fn heightmap_nbt_has_both_long_arrays() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        chunk.recompute_heightmaps(&states);

        let NbtTag::Compound(compound) = chunk.heightmaps().to_nbt() else {
            panic!("expected a compound");
        };
        for key in ["MOTION_BLOCKING", "WORLD_SURFACE"] {
            match compound.get(key) {
                // 256 columns at 9 bits, 7 per long, is 37 longs.
                Some(NbtTag::LongArray(values)) => assert_eq!(values.len(), 37, "{key}"),
                other => panic!("{key}: expected a long array, got {other:?}"),
            }
        }
    }

    #[test]
    fn section_wire_length_matches_what_is_written() {
        let states = states();
        let mut section = ChunkSection::filled(&states, states.air, 0);
        let mut out = Vec::new();

        section.write(&states, &mut out);
        assert_eq!(out.len(), section.wire_len(), "empty section");

        section.set_block(0, 0, 0, states.stone);
        out.clear();
        section.write(&states, &mut out);
        assert_eq!(out.len(), section.wire_len(), "indirect section");
    }

    #[test]
    fn chunk_wire_length_matches_what_is_written() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        for y in -64..0 {
            for x in 0..16 {
                for z in 0..16 {
                    chunk.set_block(x, y, z, states.stone, &states);
                }
            }
        }
        let mut out = Vec::new();
        chunk.write_sections(&states, &mut out);
        assert_eq!(out.len(), chunk.sections_wire_len());
    }

    #[test]
    fn an_empty_chunk_serializes_compactly() {
        let states = states();
        let chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        let mut out = Vec::new();
        chunk.write_sections(&states, &mut out);
        // 24 sections at 2 bytes of count plus two 3-byte single-value
        // containers each.
        assert_eq!(out.len(), 24 * (2 + 3 + 3));
    }

    #[test]
    fn shrink_does_not_change_block_contents() {
        let states = states();
        let mut chunk = Chunk::empty(ChunkPos::new(0, 0), HeightLimits::OVERWORLD, &states);
        for i in 0..300u32 {
            chunk.set_block(
                (i % 16) as usize,
                i as i32 % 300,
                0,
                states.stone + i,
                &states,
            );
        }
        let before: Vec<u32> = (0..300)
            .map(|i: i32| chunk.block((i % 16) as usize, i % 300, 0, &states))
            .collect();

        chunk.shrink();

        let after: Vec<u32> = (0..300)
            .map(|i: i32| chunk.block((i % 16) as usize, i % 300, 0, &states))
            .collect();
        assert_eq!(before, after);
    }
}
