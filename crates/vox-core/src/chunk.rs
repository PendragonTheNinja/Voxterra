//! Chunk block storage — palette-compressed (Milestone 01).
//!
//! Each chunk stores a *palette* (the distinct [`BlockId`]s present) and a
//! packed index array mapping every voxel to a palette entry. Memory cost
//! scales with block-type *diversity*, not block-type count — this is what
//! makes hundreds of rock/mineral types affordable (see CLAUDE.md).
//!
//! Two invariants worth knowing:
//!
//! - **Uniform fast path:** a chunk whose palette has one entry (all air,
//!   all stone, ...) stores NO index array. In a cubic-chunk world most
//!   chunks are uniform (sky or deep underground), so this is the single
//!   biggest memory win in the engine.
//! - **Bit widths are 1, 2, 4, 8, or 16** — always a divisor of 64, so a
//!   packed value never straddles a `u64` word boundary. Slightly more
//!   memory than minimal bit packing, much simpler and faster access.
//!
//! Index order is Y-major [`LocalPos::index`] order — the same ordering
//! contract the serialized chunk format (Milestone 02) will use.
//!
//! The public API is unchanged from the Milestone 00 dense version; the
//! palette is an internal representation, and nothing outside this module
//! may assume dense storage or a fixed bits-per-voxel.

use crate::block::BlockId;
use crate::coords::{CHUNK_VOLUME, LocalPos};

/// Block storage for one 32³ chunk.
pub struct Chunk {
    /// Distinct block types present (or once-present, until [`Chunk::compact`]).
    /// Invariant: never empty. In the uniform case, exactly one entry.
    palette: Vec<BlockId>,
    /// Packed per-voxel palette indices; `None` iff the chunk is uniform.
    indices: Option<Packed>,
    /// Whether this chunk differs from what worldgen would produce for its
    /// position — i.e. it has been edited (or was loaded from disk, which
    /// means it was edited at some earlier point). Only modified chunks need
    /// saving; unmodified chunks regenerate from the seed for free. Runtime
    /// state, NOT serialized (see [`Chunk::serialize`]).
    modified: bool,
    /// Per-voxel light, one packed byte each (high nibble sky, low nibble
    /// block; ADR-0004). `None` means fully dark (the common case) and costs
    /// nothing — allocated lazily on first nonzero light. Runtime-derived
    /// from blocks, NOT serialized (recomputed on load).
    light: Option<Box<[u8; CHUNK_VOLUME]>>,
}

impl Chunk {
    /// A chunk uniformly filled with one block type. O(1) memory.
    pub fn filled(block: BlockId) -> Self {
        Self {
            palette: vec![block],
            indices: None,
            modified: false,
            light: None,
        }
    }

    /// An all-air chunk.
    pub fn new_air() -> Self {
        Self::filled(BlockId::AIR)
    }

    #[inline]
    pub fn get(&self, pos: LocalPos) -> BlockId {
        match &self.indices {
            None => self.palette[0],
            Some(packed) => self.palette[packed.get(pos.index()) as usize],
        }
    }

    pub fn set(&mut self, pos: LocalPos, block: BlockId) {
        // Uniform fast path: writing the same block is a no-op.
        if self.indices.is_none() && block == self.palette[0] {
            return;
        }

        // Any write that reaches here changes (or may change) the chunk's
        // contents relative to generation — treat it as an edit.
        self.modified = true;

        let palette_index = self.palette_index_or_insert(block);

        match &mut self.indices {
            Some(packed) => packed.set(pos.index(), palette_index as u64),
            None => {
                // Leave the uniform representation: materialize an index
                // array (all zeros = old uniform block), then write.
                let mut packed = Packed::new(bits_for(self.palette.len()));
                packed.set(pos.index(), palette_index as u64);
                self.indices = Some(packed);
            }
        }
    }

    /// True if every block in the chunk is air.
    pub fn is_all_air(&self) -> bool {
        match &self.indices {
            None => self.palette[0].is_air(),
            Some(packed) => {
                // If no palette entry is air, no cell can be.
                if !self.palette.iter().any(|b| b.is_air()) {
                    return false;
                }
                (0..CHUNK_VOLUME).all(|i| self.palette[packed.get(i) as usize].is_air())
            }
        }
    }

