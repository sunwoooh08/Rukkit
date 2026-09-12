//! Packed bit storage, laid out exactly as vanilla's `SimpleBitStorage`.
//!
//! Entries of `bits` width are packed into `u64` cells, and — importantly — an
//! entry never straddles two cells. That wastes `64 % bits` bits per cell but
//! makes every access a single load, shift and mask, which is the whole point:
//! a chunk section is read far more often than it is written.
//!
//! # Why not just divide
//!
//! Locating an entry means dividing its index by `values_per_long`, which is
//! not a power of two for most widths (5 bits gives 12 per long, 6 gives 10).
//! A hardware divide is 20-40 cycles and sits in the innermost loop of every
//! chunk serialization, so the divisor is turned into a multiply-and-shift at
//! construction time, the same trick vanilla uses with its magic-number table.

/// Widths above this cannot be represented.
pub const MAX_BITS: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitStorage {
    data: Vec<u64>,
    len: usize,
    bits: u32,
    values_per_long: u32,
    /// Reciprocal of `values_per_long`, scaled by 2^32.
    magic: u64,
    mask: u64,
}

/// Cells needed to hold `len` entries of `bits` width.
#[must_use]
pub fn cells_required(len: usize, bits: u32) -> usize {
    if bits == 0 {
        return 0;
    }
    let values_per_long = (64 / bits) as usize;
    len.div_ceil(values_per_long)
}

impl BitStorage {
    /// Creates zero-filled storage for `len` entries of `bits` width.
    ///
    /// # Panics
    ///
    /// If `bits` is zero or above [`MAX_BITS`], or if `len` exceeds the range
    /// over which the reciprocal division stays exact (2^26 entries, far above
    /// any chunk container).
    #[must_use]
    pub fn new(len: usize, bits: u32) -> Self {
        assert!(
            bits > 0 && bits <= MAX_BITS,
            "bit width {bits} out of range"
        );
        assert!(len < 1 << 26, "container of {len} entries is too large");

        let values_per_long = 64 / bits;
        Self {
            data: vec![0; cells_required(len, bits)],
            len,
            bits,
            values_per_long,
            magic: reciprocal(values_per_long),
            mask: (1u64 << bits) - 1,
        }
    }

    /// Wraps existing cells, as read from disk or the network.
    ///
    /// Returns `None` if `data` is not exactly the right length, which would
    /// otherwise show up later as silently truncated terrain.
    #[must_use]
    pub fn from_data(len: usize, bits: u32, data: Vec<u64>) -> Option<Self> {
        if bits == 0 || bits > MAX_BITS || len >= 1 << 26 {
            return None;
        }
        if data.len() != cells_required(len, bits) {
            return None;
        }
        let values_per_long = 64 / bits;
        Some(Self {
            data,
            len,
            bits,
            values_per_long,
            magic: reciprocal(values_per_long),
            mask: (1u64 << bits) - 1,
        })
    }

    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    #[must_use]
    pub const fn bits(&self) -> u32 {
        self.bits
    }

    /// The raw cells, for serialization.
    #[inline]
    #[must_use]
    pub fn data(&self) -> &[u64] {
        &self.data
    }

    /// Index of the cell holding entry `index`, via reciprocal multiplication.
    #[inline]
    const fn cell_of(&self, index: usize) -> usize {
        ((index as u64 * self.magic) >> 32) as usize
    }

    /// Reads entry `index`.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> u32 {
        assert!(index < self.len, "index {index} out of bounds");
        let cell = self.cell_of(index);
        let offset = (index - cell * self.values_per_long as usize) as u32 * self.bits;
        ((self.data[cell] >> offset) & self.mask) as u32
    }

    /// Writes entry `index`, returning the previous value.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds or `value` does not fit in `bits`.
    #[inline]
    pub fn set(&mut self, index: usize, value: u32) -> u32 {
        assert!(index < self.len, "index {index} out of bounds");
        let value = u64::from(value);
        assert!(
            value <= self.mask,
            "value {value} does not fit in {} bits",
            self.bits
        );

        let cell = self.cell_of(index);
        let offset = (index - cell * self.values_per_long as usize) as u32 * self.bits;
        let word = self.data[cell];
        let previous = (word >> offset) & self.mask;
        self.data[cell] = (word & !(self.mask << offset)) | (value << offset);
        previous as u32
    }

    /// Calls `f` for every entry in index order.
    ///
    /// Decodes a whole cell at a time rather than recomputing the cell index per
    /// entry, which is what makes bulk operations — counting non-air blocks,
    /// rewriting a palette — run at memory speed.
    pub fn for_each(&self, mut f: impl FnMut(usize, u32)) {
        let mut index = 0;
        for &cell in &self.data {
            let mut word = cell;
            for _ in 0..self.values_per_long {
                if index >= self.len {
                    return;
                }
                f(index, (word & self.mask) as u32);
                word >>= self.bits;
                index += 1;
            }
        }
    }

    /// Rebuilds at a new bit width, mapping each entry through `f`.
    ///
    /// Used when a palette grows past its current width, and when converting
    /// between indirect and direct representations.
    #[must_use]
    pub fn remap(&self, new_bits: u32, mut f: impl FnMut(u32) -> u32) -> Self {
        let mut out = Self::new(self.len, new_bits);
        self.for_each(|index, value| {
            out.set(index, f(value));
        });
        out
    }
}

