//! vox-core: foundational voxel types — blocks, chunks, coordinates.
//!
//! This crate is headless: it must never depend on wgpu, winit, or anything
//! graphical (see CLAUDE.md workspace rules).

pub mod block;
pub mod chunk;
pub mod coords;
pub mod raycast;
pub mod registry;
pub mod storage;
pub mod streaming;
pub mod world;

pub use block::BlockId;
pub use chunk::{CHUNK_FORMAT_VERSION, Chunk, ChunkDecodeError};
pub use coords::{CHUNK_BITS, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, LocalPos, WorldPos};
pub use raycast::{RayHit, raycast_blocks, raycast_voxels};
pub use registry::{BlockRegistry, BlockType};
pub use storage::{StoreError, WORLD_META_VERSION, WorldStore};
pub use streaming::{StreamUpdate, Streamer};
pub use world::World;