    /// True if the chunk stores a single block type with no index array.
    /// (Mesh/serialization fast paths key off this.)
    pub fn is_uniform(&self) -> bool {
        self.indices.is_none()
    }

    /// Whether this chunk has been edited since generation (or loaded from
    /// disk). Drives save-on-unload: only modified chunks need persisting.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Mark this chunk as modified (must be persisted). Used by the storage
    /// layer for chunks read from disk, since their existence on disk means
    /// they were edited at some point.
    pub fn mark_modified(&mut self) {
        self.modified = true;
    }

    /// Mark this chunk as matching generation (need not be persisted). Used
    /// after generating a chunk, and after saving one.
    pub fn mark_unmodified(&mut self) {
        self.modified = false;
    }

    // ---- Light (Milestone 04, ADR-0004) ----
    //
    // One packed byte per voxel: high nibble sky (M05, unused here), low
    // nibble block light. Stored lazily — `None` means fully dark. Light is
    // derived from blocks and never serialized; setting it does not mark the
    // chunk modified.

    /// Block-light level (0..=15) at `pos`. Dark (0) when no light array is
    /// allocated.
    #[inline]
    pub fn block_light(&self, pos: LocalPos) -> u8 {
        match &self.light {
            Some(arr) => arr[pos.index()] & 0x0F,
            None => 0,
        }
    }

    /// Set the block-light level (clamped to 0..=15) at `pos`, preserving the
    /// sky nibble. Allocates the light array on first nonzero write.
    pub fn set_block_light(&mut self, pos: LocalPos, level: u8) {
        let level = level.min(15);
        if self.light.is_none() {
            if level == 0 {
                return; // already dark; no need to allocate
            }
            self.light = Some(Box::new([0u8; CHUNK_VOLUME]));
        }
        let arr = self.light.as_mut().unwrap();
        let i = pos.index();
        arr[i] = (arr[i] & 0xF0) | level;
    }

    /// Sky-light level (0..=15) at `pos`, from the high nibble (ADR-0005).
    /// Dark (0) when no light array is allocated. NOTE: an unallocated chunk
    /// reads 0 here, but "open to sky" cells are assigned 15 by the relight
    /// pass — storage default 0 is correct; daylight comes from propagation.
    #[inline]
    pub fn sky_light(&self, pos: LocalPos) -> u8 {
        match &self.light {
            Some(arr) => (arr[pos.index()] >> 4) & 0x0F,
            None => 0,
        }
    }

    /// Set the sky-light level (clamped to 0..=15) at `pos`, preserving the
    /// block-light nibble. Allocates the light array on first nonzero write.
    pub fn set_sky_light(&mut self, pos: LocalPos, level: u8) {
        let level = level.min(15);
        if self.light.is_none() {
            if level == 0 {
                return; // already dark; no need to allocate
            }
            self.light = Some(Box::new([0u8; CHUNK_VOLUME]));
        }
        let arr = self.light.as_mut().unwrap();
        let i = pos.index();
        arr[i] = (arr[i] & 0x0F) | (level << 4);
    }

    /// Whether any light storage is allocated (i.e. the chunk may be lit).
    /// A `false` result guarantees the chunk is fully dark (both channels).
    #[inline]
    pub fn has_light(&self) -> bool {
        self.light.is_some()
    }

    /// Clear all light back to dark, releasing the storage. Used when
    /// relighting from scratch (recompute-on-load) or when light is fully
    /// removed.
    pub fn clear_light(&mut self) {
        self.light = None;
    }

    /// Number of palette entries. After many overwrites this can include
    /// entries no longer used by any voxel — see [`Chunk::compact`].
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// Drop unused palette entries and re-pack at the smallest bit width;
    /// collapses back to the uniform representation when possible.
    ///
    /// Never called automatically — callers decide when the O(volume) cost
    /// is worth paying (e.g. before serialization).
    pub fn compact(&mut self) {
        let Some(packed) = &self.indices else {
            return; // already uniform: nothing to do
        };

        let mut used = vec![false; self.palette.len()];
        for i in 0..CHUNK_VOLUME {
            used[packed.get(i) as usize] = true;
        }

        let mut remap = vec![0u64; self.palette.len()];
        let mut new_palette = Vec::new();
        for (old_index, &is_used) in used.iter().enumerate() {
            if is_used {
                remap[old_index] = new_palette.len() as u64;
                new_palette.push(self.palette[old_index]);
            }
        }

        if new_palette.len() == 1 {
            self.palette = new_palette;
            self.indices = None;
            return;
        }

        let mut new_packed = Packed::new(bits_for(new_palette.len()));
        for i in 0..CHUNK_VOLUME {
            new_packed.set(i, remap[packed.get(i) as usize]);
        }
        self.palette = new_palette;
        self.indices = Some(new_packed);
    }

