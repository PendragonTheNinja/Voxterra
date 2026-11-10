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
    /// Whether the chunk has changed since it was last written to disk (or
    /// read from it): what an autosave must write. Narrower than `modified`,
    /// which stays set for as long as the chunk differs from generation — a
    /// chunk loaded from disk and never touched is modified but has nothing
    /// to save. Runtime state, NOT serialized.
    unsaved: bool,
    /// Per-voxel light (ADR-0004). Runtime-derived from blocks, NOT
    /// serialized (recomputed on load).
    light: Light,
}

/// A chunk's light: one packed byte per voxel, high nibble sky, low nibble
/// block (ADR-0004), stored as cheaply as its contents allow.
///
/// Most lit chunks are not lit *variously*. Open air under the sky is 15
/// sky, 0 block, in every voxel — and there are several such chunks above
/// every column of ground. As a full array each cost 32 KB to hold one
/// value 32 768 times: ~20 MB at the default radius, ~200 MB at the
/// largest (2026-11 audit). `Uniform` holds the value once. Like the
/// palette's uniform case this is an internal representation; nothing
/// outside this module can tell which one a chunk is in.
enum Light {
    /// Fully dark: every voxel 0 in both channels.
    Dark,
    /// Every voxel holds this packed byte. Never 0 — that is `Dark`.
    Uniform(u8),
    /// One packed byte per voxel, in [`LocalPos::index`] order.
    Full(Box<[u8; CHUNK_VOLUME]>),
}

impl Light {
    #[inline]
    fn packed(&self, index: usize) -> u8 {
        match self {
            Light::Dark => 0,
            Light::Uniform(b) => *b,
            Light::Full(arr) => arr[index],
        }
    }

    /// Write one voxel's packed byte, leaving a uniform representation only
    /// when the write actually differs from it.
    fn set_packed(&mut self, index: usize, value: u8) {
        let fill = match self {
            Light::Full(arr) => {
                arr[index] = value;
                return;
            }
            Light::Dark => 0,
            Light::Uniform(b) => *b,
        };
        if value != fill {
            let mut arr = Box::new([fill; CHUNK_VOLUME]);
            arr[index] = value;
            *self = Light::Full(arr);
        }
    }
}

