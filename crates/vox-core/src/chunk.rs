//! Chunk block storage.
//!
//! MILESTONE 00 SCAFFOLDING NOTE: storage is currently a dense flat array
//! (64 KiB per chunk). Milestone 01 replaces the *internals* with palette
//! compression; the public API of [`Chunk`] is the contract and must not
//! leak storage details. Nothing outside this module may assume dense
//! storage or a fixed bits-per-voxel.

use crate::block::BlockId;
use crate::coords::{CHUNK_VOLUME, LocalPos};

/// Block storage for one 32³ chunk.
pub struct Chunk {
    /// Indexed by [`LocalPos::index`] (Y-major layout).
    blocks: Box<[BlockId; CHUNK_VOLUME]>,
}

impl Chunk {
    /// A chunk uniformly filled with one block type.
    pub fn filled(block: BlockId) -> Self {
        let boxed: Box<[BlockId]> = vec![block; CHUNK_VOLUME].into_boxed_slice();
        Self {
            blocks: boxed.try_into().expect("length is CHUNK_VOLUME"),
        }
    }

    /// An all-air chunk.
    pub fn new_air() -> Self {
        Self::filled(BlockId::AIR)
    }

    #[inline]
    pub fn get(&self, pos: LocalPos) -> BlockId {
        self.blocks[pos.index()]
    }

    #[inline]
    pub fn set(&mut self, pos: LocalPos, block: BlockId) {
        self.blocks[pos.index()] = block;
    }

    /// True if every block in the chunk is air. (Meshing and rendering
    /// skip such chunks entirely.)
    pub fn is_all_air(&self) -> bool {
        self.blocks.iter().all(|b| b.is_air())
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new_air()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::LocalPos;

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
        // Neighbors untouched.
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
        // Write a unique value everywhere, then verify every cell
        // independently — catches any indexing aliasing bug.
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
}