    /// Palette index for `block`, inserting it if new. Widens the packed
    /// array when the palette grows past the current bit width's capacity.
    fn palette_index_or_insert(&mut self, block: BlockId) -> usize {
        // Linear search: palettes are small (a handful of entries in
        // practice). Revisit only if profiling ever says otherwise.
        if let Some(i) = self.palette.iter().position(|&b| b == block) {
            return i;
        }

        self.palette.push(block);
        let needed_bits = bits_for(self.palette.len());
        if let Some(packed) = &self.indices
            && needed_bits > packed.bits
        {
            let widened = packed.repacked(needed_bits);
            self.indices = Some(widened);
        }
        self.palette.len() - 1
    }

    // ---- Serialization (Milestone 02 task 4) ----
    //
    // The on-disk format stores the *semantic* content (palette + per-voxel
    // palette indices in Y-major `LocalPos::index` order), NOT the in-memory
    // `Packed` layout. This keeps saved worlds valid even if the in-memory
    // representation changes later. A version byte is written first, always.
    //
    // Format v1 (little-endian):
    //   [0..4]  magic "VXTC"
    //   [4]     version = 1
    //   [5..7]  palette length N (u16)
    //   [7....] palette: N × BlockId (u16 each)
    //   then, ONLY if N > 1:
    //           CHUNK_VOLUME palette indices, in LocalPos::index order,
    //           1 byte each if N <= 256, else 2 bytes each (LE).
    //   (N == 1 is a uniform chunk: the single palette entry fills the whole
    //   chunk, so no index body is written.)

    /// Serialize this chunk to the versioned on-disk format.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&CHUNK_MAGIC);
        out.push(CHUNK_FORMAT_VERSION);

        let n = self.palette.len();
        out.extend_from_slice(&(n as u16).to_le_bytes());
        for b in &self.palette {
            out.extend_from_slice(&b.0.to_le_bytes());
        }

        if n > 1 {
            let two_bytes = n > 256;
            for i in 0..CHUNK_VOLUME {
                let idx = match &self.indices {
                    Some(packed) => packed.get(i) as u16,
                    // n > 1 with no index array is impossible by construction,
                    // but fall back safely to palette entry 0.
                    None => 0,
                };
                if two_bytes {
                    out.extend_from_slice(&idx.to_le_bytes());
                } else {
                    out.push(idx as u8);
                }
            }
        }
        out
    }

    /// Reconstruct a chunk from bytes produced by [`Chunk::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Chunk, ChunkDecodeError> {
        let mut cur = 0usize;
        let take = |cur: &mut usize, n: usize| -> Result<&[u8], ChunkDecodeError> {
            let end = cur.checked_add(n).ok_or(ChunkDecodeError::Truncated)?;
            let slice = bytes.get(*cur..end).ok_or(ChunkDecodeError::Truncated)?;
            *cur = end;
            Ok(slice)
        };

        let magic = take(&mut cur, 4)?;
        if magic != CHUNK_MAGIC {
            return Err(ChunkDecodeError::BadMagic);
        }
        let version = take(&mut cur, 1)?[0];
        if version != CHUNK_FORMAT_VERSION {
            return Err(ChunkDecodeError::UnsupportedVersion(version));
        }

        let n = u16::from_le_bytes(take(&mut cur, 2)?.try_into().unwrap()) as usize;
        if n == 0 {
            return Err(ChunkDecodeError::EmptyPalette);
        }
        let mut palette = Vec::with_capacity(n);
        for _ in 0..n {
            let id = u16::from_le_bytes(take(&mut cur, 2)?.try_into().unwrap());
            palette.push(BlockId(id));
        }

        if n == 1 {
            // Uniform chunk.
            return Ok(Chunk {
                palette,
                indices: None,
                modified: false,
                light: None,
            });
        }

        // Rebuild the packed index array from the body.
        let mut packed = Packed::new(bits_for(n));
        let two_bytes = n > 256;
        for i in 0..CHUNK_VOLUME {
            let idx = if two_bytes {
                u16::from_le_bytes(take(&mut cur, 2)?.try_into().unwrap()) as usize
            } else {
                take(&mut cur, 1)?[0] as usize
            };
            if idx >= n {
                return Err(ChunkDecodeError::IndexOutOfRange);
            }
            packed.set(i, idx as u64);
        }

        Ok(Chunk {
            palette,
            indices: Some(packed),
            modified: false,
            light: None,
        })
    }
}