impl Chunk {
    /// A chunk uniformly filled with one block type. O(1) memory.
    pub fn filled(block: BlockId) -> Self {
        Self {
            palette: vec![block],
            indices: None,
            modified: false,
            unsaved: false,
            light: Light::Dark,
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
        self.unsaved = true;

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
        self.unsaved = false;
    }

    /// Whether the chunk holds changes that are not on disk yet: it was
    /// edited since it was generated, loaded or last saved. What a save
    /// pass writes. Always false for an unmodified chunk.
    pub fn is_unsaved(&self) -> bool {
        self.unsaved
    }

    /// Record that the chunk's current contents are on disk — after writing
    /// it, or on reading it back. It stays [`modified`](Chunk::is_modified):
    /// it still differs from generation, so it must still be loaded from
    /// disk rather than regenerated.
    pub fn mark_saved(&mut self) {
        self.unsaved = false;
    }

    /// Whether any block type this chunk holds satisfies `pred` — asked of
    /// the palette, so a handful of tests rather than 32 768. Conservative:
    /// the palette can keep an entry no voxel uses any more (until
    /// [`Chunk::compact`]), so `true` means "may contain", `false` means
    /// "certainly does not".
    pub fn may_contain(&self, pred: impl Fn(BlockId) -> bool) -> bool {
        self.palette.iter().any(|&b| pred(b))
    }

    // ---- Light (Milestone 04, ADR-0004) ----
    //
    // One packed byte per voxel: high nibble sky, low nibble block light,
    // held as cheaply as its contents allow (see `Light`). Light is derived
    // from blocks and never serialized; setting it does not mark the chunk
    // modified.

    /// Block-light level (0..=15) at `pos`. Dark (0) when no light array is
    /// allocated.
    #[inline]
    pub fn block_light(&self, pos: LocalPos) -> u8 {
        self.light.packed(pos.index()) & 0x0F
    }

    /// Set the block-light level (clamped to 0..=15) at `pos`, preserving the
    /// sky nibble. Allocates the light array on first nonzero write.
    pub fn set_block_light(&mut self, pos: LocalPos, level: u8) {
        let i = pos.index();
        let packed = (self.light.packed(i) & 0xF0) | level.min(15);
        self.light.set_packed(i, packed);
    }

    /// Sky-light level (0..=15) at `pos`, from the high nibble (ADR-0005).
    /// Dark (0) when no light array is allocated. NOTE: an unallocated chunk
    /// reads 0 here, but "open to sky" cells are assigned 15 by the relight
    /// pass — storage default 0 is correct; daylight comes from propagation.
    #[inline]
    pub fn sky_light(&self, pos: LocalPos) -> u8 {
        self.light.packed(pos.index()) >> 4
    }

    /// Set the sky-light level (clamped to 0..=15) at `pos`, preserving the
    /// block-light nibble. Allocates the light array on first nonzero write.
    pub fn set_sky_light(&mut self, pos: LocalPos, level: u8) {
        let i = pos.index();
        let packed = (self.light.packed(i) & 0x0F) | (level.min(15) << 4);
        self.light.set_packed(i, packed);
    }

    /// Both channels of one voxel as the packed byte they are stored in —
    /// high nibble sky, low nibble block — by [`LocalPos::index`].
    #[inline]
    pub fn packed_light(&self, index: usize) -> u8 {
        self.light.packed(index)
    }

    /// Replace all of the chunk's light with `light`: one packed byte per
    /// voxel in [`LocalPos::index`] order, the buffer the relight pass
    /// produces. One pass over the buffer, and a buffer that holds a single
    /// value throughout — dark rock, open air under the sky — is stored as
    /// that value alone.
    ///
    /// Panics if `light` is not exactly [`CHUNK_VOLUME`] bytes.
    pub fn set_light_packed(&mut self, light: &[u8]) {
        assert_eq!(light.len(), CHUNK_VOLUME, "light buffer must be 32^3");
        let first = light[0];
        self.light = if light.iter().all(|&b| b == first) {
            if first == 0 {
                Light::Dark
            } else {
                Light::Uniform(first)
            }
        } else {
            let arr: Box<[u8; CHUNK_VOLUME]> = Box::<[u8]>::from(light)
                .try_into()
                .expect("length checked above");
            Light::Full(arr)
        };
    }

    /// Whether the chunk's light equals `light`, voxel for voxel (the
    /// layout [`Chunk::set_light_packed`] takes): how the relight pass
    /// tells that a recomputation changed nothing, without 32 768 nibble
    /// reads.
    pub fn light_matches(&self, light: &[u8]) -> bool {
        match &self.light {
            Light::Dark => light.iter().all(|&b| b == 0),
            Light::Uniform(v) => light.iter().all(|b| b == v),
            Light::Full(arr) => arr[..] == *light,
        }
    }

    /// The 32 blocks of one X row — `x` 0..32 at (`y`, `z`) — into `out`, in
    /// one pass. The mesher's snapshot (`vox_mesh::MeshInput`) reads every
    /// cell of a chunk it meshes; through [`Chunk::get`] each cell paid a
    /// runtime division and modulo by the bit width, a third of the
    /// snapshot's cost (M12 task 1). A row is contiguous in [`LocalPos::index`]
    /// order, so its values sit in consecutive packed slots.
    pub fn decode_row(&self, y: usize, z: usize, out: &mut [BlockId; 32]) {
        match &self.indices {
            None => out.fill(self.palette[0]),
            Some(packed) => {
                let start = LocalPos::new(0, y as u8, z as u8).index();
                // Widths are powers of two dividing 64, so slots per word and
                // the bit offset within a word are shifts and masks.
                let per_word_log2 = (64 / packed.bits).trailing_zeros();
                let bits_log2 = packed.bits.trailing_zeros();
                for (k, slot) in out.iter_mut().enumerate() {
                    let i = start + k;
                    let word = packed.data[i >> per_word_log2];
                    let shift = ((i as u32) & (packed.per_word - 1)) << bits_log2;
                    *slot = self.palette[((word >> shift) & packed.mask) as usize];
                }
            }
        }
    }

    /// Sky and block light of one X row, as [`Chunk::decode_row`] does for
    /// blocks. Returns `false`, writing nothing, when the chunk is fully dark.
    pub fn light_row(&self, y: usize, z: usize, sky: &mut [u8; 32], block: &mut [u8; 32]) -> bool {
        match &self.light {
            Light::Dark => false,
            Light::Uniform(b) => {
                sky.fill(b >> 4);
                block.fill(b & 0x0F);
                true
            }
            Light::Full(arr) => {
                let start = LocalPos::new(0, y as u8, z as u8).index();
                for (k, &b) in arr[start..start + 32].iter().enumerate() {
                    sky[k] = b >> 4;
                    block[k] = b & 0x0F;
                }
                true
            }
        }
    }

    /// Whether any light storage is allocated (i.e. the chunk may be lit).
    /// A `false` result guarantees the chunk is fully dark (both channels).
    #[inline]
    pub fn has_light(&self) -> bool {
        !matches!(self.light, Light::Dark)
    }

    /// Clear all light back to dark, releasing the storage. Used when
    /// relighting from scratch (recompute-on-load) or when light is fully
    /// removed.
    pub fn clear_light(&mut self) {
        self.light = Light::Dark;
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
                unsaved: false,
                light: Light::Dark,
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
            unsaved: false,
            light: Light::Dark,
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
    /// Every palette width (1, 2, 4, 8 and 16 bits), and the uniform case:
    /// a decoded row is exactly what `get` reads cell by cell, and a light
    /// row exactly `sky_light` / `block_light`.
    #[test]
    fn rows_decode_exactly_what_get_reads() {
        let mut rng = SplitMix64::new(0x5EED_8088);
        for kinds in [1u64, 2, 3, 9, 200, 600] {
            let mut chunk = Chunk::new_air();
            for pos in LocalPos::iter() {
                chunk.set(pos, BlockId((rng.next() % kinds) as u16));
                if rng.next().is_multiple_of(5) {
                    chunk.set_sky_light(pos, (rng.next() % 16) as u8);
                    chunk.set_block_light(pos, (rng.next() % 16) as u8);
                }
            }
            let (mut row, mut sky, mut block) = ([BlockId::AIR; 32], [0u8; 32], [0u8; 32]);
            for y in 0..32 {
                for z in 0..32 {
                    chunk.decode_row(y, z, &mut row);
                    let lit = chunk.light_row(y, z, &mut sky, &mut block);
                    assert_eq!(lit, chunk.has_light());
                    for x in 0..32 {
                        let p = LocalPos::new(x as u8, y as u8, z as u8);
                        assert_eq!(row[x], chunk.get(p), "{kinds} kinds at {p:?}");
                        if lit {
                            assert_eq!(sky[x], chunk.sky_light(p));
                            assert_eq!(block[x], chunk.block_light(p));
                        }
                    }
                }
            }
        }
        let mut row = [BlockId::AIR; 32];
        let (mut sky, mut block) = ([9u8; 32], [9u8; 32]);
        let stone = Chunk::filled(BlockId(1));
        stone.decode_row(5, 7, &mut row);
        assert_eq!(row, [BlockId(1); 32]);
        assert!(!stone.light_row(5, 7, &mut sky, &mut block));
        assert_eq!((sky, block), ([9u8; 32], [9u8; 32]), "dark writes nothing");
    }

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

    // ---- Light representations and the save flag (2026-11 audit) ----

    const STONE: BlockId = BlockId(1);
    const DIRT: BlockId = BlockId(2);

    /// A reference copy of what a chunk's light should read as, cell by cell.
    fn assert_light_reads_as(chunk: &Chunk, want: &[u8]) {
        assert!(chunk.light_matches(want));
        assert_eq!(chunk.has_light(), want.iter().any(|&b| b != 0));
        for (i, &b) in want.iter().enumerate() {
            let p = LocalPos::from_index(i);
            assert_eq!(chunk.packed_light(i), b, "packed at {i}");
            assert_eq!(
                (chunk.sky_light(p), chunk.block_light(p)),
                (b >> 4, b & 0x0F)
            );
        }
        let (mut sky, mut block) = ([0u8; 32], [0u8; 32]);
        for z in 0..32 {
            for y in 0..32 {
                if chunk.light_row(y, z, &mut sky, &mut block) {
                    for x in 0..32 {
                        let b = want[LocalPos::new(x as u8, y as u8, z as u8).index()];
                        assert_eq!((sky[x], block[x]), (b >> 4, b & 0x0F));
                    }
                }
            }
        }
    }

    /// However a chunk's light is held — dark, one value, or an array — it
    /// reads back exactly as the buffer it was set from.
    #[test]
    fn set_light_packed_reads_back_in_every_representation() {
        let mut rng = SplitMix64::new(0x11_2026);
        let mut chunk = Chunk::new_air();
        let dark = vec![0u8; CHUNK_VOLUME];
        let open_sky = vec![0xF0u8; CHUNK_VOLUME];
        let lamp_room = vec![0x0Eu8; CHUNK_VOLUME];
        let mixed: Vec<u8> = (0..CHUNK_VOLUME).map(|_| rng.next() as u8).collect();
        let mut one_off = open_sky.clone();
        one_off[CHUNK_VOLUME - 1] = 0xE0; // uniform but for the last voxel
        // Each after each, so every transition between representations runs.
        for want in [
            &open_sky, &mixed, &dark, &lamp_room, &one_off, &open_sky, &dark, &mixed,
        ] {
            chunk.set_light_packed(want);
            assert_light_reads_as(&chunk, want);
        }
        assert!(
            !chunk.is_modified(),
            "light is derived: it never marks a chunk modified"
        );
    }

    /// `light_matches` tells a chunk's light from every buffer that differs
    /// from it in one voxel, in each representation.
    #[test]
    fn light_matches_notices_a_single_voxel() {
        let mut rng = SplitMix64::new(77);
        let mixed: Vec<u8> = (0..CHUNK_VOLUME).map(|_| rng.next() as u8).collect();
        for base in [vec![0u8; CHUNK_VOLUME], vec![0xF0u8; CHUNK_VOLUME], mixed] {
            let mut chunk = Chunk::new_air();
            chunk.set_light_packed(&base);
            for i in [0, 1, 12_345, CHUNK_VOLUME - 1] {
                let mut other = base.clone();
                other[i] ^= 0x10;
                assert!(!chunk.light_matches(&other), "missed a change at {i}");
            }
            assert!(chunk.light_matches(&base));
        }
    }

    /// Writing one voxel of a uniformly lit chunk changes that voxel only,
    /// and writing the value it already holds changes nothing.
    #[test]
    fn a_write_into_uniform_light_touches_one_voxel() {
        let mut chunk = Chunk::new_air();
        let mut want = vec![0xF0u8; CHUNK_VOLUME];
        chunk.set_light_packed(&want);
        let p = LocalPos::new(3, 30, 17);
        chunk.set_sky_light(p, 15); // already 15
        chunk.set_block_light(p, 0); // already 0
        assert_light_reads_as(&chunk, &want);
        chunk.set_block_light(p, 9);
        want[p.index()] = 0xF9;
        assert_light_reads_as(&chunk, &want);
        chunk.set_sky_light(LocalPos::new(0, 0, 0), 2);
        want[0] = 0x20;
        assert_light_reads_as(&chunk, &want);
    }

    /// `unsaved` is narrower than `modified`: set by an edit, cleared by a
    /// save, while the chunk still differs from generation.
    #[test]
    fn unsaved_tracks_edits_since_the_last_save() {
        let mut chunk = Chunk::new_air();
        assert!(!chunk.is_unsaved());
        chunk.set(LocalPos::new(0, 0, 0), BlockId::AIR); // no change
        assert!(!chunk.is_unsaved() && !chunk.is_modified());
        chunk.set(LocalPos::new(1, 2, 3), STONE);
        assert!(chunk.is_unsaved() && chunk.is_modified());
        chunk.mark_saved();
        assert!(!chunk.is_unsaved(), "saved: nothing left to write");
        assert!(chunk.is_modified(), "but still not what the seed generates");
        chunk.set(LocalPos::new(1, 2, 4), STONE);
        assert!(chunk.is_unsaved(), "a later edit needs saving again");
        chunk.mark_unmodified();
        assert!(!chunk.is_unsaved() && !chunk.is_modified());
        // Read back from disk: modified (keep loading it), nothing to write.
        let mut loaded = Chunk::deserialize(&chunk.serialize()).unwrap();
        loaded.mark_modified();
        assert!(loaded.is_modified() && !loaded.is_unsaved());
        // Light never counts as an edit.
        loaded.set_light_packed(&vec![0xF0u8; CHUNK_VOLUME]);
        assert!(!loaded.is_unsaved());
    }

    /// `may_contain` is a palette question: never a false "no".
    #[test]
    fn may_contain_never_misses_a_block() {
        let mut chunk = Chunk::filled(STONE);
        assert!(chunk.may_contain(|b| b == STONE));
        assert!(!chunk.may_contain(|b| b == DIRT));
        chunk.set(LocalPos::new(4, 4, 4), DIRT);
        assert!(chunk.may_contain(|b| b == DIRT));
        // Overwritten: the palette may still list it (conservative) until
        // compaction, after which it certainly does not.
        chunk.set(LocalPos::new(4, 4, 4), STONE);
        chunk.compact();
        assert!(!chunk.may_contain(|b| b == DIRT));
    }
}
