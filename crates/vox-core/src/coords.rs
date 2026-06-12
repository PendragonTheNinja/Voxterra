//! Coordinate types and conversions.
//!
//! Three position spaces, each a distinct newtype so they can never be
//! confused across a function boundary (see CLAUDE.md):
//!
//! - [`WorldPos`]: absolute block position in the world, `i64` per axis.
//! - [`ChunkPos`]: position of a 32³ chunk in the infinite 3D chunk grid.
//! - [`LocalPos`]: position *within* a chunk, `0..32` per axis.
//!
//! World → chunk uses an arithmetic right shift and world → local uses a
//! bitmask. This is deliberate: `/ 32` and `% 32` on signed integers round
//! toward zero and are WRONG for negative coordinates (e.g. `-1 / 32 == 0`,
//! but block -1 lives in chunk -1). Never reimplement this math at call
//! sites — use these helpers.

/// log2 of the chunk edge length.
pub const CHUNK_BITS: u32 = 5;
/// Chunk edge length in blocks (32).
pub const CHUNK_SIZE: usize = 1 << CHUNK_BITS;
/// Number of blocks in one chunk (32³ = 32768).
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Bitmask extracting the local (in-chunk) part of a world coordinate.
const LOCAL_MASK: i64 = (CHUNK_SIZE as i64) - 1;

/// Absolute block position in the world. Y is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldPos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

/// Position of a chunk in the 3D chunk grid (all axes, including Y —
/// chunks are cubic, there are no columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

/// Position within a chunk. Invariant: every component is `< CHUNK_SIZE`.
/// Construct via [`LocalPos::new`] (checked) — fields are private so the
/// invariant cannot be violated from outside this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalPos {
    x: u8,
    y: u8,
    z: u8,
}

impl WorldPos {
    #[inline]
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// The chunk containing this block.
    #[inline]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos::new(
            self.x >> CHUNK_BITS,
            self.y >> CHUNK_BITS,
            self.z >> CHUNK_BITS,
        )
    }

    /// This block's position within its chunk.
    #[inline]
    pub const fn local(self) -> LocalPos {
        LocalPos {
            x: (self.x & LOCAL_MASK) as u8,
            y: (self.y & LOCAL_MASK) as u8,
            z: (self.z & LOCAL_MASK) as u8,
        }
    }

    /// Split into (containing chunk, position within it).
    #[inline]
    pub const fn split(self) -> (ChunkPos, LocalPos) {
        (self.chunk(), self.local())
    }
}

impl ChunkPos {
    #[inline]
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// World position of this chunk's minimum corner (lowest x/y/z block).
    #[inline]
    pub const fn origin(self) -> WorldPos {
        WorldPos::new(
            self.x << CHUNK_BITS,
            self.y << CHUNK_BITS,
            self.z << CHUNK_BITS,
        )
    }

    /// Recompose a chunk position and a local position into a world position.
    #[inline]
    pub const fn world_pos(self, local: LocalPos) -> WorldPos {
        let o = self.origin();
        WorldPos::new(
            o.x + local.x as i64,
            o.y + local.y as i64,
            o.z + local.z as i64,
        )
    }
}

impl LocalPos {
    /// Create a local position. Panics in debug builds if any component
    /// is out of range; callers are responsible for the `< CHUNK_SIZE`
    /// invariant (hot loops should iterate via [`LocalPos::iter`] or
    /// ranges that guarantee it).
    #[inline]
    pub fn new(x: u8, y: u8, z: u8) -> Self {
        debug_assert!(
            (x as usize) < CHUNK_SIZE && (y as usize) < CHUNK_SIZE && (z as usize) < CHUNK_SIZE,
            "LocalPos out of range: ({x}, {y}, {z})"
        );
        Self { x, y, z }
    }

    #[inline]
    pub const fn x(self) -> u8 {
        self.x
    }
    #[inline]
    pub const fn y(self) -> u8 {
        self.y
    }
    #[inline]
    pub const fn z(self) -> u8 {
        self.z
    }