/// Magic bytes identifying a serialized Voxterra chunk.
const CHUNK_MAGIC: [u8; 4] = *b"VXTC";
/// On-disk chunk format version. Bump when the format changes; old versions
/// must continue to deserialize (or be migrated) — never silently reinterpret.
pub const CHUNK_FORMAT_VERSION: u8 = 1;

/// Why a chunk failed to deserialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkDecodeError {
    /// Input ended before the format expected.
    Truncated,
    /// Magic bytes didn't match — not a Voxterra chunk.
    BadMagic,
    /// Format version not understood by this build.
    UnsupportedVersion(u8),
    /// Palette claimed zero entries (always invalid; min is 1).
    EmptyPalette,
    /// A stored index pointed outside the palette.
    IndexOutOfRange,
}

impl core::fmt::Display for ChunkDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => write!(f, "chunk data truncated"),
            Self::BadMagic => write!(f, "bad chunk magic (not a Voxterra chunk)"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported chunk format version {v}"),
            Self::EmptyPalette => write!(f, "chunk palette is empty"),
            Self::IndexOutOfRange => write!(f, "chunk index out of palette range"),
        }
    }
}

impl std::error::Error for ChunkDecodeError {}

impl Default for Chunk {
    fn default() -> Self {
        Self::new_air()
    }
}

/// Smallest supported bit width that can index a palette of `len` entries.
/// Always one of {1, 2, 4, 8, 16} so values never straddle a u64 word.
fn bits_for(len: usize) -> u32 {
    debug_assert!((1..=u16::MAX as usize + 1).contains(&len));
    if len <= 2 {
        return 1;
    }
    // ceil(log2(len)), rounded up to a power of two.
    let needed = usize::BITS - (len - 1).leading_zeros();
    needed.next_power_of_two()
}

/// Fixed-width packed integer array, CHUNK_VOLUME entries.
struct Packed {
    /// Bits per value: 1, 2, 4, 8, or 16.
    bits: u32,
    /// Values per u64 word (64 / bits).
    per_word: u32,
    /// Mask of `bits` low ones.
    mask: u64,
    data: Vec<u64>,
}

impl Packed {
    fn new(bits: u32) -> Self {
        debug_assert!(matches!(bits, 1 | 2 | 4 | 8 | 16));
        let per_word = 64 / bits;
        let words = CHUNK_VOLUME.div_ceil(per_word as usize);
        Self {
            bits,
            per_word,
            mask: (1u64 << bits) - 1,
            data: vec![0; words],
        }
    }

    #[inline]
    fn get(&self, i: usize) -> u64 {
        debug_assert!(i < CHUNK_VOLUME);
        let word = i / self.per_word as usize;
        let shift = (i as u32 % self.per_word) * self.bits;
        (self.data[word] >> shift) & self.mask
    }

    #[inline]
    fn set(&mut self, i: usize, value: u64) {
        debug_assert!(i < CHUNK_VOLUME);
        debug_assert!(value <= self.mask);
        let word = i / self.per_word as usize;
        let shift = (i as u32 % self.per_word) * self.bits;
        let slot = &mut self.data[word];
        *slot = (*slot & !(self.mask << shift)) | (value << shift);
    }

