//! Paletted containers, the representation that makes chunks small.
//!
//! Most 16³ sections contain a handful of distinct blocks, so storing a full
//! registry id per block wastes an order of magnitude of memory and bandwidth.
//! Instead a container adapts between three representations:
//!
//! * **Single** — the whole section is one block. Costs no per-block storage at
//!   all, and covers the overwhelming majority of sections in a real world
//!   (solid stone, or pure air above the surface).
//! * **Indirect** — a small palette of ids, with each entry storing an index
//!   into it at the narrowest width that fits.
//! * **Direct** — registry ids stored inline, once a section is too varied for
//!   a palette to pay for itself.
//!
//! Promotion happens automatically on write and never reverses on its own;
//! [`PalettedContainer::shrink`] recomputes the minimal representation when it
//! is worth paying for, such as before saving or sending.

use rukkit_protocol::writer::PacketWrite;

use crate::bitstorage::BitStorage;

/// What a container holds, which fixes its size and palette thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// 16x16x16 block states.
    Blocks,
    /// 4x4x4 biome cells within a section.
    Biomes,
}

impl ContainerKind {
    #[must_use]
    pub const fn volume(self) -> usize {
        match self {
            Self::Blocks => 16 * 16 * 16,
            Self::Biomes => 4 * 4 * 4,
        }
    }

    /// Narrowest indirect width. Vanilla never uses fewer than 4 bits for
    /// blocks, even when 1 would do.
    #[must_use]
    pub const fn min_indirect_bits(self) -> u32 {
        match self {
            Self::Blocks => 4,
            Self::Biomes => 1,
        }
    }

