//! vox-core: foundational voxel types — blocks, chunks, coordinates.
//!
//! This crate is headless: it must never depend on wgpu, winit, or anything
//! graphical (see CLAUDE.md workspace rules).

pub mod block;
pub mod chunk;
pub mod coords;
pub mod light;
pub mod raycast;
pub mod registry;
pub mod storage;
pub mod streaming;
pub mod world;

pub use block::BlockId;
pub use chunk::{CHUNK_FORMAT_VERSION, Chunk, ChunkDecodeError};
pub use coords::{CHUNK_BITS, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, LocalPos, WorldPos};
pub use light::{
    ColumnMap, LightVolume, MAX_LIGHT, NeighborLight, apply_chunk_light, chunk_column_heights,
    chunk_column_occludes, chunk_light_plane, column_index, compute_chunk_light,
    propagate_block_light, propagate_sky_light, relight_chunk, remove_block_light,
};
pub use raycast::{RayHit, cell_overlaps_aabb, raycast_blocks, raycast_voxels};
pub use registry::{BlockRegistry, BlockType};
pub use storage::{StoreError, WORLD_META_VERSION, WorldStore};
pub use streaming::{StreamUpdate, Streamer};
pub use world::World;