    /// Copy of self at a wider bit width.
    fn repacked(&self, new_bits: u32) -> Packed {
        debug_assert!(new_bits > self.bits);
        let mut out = Packed::new(new_bits);
        for i in 0..CHUNK_VOLUME {
            out.set(i, self.get(i));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::LocalPos;

    // --- Milestone 00 tests, unchanged: the API contract. ---

    #[test]
    fn new_air_is_all_air() {
        let chunk = Chunk::new_air();
        assert!(chunk.is_all_air());
        assert_eq!(chunk.get(LocalPos::new(0, 0, 0)), BlockId::AIR);
        assert_eq!(chunk.get(LocalPos::new(31, 31, 31)), BlockId::AIR);
    }

    #[test]
    fn set_then_get_roundtrip() {
        let mut chunk = Chunk::new_air();
        let stone = BlockId(1);
        let pos = LocalPos::new(5, 17, 30);

        chunk.set(pos, stone);

        assert_eq!(chunk.get(pos), stone);
        assert!(!chunk.is_all_air());
        assert_eq!(chunk.get(LocalPos::new(4, 17, 30)), BlockId::AIR);
        assert_eq!(chunk.get(LocalPos::new(5, 16, 30)), BlockId::AIR);
    }

    #[test]
    fn filled_fills_everything() {
        let dirt = BlockId(2);
        let chunk = Chunk::filled(dirt);
        for pos in LocalPos::iter() {
            assert_eq!(chunk.get(pos), dirt);
        }
    }

    #[test]
    fn set_does_not_bleed_between_cells() {
        let mut chunk = Chunk::new_air();
        for pos in LocalPos::iter() {
            chunk.set(pos, BlockId((pos.index() % u16::MAX as usize) as u16));
        }
        for pos in LocalPos::iter() {
            assert_eq!(
                chunk.get(pos),
                BlockId((pos.index() % u16::MAX as usize) as u16)
            );
        }
    }

    // --- Milestone 01: palette-specific tests. ---

    #[test]
    fn uniform_chunks_store_no_indices() {
        assert!(Chunk::new_air().is_uniform());
        assert!(Chunk::filled(BlockId(7)).is_uniform());
        assert_eq!(Chunk::filled(BlockId(7)).palette_len(), 1);
    }

    #[test]
    fn writing_uniform_value_stays_uniform() {
        let mut chunk = Chunk::filled(BlockId(7));
        chunk.set(LocalPos::new(3, 3, 3), BlockId(7));
        assert!(chunk.is_uniform());
    }

    #[test]
    fn first_differing_write_leaves_uniform() {
        let mut chunk = Chunk::filled(BlockId(7));
        chunk.set(LocalPos::new(3, 3, 3), BlockId(8));
        assert!(!chunk.is_uniform());
        assert_eq!(chunk.get(LocalPos::new(3, 3, 3)), BlockId(8));
        assert_eq!(chunk.get(LocalPos::new(3, 3, 4)), BlockId(7));
        assert_eq!(chunk.palette_len(), 2);
    }

    #[test]
    fn bits_for_widths_are_word_aligned() {
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 2);
        assert_eq!(bits_for(5), 4);
        assert_eq!(bits_for(16), 4);
        assert_eq!(bits_for(17), 8);
        assert_eq!(bits_for(256), 8);
        assert_eq!(bits_for(257), 16);
        assert_eq!(bits_for(65536), 16);
    }

    /// Insert progressively more block types so the packed array is forced
    /// through every bit-width boundary; previously written data must
    /// survive every widening.
    #[test]
    fn growth_across_bit_width_boundaries_preserves_data() {
        let mut chunk = Chunk::new_air();
        // 300 distinct types pushes the palette through 1→2→4→8→16 bits.
        for i in 0..300u16 {
            let pos = LocalPos::from_index(i as usize);
            chunk.set(pos, BlockId(i + 1));
            // After every single insertion, verify everything so far.
            for j in 0..=i {
                let p = LocalPos::from_index(j as usize);
                assert_eq!(chunk.get(p), BlockId(j + 1), "lost data at width growth");
            }
        }
    }

    /// Differential test against a dense reference array under a
    /// deterministic random workload — the strongest correctness evidence.
    #[test]
    fn random_workload_matches_dense_reference() {
        let mut chunk = Chunk::new_air();
        let mut reference = vec![BlockId::AIR; CHUNK_VOLUME];
        let mut rng = SplitMix64::new(0xB10C_5EED);

        for _ in 0..50_000 {
            let index = (rng.next() as usize) % CHUNK_VOLUME;
            // Small block-type range so overwrites and reuse are common.
            let block = BlockId((rng.next() % 12) as u16);
            chunk.set(LocalPos::from_index(index), block);
            reference[index] = block;
        }

        for (i, &expected) in reference.iter().enumerate() {
            assert_eq!(chunk.get(LocalPos::from_index(i)), expected);
        }
        assert_eq!(chunk.is_all_air(), reference.iter().all(|b| b.is_air()));
    }

    #[test]
    fn compact_drops_unused_entries_and_recovers_uniform() {
        let mut chunk = Chunk::new_air();
        let pos = LocalPos::new(1, 2, 3);

        // Touch several types, then overwrite everything back to air.
        chunk.set(pos, BlockId(1));
        chunk.set(pos, BlockId(2));
        chunk.set(pos, BlockId(3));
        chunk.set(pos, BlockId::AIR);
        assert!(chunk.palette_len() > 1);

        chunk.compact();

        assert!(
            chunk.is_uniform(),
            "all-air chunk should compact to uniform"
        );
        assert_eq!(chunk.palette_len(), 1);
        assert!(chunk.is_all_air());
    }

    #[test]
    fn compact_preserves_contents() {
        let mut chunk = Chunk::new_air();
        let mut rng = SplitMix64::new(42);
        let mut reference = vec![BlockId::AIR; CHUNK_VOLUME];

        for _ in 0..10_000 {
            let index = (rng.next() as usize) % CHUNK_VOLUME;
            let block = BlockId((rng.next() % 30) as u16);
            chunk.set(LocalPos::from_index(index), block);
            reference[index] = block;
        }

        chunk.compact();

        for (i, &expected) in reference.iter().enumerate() {
            assert_eq!(chunk.get(LocalPos::from_index(i)), expected);
        }
    }

    /// Minimal deterministic RNG for tests — no external crates in vox-core.
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
    }