    /// Flat index into a chunk's block array.
    ///
    /// Layout is Y-major (`y`, then `z`, then `x`): horizontal slices are
    /// contiguous in memory, which is the access pattern worldgen and
    /// (later) skylight care about most. This layout is also the contract
    /// for the Milestone 01 palette storage and the serialized chunk
    /// format — do not change it casually.
    #[inline]
    pub const fn index(self) -> usize {
        ((self.y as usize) << (2 * CHUNK_BITS))
            | ((self.z as usize) << CHUNK_BITS)
            | (self.x as usize)
    }

    /// Inverse of [`LocalPos::index`].
    #[inline]
    pub fn from_index(index: usize) -> Self {
        debug_assert!(index < CHUNK_VOLUME, "index out of range: {index}");
        Self {
            x: (index & (CHUNK_SIZE - 1)) as u8,
            z: ((index >> CHUNK_BITS) & (CHUNK_SIZE - 1)) as u8,
            y: ((index >> (2 * CHUNK_BITS)) & (CHUNK_SIZE - 1)) as u8,
        }
    }

    /// Iterate every position in a chunk, in [`LocalPos::index`] order.
    pub fn iter() -> impl Iterator<Item = LocalPos> {
        (0..CHUNK_VOLUME).map(LocalPos::from_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_coords_split() {
        let (chunk, local) = WorldPos::new(33, 0, 70).split();
        assert_eq!(chunk, ChunkPos::new(1, 0, 2));
        assert_eq!(local, LocalPos::new(1, 0, 6));
    }

    /// THE classic bug: block -1 lives in chunk -1 at local 31, not chunk 0.
    #[test]
    fn negative_coords_split() {
        let (chunk, local) = WorldPos::new(-1, -32, -33).split();
        assert_eq!(chunk, ChunkPos::new(-1, -1, -2));
        assert_eq!(local, LocalPos::new(31, 0, 31));
    }

    #[test]
    fn chunk_boundaries() {
        // Exactly at a chunk's minimum corner.
        assert_eq!(WorldPos::new(32, 32, 32).chunk(), ChunkPos::new(1, 1, 1));
        assert_eq!(WorldPos::new(31, 31, 31).chunk(), ChunkPos::new(0, 0, 0));
        assert_eq!(WorldPos::new(0, 0, 0).chunk(), ChunkPos::new(0, 0, 0));
        assert_eq!(WorldPos::new(-32, 0, 0).chunk(), ChunkPos::new(-1, 0, 0));
    }

    /// split → recompose must be the identity for any world position,
    /// including deep negatives and huge values.
    #[test]
    fn split_recompose_roundtrip() {
        let cases = [
            WorldPos::new(0, 0, 0),
            WorldPos::new(-1, -1, -1),
            WorldPos::new(123_456, -789_012, 31),
            WorldPos::new(i64::MIN / 2, i64::MAX / 2, -1_000_000),
        ];
        for pos in cases {
            let (chunk, local) = pos.split();
            assert_eq!(chunk.world_pos(local), pos, "roundtrip failed for {pos:?}");
        }
    }

    #[test]
    fn chunk_origin() {
        assert_eq!(ChunkPos::new(-1, 0, 2).origin(), WorldPos::new(-32, 0, 64));
    }

    #[test]
    fn index_roundtrip_covers_whole_chunk() {
        for i in 0..CHUNK_VOLUME {
            assert_eq!(LocalPos::from_index(i).index(), i);
        }
    }

    #[test]
    fn index_layout_is_y_major() {
        assert_eq!(LocalPos::new(1, 0, 0).index(), 1);
        assert_eq!(LocalPos::new(0, 0, 1).index(), CHUNK_SIZE);
        assert_eq!(LocalPos::new(0, 1, 0).index(), CHUNK_SIZE * CHUNK_SIZE);
    }

    #[test]
    fn iter_visits_every_position_once() {
        assert_eq!(LocalPos::iter().count(), CHUNK_VOLUME);
        let mut seen = vec![false; CHUNK_VOLUME];
        for pos in LocalPos::iter() {
            assert!(!seen[pos.index()], "duplicate position {pos:?}");
            seen[pos.index()] = true;
        }
    }
}
