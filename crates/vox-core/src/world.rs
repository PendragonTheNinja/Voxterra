//! The world: a sparse 3D grid of cubic chunks.
//!
//! Chunks exist at any `ChunkPos` on all three axes — there is no world
//! height limit and no column structure (see CLAUDE.md invariants).
//! Missing chunks read as air.

use std::collections::HashMap;

use crate::block::BlockId;
use crate::chunk::Chunk;
use crate::coords::{ChunkPos, WorldPos};

#[derive(Default)]
pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Block at a world position. Positions in chunks that don't exist
    /// read as air.
    pub fn get_block(&self, pos: WorldPos) -> BlockId {
        let (chunk_pos, local) = pos.split();
        self.chunks
            .get(&chunk_pos)
            .map_or(BlockId::AIR, |chunk| chunk.get(local))
    }

    /// Set a block at a world position, creating the containing chunk if
    /// needed. Writing air where no chunk exists is a no-op (it would
    /// create an all-air chunk for nothing).
    pub fn set_block(&mut self, pos: WorldPos, block: BlockId) {
        let (chunk_pos, local) = pos.split();
        match self.chunks.get_mut(&chunk_pos) {
            Some(chunk) => chunk.set(local, block),
            None => {
                if block.is_air() {
                    return;
                }
                let mut chunk = Chunk::new_air();
                chunk.set(local, block);
                self.chunks.insert(chunk_pos, chunk);
            }
        }
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Insert a chunk wholesale (worldgen's path). Returns the displaced
    /// chunk if one was already there.
    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) -> Option<Chunk> {
        self.chunks.insert(pos, chunk)
    }

    pub fn remove_chunk(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    pub fn chunks(&self) -> impl Iterator<Item = (ChunkPos, &Chunk)> {
        self.chunks.iter().map(|(pos, chunk)| (*pos, chunk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: BlockId = BlockId(1);

    #[test]
    fn missing_chunks_read_as_air() {
        let world = World::new();
        assert_eq!(world.get_block(WorldPos::new(0, 0, 0)), BlockId::AIR);
        assert_eq!(
            world.get_block(WorldPos::new(-1000, 99999, 12)),
            BlockId::AIR
        );
        assert_eq!(world.chunk_count(), 0);
    }

    #[test]
    fn set_creates_chunk_and_roundtrips() {
        let mut world = World::new();
        let pos = WorldPos::new(100, -200, 305);

        world.set_block(pos, STONE);

        assert_eq!(world.get_block(pos), STONE);
        assert_eq!(world.chunk_count(), 1);
        // Neighbor block in the same chunk is air.
        assert_eq!(world.get_block(WorldPos::new(101, -200, 305)), BlockId::AIR);
    }

    /// Adjacent world positions that straddle a chunk border must land in
    /// different chunks and both read back correctly — exercises the
    /// negative-coordinate math end to end.
    #[test]
    fn blocks_across_chunk_borders() {
        let mut world = World::new();
        let a = WorldPos::new(-1, 5, 5); // chunk (-1, 0, 0), local x=31
        let b = WorldPos::new(0, 5, 5); // chunk (0, 0, 0), local x=0

        world.set_block(a, STONE);
        world.set_block(b, BlockId(2));

        assert_eq!(world.chunk_count(), 2);
        assert_eq!(world.get_block(a), STONE);
        assert_eq!(world.get_block(b), BlockId(2));
    }

    #[test]
    fn writing_air_into_missing_chunk_creates_nothing() {
        let mut world = World::new();
        world.set_block(WorldPos::new(5, 5, 5), BlockId::AIR);
        assert_eq!(world.chunk_count(), 0);
    }

    #[test]
    fn overwriting_with_air_works_in_existing_chunk() {
        let mut world = World::new();
        let pos = WorldPos::new(5, 5, 5);
        world.set_block(pos, STONE);
        world.set_block(pos, BlockId::AIR);
        assert_eq!(world.get_block(pos), BlockId::AIR);
        // Chunk stays (possibly all-air); unload policy is Milestone 02's
        // concern, not set_block's.
        assert_eq!(world.chunk_count(), 1);
    }
}