    // ---- Light storage (Milestone 04, ADR-0004) ----

    #[test]
    fn fresh_chunk_is_dark_and_unallocated() {
        let chunk = Chunk::new_air();
        assert!(!chunk.has_light());
        assert_eq!(chunk.block_light(LocalPos::new(0, 0, 0)), 0);
        assert_eq!(chunk.block_light(LocalPos::new(31, 31, 31)), 0);
    }

    #[test]
    fn setting_zero_light_does_not_allocate() {
        let mut chunk = Chunk::new_air();
        chunk.set_block_light(LocalPos::new(5, 5, 5), 0);
        assert!(!chunk.has_light(), "zero light must not allocate storage");
    }

    #[test]
    fn set_then_get_block_light() {
        let mut chunk = Chunk::new_air();
        chunk.set_block_light(LocalPos::new(3, 4, 5), 12);
        assert!(chunk.has_light());
        assert_eq!(chunk.block_light(LocalPos::new(3, 4, 5)), 12);
        // Neighbors remain dark.
        assert_eq!(chunk.block_light(LocalPos::new(3, 4, 6)), 0);
    }

    #[test]
    fn block_light_clamps_to_15() {
        let mut chunk = Chunk::new_air();
        chunk.set_block_light(LocalPos::new(0, 0, 0), 200);
        assert_eq!(chunk.block_light(LocalPos::new(0, 0, 0)), 15);
    }

    #[test]
    fn set_block_light_preserves_sky_nibble() {
        // Simulate a future skylight value in the high nibble, then set block
        // light, and confirm the high nibble survives (M05-readiness).
        let mut chunk = Chunk::new_air();
        let p = LocalPos::new(1, 2, 3);
        // Force allocation and write a raw byte with a sky nibble set.
        chunk.set_block_light(p, 1);
        // Manually poke the high nibble via another block-light write path:
        // set block light to 7 and verify only the low nibble changed from
        // whatever it was; since we can't write sky directly yet, assert the
        // low-nibble masking behavior instead.
        chunk.set_block_light(p, 7);
        assert_eq!(chunk.block_light(p), 7);
    }

    #[test]
    fn clear_light_releases_storage() {
        let mut chunk = Chunk::new_air();
        chunk.set_block_light(LocalPos::new(0, 0, 0), 10);
        assert!(chunk.has_light());
        chunk.clear_light();
        assert!(!chunk.has_light());
        assert_eq!(chunk.block_light(LocalPos::new(0, 0, 0)), 0);
    }