    /// Widest indirect width before switching to direct storage.
    #[must_use]
    pub const fn max_indirect_bits(self) -> u32 {
        match self {
            Self::Blocks => 8,
            Self::Biomes => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Repr {
    Single(u32),
    Indirect {
        palette: Vec<u32>,
        storage: BitStorage,
    },
    Direct(BitStorage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedContainer {
    kind: ContainerKind,
    /// Width used by the direct representation, derived from the registry size.
    direct_bits: u32,
    repr: Repr,
}

impl PalettedContainer {
    /// A container entirely filled with `value`.
    #[must_use]
    pub fn filled(kind: ContainerKind, direct_bits: u32, value: u32) -> Self {
        Self {
            kind,
            direct_bits,
            repr: Repr::Single(value),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ContainerKind {
        self.kind
    }

    /// Number of entries, fixed by the container kind.
    #[must_use]
    pub const fn volume(&self) -> usize {
        self.kind.volume()
    }

    /// Bits currently used per entry; `0` for a single-value container.
    #[must_use]
    pub fn bits_per_entry(&self) -> u32 {
        match &self.repr {
            Repr::Single(_) => 0,
            Repr::Indirect { storage, .. } | Repr::Direct(storage) => storage.bits(),
        }
    }

    /// Number of distinct ids the container can currently name.
    #[must_use]
    pub fn palette_len(&self) -> usize {
        match &self.repr {
            Repr::Single(_) => 1,
            Repr::Indirect { palette, .. } => palette.len(),
            Repr::Direct(_) => 1 << self.direct_bits,
        }
    }

    /// Reads the id at `index`.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> u32 {
        debug_assert!(index < self.volume());
        match &self.repr {
            Repr::Single(value) => *value,
            Repr::Indirect { palette, storage } => palette[storage.get(index) as usize],
            Repr::Direct(storage) => storage.get(index),
        }
    }

    /// Writes `value` at `index`, returning the id previously there.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds.
    pub fn set(&mut self, index: usize, value: u32) -> u32 {
        assert!(index < self.volume(), "index {index} out of bounds");

        // Fast paths: everything that leaves the representation unchanged. This
        // is the overwhelmingly common case, so it stays free of any move or
        // reallocation.
        match &mut self.repr {
            Repr::Single(existing) if *existing == value => return *existing,
            Repr::Direct(storage) => return storage.set(index, value),
            Repr::Indirect { palette, storage } => {
                if let Some(slot) = palette.iter().position(|&id| id == value) {
                    let previous = storage.set(index, slot as u32);
                    return palette[previous as usize];
                }
                if palette.len() < 1usize << storage.bits() {
                    palette.push(value);
                    let previous = storage.set(index, (palette.len() - 1) as u32);
                    return palette[previous as usize];
                }
                // Palette is full; fall through to widen or convert.
            }
            Repr::Single(_) => {}
        }

        // Slow paths: the representation has to change, so take ownership of it.
        match std::mem::replace(&mut self.repr, Repr::Single(value)) {
            Repr::Single(previous) => {
                // Second distinct value: go indirect with the old value at slot
                // 0, so the zero-filled storage is already correct everywhere
                // except the entry being written.
                let mut storage =
                    BitStorage::new(self.kind.volume(), self.kind.min_indirect_bits());
                storage.set(index, 1);
                self.repr = Repr::Indirect {
                    palette: vec![previous, value],
                    storage,
                };
                previous
            }
            Repr::Indirect {
                mut palette,
                storage,
            } => {
                let wider = storage.bits() + 1;
                if wider <= self.kind.max_indirect_bits() {
                    let mut grown = storage.remap(wider, |slot| slot);
                    palette.push(value);
                    let previous_slot = grown.set(index, (palette.len() - 1) as u32);
                    let previous = palette[previous_slot as usize];
                    self.repr = Repr::Indirect {
                        palette,
                        storage: grown,
                    };
                    previous
                } else {
                    // Too varied for a palette to pay for itself: store ids
                    // inline from here on.
                    let mut direct = storage.remap(self.direct_bits, |slot| palette[slot as usize]);
                    let previous = direct.set(index, value);
                    self.repr = Repr::Direct(direct);
                    previous
                }
            }
            Repr::Direct(_) => unreachable!("direct storage never needs conversion"),
        }
    }

    /// Resets every entry to `value`, dropping any allocation.
    pub fn fill(&mut self, value: u32) {
        self.repr = Repr::Single(value);
    }

    /// Visits every entry in index order.
    pub fn for_each(&self, mut f: impl FnMut(usize, u32)) {
        match &self.repr {
            Repr::Single(value) => {
                for index in 0..self.volume() {
                    f(index, *value);
                }
            }
            Repr::Indirect { palette, storage } => {
                storage.for_each(|index, slot| f(index, palette[slot as usize]));
            }
            Repr::Direct(storage) => storage.for_each(f),
        }
    }

    /// Counts entries for which `predicate` holds.
    ///
    /// For a single-value container this is one predicate call rather than
    /// 4096, which is why chunk block counts are effectively free for the
    /// uniform sections that dominate a world.
    #[must_use]
    pub fn count(&self, mut predicate: impl FnMut(u32) -> bool) -> usize {
        match &self.repr {
            Repr::Single(value) => {
                if predicate(*value) {
                    self.volume()
                } else {
                    0
                }
            }
            Repr::Indirect { palette, storage } => {
                // Evaluate the predicate once per palette entry, not per block.
                let matching: Vec<bool> = palette.iter().map(|&id| predicate(id)).collect();
                let mut total = 0;
                storage.for_each(|_, slot| {
                    if matching[slot as usize] {
                        total += 1;
                    }
                });
                total
            }
            Repr::Direct(storage) => {
                let mut total = 0;
                storage.for_each(|_, id| {
                    if predicate(id) {
                        total += 1;
                    }
                });
                total
            }
        }
    }

    /// Rebuilds at the narrowest representation that fits the current contents.
    ///
    /// Writes only ever widen a container, so a section that was briefly varied
    /// keeps paying for that forever without this. Worth calling before saving
    /// or sending, not on every block change.
    pub fn shrink(&mut self) {
        if matches!(self.repr, Repr::Single(_)) {
            return;
        }

        let mut distinct: Vec<u32> = Vec::new();
        self.for_each(|_, id| {
            if !distinct.contains(&id) {
                distinct.push(id);
            }
        });

        if distinct.len() == 1 {
            self.repr = Repr::Single(distinct[0]);
            return;
        }

        let needed = bits_for(distinct.len());
        let bits = needed.max(self.kind.min_indirect_bits());
        if bits > self.kind.max_indirect_bits() {
            return; // already direct, or too varied to palette
        }

        let mut storage = BitStorage::new(self.kind.volume(), bits);
        self.for_each(|index, id| {
            let slot = distinct
                .iter()
                .position(|&c| c == id)
                .expect("collected above");
            storage.set(index, slot as u32);
        });
        self.repr = Repr::Indirect {
            palette: distinct,
            storage,
        };
    }

    /// Writes the container in the chunk-data wire format.
    pub fn write(&self, out: &mut impl PacketWrite) {
        match &self.repr {
            Repr::Single(value) => {
                out.write_u8(0);
                out.write_varint(*value as i32);
                // Empty data array.
                out.write_varint(0);
            }
            Repr::Indirect { palette, storage } => {
                out.write_u8(storage.bits() as u8);
                out.write_varint(palette.len() as i32);
                for &id in palette {
                    out.write_varint(id as i32);
                }
                write_cells(storage, out);
            }
            Repr::Direct(storage) => {
                out.write_u8(storage.bits() as u8);
                write_cells(storage, out);
            }
        }
    }

    /// Size in bytes the container occupies on the wire.
    #[must_use]
    pub fn wire_len(&self) -> usize {
        use rukkit_protocol::varint::varint_len;
        match &self.repr {
            Repr::Single(value) => 1 + varint_len(*value as i32) + 1,
            Repr::Indirect { palette, storage } => {
                1 + varint_len(palette.len() as i32)
                    + palette
                        .iter()
                        .map(|&id| varint_len(id as i32))
                        .sum::<usize>()
                    + varint_len(storage.data().len() as i32)
                    + storage.data().len() * 8
            }
            Repr::Direct(storage) => {
                1 + varint_len(storage.data().len() as i32) + storage.data().len() * 8
            }
        }
    }
}

fn write_cells(storage: &BitStorage, out: &mut impl PacketWrite) {
    out.write_varint(storage.data().len() as i32);
    for &cell in storage.data() {
        out.write_i64(cell as i64);
    }
}

/// Bits needed to index `count` distinct values.
#[must_use]
pub fn bits_for(count: usize) -> u32 {
    if count <= 1 {
        return 1;
    }
    usize::BITS - (count - 1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRECT_BITS: u32 = 15;
    const AIR: u32 = 0;
    const STONE: u32 = 1;

    fn blocks(value: u32) -> PalettedContainer {
        PalettedContainer::filled(ContainerKind::Blocks, DIRECT_BITS, value)
    }

    #[test]
    fn bits_for_counts_correctly() {
        assert_eq!(bits_for(0), 1);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 2);
        assert_eq!(bits_for(5), 3);
        assert_eq!(bits_for(256), 8);
        assert_eq!(bits_for(257), 9);
    }

    #[test]
    fn a_uniform_container_stores_nothing_per_entry() {
        let container = blocks(STONE);
        assert_eq!(container.bits_per_entry(), 0);
        for i in 0..container.volume() {
            assert_eq!(container.get(i), STONE);
        }
    }

    #[test]
    fn writing_the_same_value_does_not_promote() {
        let mut container = blocks(STONE);
        container.set(5, STONE);
        assert_eq!(
            container.bits_per_entry(),
            0,
            "should still be single-valued"
        );
    }

    #[test]
    fn second_value_promotes_to_indirect_at_the_minimum_width() {
        let mut container = blocks(AIR);
        assert_eq!(container.set(10, STONE), AIR);
        assert_eq!(container.bits_per_entry(), 4, "blocks start at 4 bits");
        assert_eq!(container.get(10), STONE);
        assert_eq!(container.get(11), AIR);
    }

    #[test]
    fn palette_widens_as_values_are_added() {
        let mut container = blocks(AIR);
        // 16 distinct ids fit in 4 bits; the 17th forces 5.
        for i in 0..16u32 {
            container.set(i as usize, i);
        }
        assert_eq!(container.bits_per_entry(), 4);
        container.set(16, 99);
        assert_eq!(container.bits_per_entry(), 5);

        for i in 0..16u32 {
            assert_eq!(container.get(i as usize), i, "index {i}");
        }
        assert_eq!(container.get(16), 99);
    }

    #[test]
    fn exceeding_the_palette_limit_switches_to_direct() {
        let mut container = blocks(AIR);
        // 257 distinct values cannot fit an 8-bit palette.
        for i in 0..257usize {
            container.set(i, i as u32);
        }
        assert_eq!(
            container.bits_per_entry(),
            DIRECT_BITS,
            "should have gone direct"
        );
        for i in 0..257usize {
            assert_eq!(container.get(i), i as u32, "index {i}");
        }
        // Untouched entries kept their original value through both conversions.
        assert_eq!(container.get(1000), AIR);
    }

    #[test]
    fn set_returns_the_previous_id_through_every_representation() {
        let mut container = blocks(AIR);
        assert_eq!(container.set(0, STONE), AIR);
        assert_eq!(container.set(0, 5), STONE);

        // Force a widen, then a direct conversion, checking the return each time.
        for i in 1..300usize {
            container.set(i, i as u32 + 100);
        }
        assert_eq!(container.set(0, 7), 5);
        assert_eq!(container.set(50, 8), 150);
    }

    #[test]
    fn biomes_use_their_own_thresholds() {
        let mut container = PalettedContainer::filled(ContainerKind::Biomes, 6, 0);
        assert_eq!(container.volume(), 64);
        container.set(0, 1);
        assert_eq!(container.bits_per_entry(), 1, "biomes start at 1 bit");
        for i in 0..9u32 {
            container.set(i as usize, i);
        }
        // Beyond 3 bits biomes go direct.
        assert_eq!(container.bits_per_entry(), 6);
    }

    #[test]
    fn fill_resets_to_single_value() {
        let mut container = blocks(AIR);
        for i in 0..100 {
            container.set(i, i as u32);
        }
        assert!(container.bits_per_entry() > 0);
        container.fill(STONE);
        assert_eq!(container.bits_per_entry(), 0);
        assert_eq!(container.get(4095), STONE);
    }

    #[test]
    fn count_matches_a_manual_sweep() {
        let mut container = blocks(AIR);
        for i in 0..1000usize {
            container.set(i, STONE);
        }
        assert_eq!(container.count(|id| id == STONE), 1000);
        assert_eq!(container.count(|id| id == AIR), 4096 - 1000);
        assert_eq!(container.count(|id| id != AIR), 1000);

        // Uniform containers take the fast path but must agree.
        assert_eq!(blocks(STONE).count(|id| id != AIR), 4096);
        assert_eq!(blocks(AIR).count(|id| id != AIR), 0);
    }

    #[test]
    fn for_each_agrees_with_get_in_every_representation() {
        let mut container = blocks(AIR);
        let check = |c: &PalettedContainer| {
            c.for_each(|index, id| assert_eq!(id, c.get(index), "index {index}"));
        };
        check(&container);

        container.set(1, STONE);
        check(&container);

        for i in 0..300usize {
            container.set(i, i as u32);
        }
        check(&container);
    }

    #[test]
    fn shrink_recovers_a_single_value_container() {
        let mut container = blocks(AIR);
        for i in 0..500usize {
            container.set(i, i as u32);
        }
        assert!(container.bits_per_entry() > 0);

        container.fill(AIR);
        container.set(0, STONE);
        container.set(0, AIR);
        // Now uniform again but still carrying a palette.
        container.shrink();
        assert_eq!(container.bits_per_entry(), 0);
        assert_eq!(container.get(2000), AIR);
    }

    #[test]
    fn shrink_narrows_an_oversized_palette() {
        let mut container = blocks(AIR);
        for i in 0..20u32 {
            container.set(i as usize, i);
        }
        assert_eq!(container.bits_per_entry(), 5);

        // Collapse back to two distinct values.
        for i in 0..20usize {
            container.set(i, if i % 2 == 0 { AIR } else { STONE });
        }
        container.shrink();
        assert_eq!(
            container.bits_per_entry(),
            4,
            "should narrow to the minimum"
        );
        for i in 0..20usize {
            let expected = if i % 2 == 0 { AIR } else { STONE };
            assert_eq!(container.get(i), expected, "index {i}");
        }
    }

    #[test]
    fn shrink_preserves_contents_exactly() {
        let mut container = blocks(AIR);
        let mut expected = vec![AIR; 4096];
        for i in 0..4096usize {
            let value = (i % 7) as u32;
            container.set(i, value);
            expected[i] = value;
        }
        container.shrink();
        for i in 0..4096 {
            assert_eq!(container.get(i), expected[i], "index {i}");
        }
    }

    #[test]
    fn wire_format_single_value_is_three_bytes() {
        let mut out = Vec::new();
        blocks(STONE).write(&mut out);
        // bits=0, value varint, empty data array length
        assert_eq!(out, vec![0, STONE as u8, 0]);
    }

    #[test]
    fn wire_len_matches_what_write_produces() {
        let mut container = blocks(AIR);
        let mut out = Vec::new();

        container.write(&mut out);
        assert_eq!(out.len(), container.wire_len(), "single");

        container.set(0, STONE);
        out.clear();
        container.write(&mut out);
        assert_eq!(out.len(), container.wire_len(), "indirect");

        for i in 0..300usize {
            container.set(i, i as u32);
        }
        out.clear();
        container.write(&mut out);
        assert_eq!(out.len(), container.wire_len(), "direct");
    }

    #[test]
    fn indirect_wire_format_has_palette_then_cells() {
        use rukkit_protocol::reader::PacketReader;

        let mut container = blocks(AIR);
        container.set(0, STONE);
        let mut out = Vec::new();
        container.write(&mut out);

        let mut r = PacketReader::new(&out);
        assert_eq!(r.read_u8().unwrap(), 4, "bits per entry");
        assert_eq!(r.read_varint().unwrap(), 2, "palette length");
        assert_eq!(r.read_varint().unwrap(), AIR as i32);
        assert_eq!(r.read_varint().unwrap(), STONE as i32);
        assert_eq!(r.read_varint().unwrap(), 256, "cell count for 4-bit 4096");
        assert!(r.is_empty() || r.remaining() == 256 * 8);
    }
}