/// `ceil(2^32 / divisor)`, the multiplier for reciprocal division.
///
/// For `index * error < 2^32` the result of `(index * magic) >> 32` equals
/// `index / divisor` exactly. With `divisor <= 64` the error term is under 64,
/// so the identity holds for every index below 2^26 — checked in the
/// constructor.
#[inline]
const fn reciprocal(divisor: u32) -> u64 {
    let d = divisor as u64;
    (1u64 << 32).div_ceil(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reciprocal_division_matches_real_division() {
        // The optimization the whole structure rests on.
        for bits in 1..=MAX_BITS {
            let vpl = 64 / bits;
            let magic = reciprocal(vpl);
            for index in 0..100_000usize {
                let fast = ((index as u64 * magic) >> 32) as usize;
                assert_eq!(
                    fast,
                    index / vpl as usize,
                    "bits={bits} vpl={vpl} index={index}"
                );
            }
        }
    }

    #[test]
    fn round_trips_every_value_at_every_width() {
        for bits in 1..=16u32 {
            let max = (1u64 << bits) - 1;
            let mut storage = BitStorage::new(64, bits);
            for i in 0..64usize {
                storage.set(i, (i as u64 % (max + 1)) as u32);
            }
            for i in 0..64usize {
                assert_eq!(
                    storage.get(i),
                    (i as u64 % (max + 1)) as u32,
                    "bits={bits} index={i}"
                );
            }
        }
    }

    #[test]
    fn set_returns_the_previous_value() {
        let mut storage = BitStorage::new(16, 5);
        assert_eq!(storage.set(3, 17), 0);
        assert_eq!(storage.set(3, 4), 17);
        assert_eq!(storage.get(3), 4);
    }

    #[test]
    fn entries_never_straddle_two_cells() {
        // 5 bits gives 12 values per long with 4 bits left unused; writing the
        // last value in a cell must not disturb the next cell.
        let mut storage = BitStorage::new(24, 5);
        storage.set(11, 31);
        storage.set(12, 1);
        assert_eq!(storage.get(11), 31);
        assert_eq!(storage.get(12), 1);
        assert_eq!(storage.data().len(), 2);
        // The top 4 bits of the first cell stay clear.
        assert_eq!(storage.data()[0] >> 60, 0);
    }

    #[test]
    fn cell_count_matches_vanilla_layout() {
        // 4096 blocks at 4 bits: 16 per long, 256 longs.
        assert_eq!(cells_required(4096, 4), 256);
        // 4096 at 5 bits: 12 per long, 342 longs (the last partly used).
        assert_eq!(cells_required(4096, 5), 342);
        // 4096 at 15 bits: 4 per long, 1024 longs.
        assert_eq!(cells_required(4096, 15), 1024);
        // 64 biomes at 3 bits: 21 per long, 4 longs.
        assert_eq!(cells_required(64, 3), 4);
    }

    #[test]
    fn writes_do_not_bleed_into_neighbours() {
        let mut storage = BitStorage::new(4096, 6);
        for i in 0..4096 {
            storage.set(i, 63);
        }
        // Clear one entry and confirm only it changed.
        storage.set(1000, 0);
        for i in 0..4096 {
            let expected = if i == 1000 { 0 } else { 63 };
            assert_eq!(storage.get(i), expected, "index {i}");
        }
    }

    #[test]
    fn for_each_visits_every_entry_in_order() {
        let mut storage = BitStorage::new(100, 7);
        for i in 0..100 {
            storage.set(i, (i % 128) as u32);
        }
        let mut seen = Vec::new();
        storage.for_each(|index, value| seen.push((index, value)));
        assert_eq!(seen.len(), 100);
        for (i, (index, value)) in seen.into_iter().enumerate() {
            assert_eq!(index, i);
            assert_eq!(value, (i % 128) as u32);
        }
    }

    #[test]
    fn for_each_agrees_with_get() {
        for bits in [1u32, 4, 5, 6, 9, 15, 32] {
            let len = 300;
            let mut storage = BitStorage::new(len, bits);
            let max = ((1u64 << bits) - 1) as u32;
            for i in 0..len {
                storage.set(i, (i as u32).min(max));
            }
            storage.for_each(|index, value| {
                assert_eq!(value, storage.get(index), "bits={bits} index={index}");
            });
        }
    }

    #[test]
    fn remap_preserves_logical_contents() {
        let mut storage = BitStorage::new(4096, 4);
        for i in 0..4096 {
            storage.set(i, (i % 16) as u32);
        }
        let wider = storage.remap(9, |v| v * 2);
        assert_eq!(wider.bits(), 9);
        assert_eq!(wider.len(), 4096);
        for i in 0..4096 {
            assert_eq!(wider.get(i), (i % 16) as u32 * 2, "index {i}");
        }
    }

    #[test]
    fn from_data_rejects_a_wrong_sized_array() {
        assert!(BitStorage::from_data(4096, 4, vec![0; 256]).is_some());
        assert!(BitStorage::from_data(4096, 4, vec![0; 255]).is_none());
        assert!(BitStorage::from_data(4096, 4, vec![0; 257]).is_none());
        assert!(BitStorage::from_data(4096, 0, vec![]).is_none());
    }

    #[test]
    fn from_data_preserves_the_bit_pattern() {
        let mut original = BitStorage::new(64, 6);
        for i in 0..64 {
            original.set(i, (i % 64) as u32);
        }
        let restored = BitStorage::from_data(64, 6, original.data().to_vec()).expect("valid data");
        assert_eq!(restored, original);
    }

    #[test]
    #[should_panic(expected = "does not fit")]
    fn value_wider_than_the_field_panics() {
        let mut storage = BitStorage::new(16, 4);
        storage.set(0, 16);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn out_of_bounds_read_panics() {
        let _ = BitStorage::new(16, 4).get(16);
    }
}