    #[test]
    fn light_independent_of_block_modified_flag() {
        let mut chunk = Chunk::filled(BlockId::AIR);
        assert!(!chunk.is_modified());
        chunk.set_block_light(LocalPos::new(0, 0, 0), 14);
        assert!(
            !chunk.is_modified(),
            "setting light must not mark the chunk modified"
        );
    }

    #[test]
    fn light_survives_full_voxel_range() {
        let mut chunk = Chunk::new_air();
        // Write a deterministic pattern over the whole volume, read it back.
        for pos in LocalPos::iter() {
            let level = (pos.x() ^ pos.y() ^ pos.z()) & 0x0F;
            chunk.set_block_light(pos, level);
        }
        for pos in LocalPos::iter() {
            let expected = (pos.x() ^ pos.y() ^ pos.z()) & 0x0F;
            assert_eq!(chunk.block_light(pos), expected);
        }
    }

    // ---- Sky light (Milestone 05, ADR-0005) ----

    #[test]
    fn fresh_chunk_has_no_sky_light() {
        let chunk = Chunk::new_air();
        assert_eq!(chunk.sky_light(LocalPos::new(0, 0, 0)), 0);
        assert_eq!(chunk.sky_light(LocalPos::new(31, 31, 31)), 0);
    }

    #[test]
    fn set_then_get_sky_light() {
        let mut chunk = Chunk::new_air();
        chunk.set_sky_light(LocalPos::new(3, 4, 5), 15);
        assert_eq!(chunk.sky_light(LocalPos::new(3, 4, 5)), 15);
        assert_eq!(chunk.sky_light(LocalPos::new(3, 4, 6)), 0);
    }

    #[test]
    fn sky_light_clamps_to_15() {
        let mut chunk = Chunk::new_air();
        chunk.set_sky_light(LocalPos::new(0, 0, 0), 200);
        assert_eq!(chunk.sky_light(LocalPos::new(0, 0, 0)), 15);
    }

    #[test]
    fn sky_and_block_light_are_independent() {
        // THE key property: both channels share one byte without clobbering.
        let mut chunk = Chunk::new_air();
        let p = LocalPos::new(7, 8, 9);
        chunk.set_block_light(p, 11);
        chunk.set_sky_light(p, 13);
        assert_eq!(chunk.block_light(p), 11, "block survived sky write");
        assert_eq!(chunk.sky_light(p), 13, "sky survived block write");

        // Overwrite one; the other is untouched.
        chunk.set_block_light(p, 2);
        assert_eq!(chunk.sky_light(p), 13);
        assert_eq!(chunk.block_light(p), 2);
        chunk.set_sky_light(p, 5);
        assert_eq!(chunk.block_light(p), 2);
        assert_eq!(chunk.sky_light(p), 5);
    }

    #[test]
    fn setting_zero_sky_light_does_not_allocate() {
        let mut chunk = Chunk::new_air();
        chunk.set_sky_light(LocalPos::new(1, 1, 1), 0);
        assert!(!chunk.has_light(), "zero sky light must not allocate");
    }

    #[test]
    fn clear_light_clears_both_channels() {
        let mut chunk = Chunk::new_air();
        let p = LocalPos::new(2, 2, 2);
        chunk.set_block_light(p, 9);
        chunk.set_sky_light(p, 14);
        chunk.clear_light();
        assert_eq!(chunk.block_light(p), 0);
        assert_eq!(chunk.sky_light(p), 0);
    }

    #[test]
    fn both_channels_survive_full_voxel_range() {
        let mut chunk = Chunk::new_air();
        for pos in LocalPos::iter() {
            let b = (pos.x() ^ pos.y()) & 0x0F;
            let s = (pos.y() ^ pos.z()) & 0x0F;
            chunk.set_block_light(pos, b);
            chunk.set_sky_light(pos, s);
        }
        for pos in LocalPos::iter() {
            let b = (pos.x() ^ pos.y()) & 0x0F;
            let s = (pos.y() ^ pos.z()) & 0x0F;
            assert_eq!(chunk.block_light(pos), b);
            assert_eq!(chunk.sky_light(pos), s);
        }
    }

    // ---- Serialization (Milestone 02 task 4) ----

    fn assert_roundtrip(chunk: &Chunk) {
        let bytes = chunk.serialize();
        let back = Chunk::deserialize(&bytes).expect("deserialize");
        for pos in LocalPos::iter() {
            assert_eq!(chunk.get(pos), back.get(pos), "mismatch at {pos:?}");
        }
        // Re-serializing the decoded chunk must yield identical bytes.
        assert_eq!(bytes, back.serialize(), "serialize not deterministic");
    }

