//! vox-core: foundational voxel types — blocks, chunks, coordinates.
//!
//! This crate is headless: it must never depend on wgpu, winit, or anything
//! graphical (see CLAUDE.md workspace rules).

pub mod block;
pub mod chunk;
pub mod coords;
pub mod streaming;
pub mod world;

pub use block::BlockId;
pub use chunk::Chunk;
pub use coords::{CHUNK_BITS, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, LocalPos, WorldPos};
pub use streaming::{StreamUpdate, Streamer};
pub use world::World;