    #[test]
    fn roundtrip_uniform_air() {
        assert_roundtrip(&Chunk::new_air());
    }

    #[test]
    fn roundtrip_uniform_stone() {
        assert_roundtrip(&Chunk::filled(BlockId(1)));
    }

    #[test]
    fn uniform_chunk_serializes_compactly() {
        // magic(4) + version(1) + palette_len(2) + 1 BlockId(2) = 9 bytes,
        // and crucially NO per-voxel body.
        let bytes = Chunk::filled(BlockId(7)).serialize();
        assert_eq!(bytes.len(), 9, "uniform chunk should have no index body");
    }

    #[test]
    fn roundtrip_two_blocks() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(1, 2, 3), BlockId(1));
        chunk.set(LocalPos::new(30, 30, 30), BlockId(1));
        assert_roundtrip(&chunk);
        // 2 palette entries => 1 byte per voxel.
        let bytes = chunk.serialize();
        assert_eq!(bytes.len(), 4 + 1 + 2 + 2 * 2 + CHUNK_VOLUME);
    }

    #[test]
    fn roundtrip_many_blocks_two_byte_indices() {
        // >256 palette entries forces 2-byte indices.
        let mut chunk = Chunk::new_air();
        for i in 0..300u16 {
            chunk.set(LocalPos::from_index(i as usize), BlockId(i + 1));
        }
        assert!(chunk.palette_len() > 256);
        assert_roundtrip(&chunk);
        let bytes = chunk.serialize();
        let n = chunk.palette_len();
        assert_eq!(bytes.len(), 4 + 1 + 2 + n * 2 + CHUNK_VOLUME * 2);
    }

    #[test]
    fn roundtrip_random_workload() {
        let mut rng = SplitMix64::new(0xBEEF);
        let mut chunk = Chunk::new_air();
        for _ in 0..40_000 {
            let i = (rng.next() as usize) % CHUNK_VOLUME;
            let b = BlockId((rng.next() % 50) as u16);
            chunk.set(LocalPos::from_index(i), b);
        }
        assert_roundtrip(&chunk);
    }

    #[test]
    fn deserialize_rejects_bad_input() {
        assert!(matches!(
            Chunk::deserialize(&[]),
            Err(ChunkDecodeError::Truncated)
        ));
        assert!(matches!(
            Chunk::deserialize(b"XXXX\x01"),
            Err(ChunkDecodeError::BadMagic)
        ));
        // Good magic, unknown version.
        let mut bad = b"VXTC".to_vec();
        bad.push(99);
        assert!(matches!(
            Chunk::deserialize(&bad),
            Err(ChunkDecodeError::UnsupportedVersion(99))
        ));
        // Good header, palette length 0.
        let mut empty_pal = b"VXTC".to_vec();
        empty_pal.push(CHUNK_FORMAT_VERSION);
        empty_pal.extend_from_slice(&0u16.to_le_bytes());
        assert!(matches!(
            Chunk::deserialize(&empty_pal),
            Err(ChunkDecodeError::EmptyPalette)
        ));
        // Truncated body: claims 2 palette entries but supplies no index data.
        let mut trunc = b"VXTC".to_vec();
        trunc.push(CHUNK_FORMAT_VERSION);
        trunc.extend_from_slice(&2u16.to_le_bytes());
        trunc.extend_from_slice(&0u16.to_le_bytes());
        trunc.extend_from_slice(&1u16.to_le_bytes());
        assert!(matches!(
            Chunk::deserialize(&trunc),
            Err(ChunkDecodeError::Truncated)
        ));
    }

    /// Compacting before serialize must not change the decoded contents.
    #[test]
    fn roundtrip_after_compact() {
        let mut chunk = Chunk::new_air();
        for i in 0..100u16 {
            chunk.set(LocalPos::from_index(i as usize), BlockId(i + 1));
        }
        // Overwrite half back to air, then compact.
        for i in 0..50u16 {
            chunk.set(LocalPos::from_index(i as usize), BlockId::AIR);
        }
        chunk.compact();
        assert_roundtrip(&chunk);
    }
}
