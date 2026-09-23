//! vox-worldgen: terrain generation.
//!
//! MILESTONE 01 STATUS: this is **placeholder** generation — a seeded
//! value-noise heightmap, just enough to give the chunk streamer
//! (Milestone 02) something to stream and the renderer something to draw.
//! It is NOT the real geology pipeline (tectonics → stratigraphy →
//! associative ore); that is a dedicated milestone after lighting/textures
//! (see CLAUDE.md and ADR-0001).
//!
//! Two invariants this module exists to establish early, because the
//! streamer and (eventually) the saved-world format depend on them:
//!
//! - **Per-chunk independence.** [`Generator::generate_chunk`] produces any
//!   `ChunkPos` at any Y without generating its neighbors or anything
//!   "below" it. Cubic chunks require this; column-based shortcuts are
//!   forbidden (CLAUDE.md).
//! - **Determinism.** Same `(seed, ChunkPos)` → byte-identical chunk,
//!   forever. This is what makes a world reproducible from a seed and lets
//!   the streamer regenerate instead of always loading from disk.

pub mod elevation;

use elevation::Elevation;
use vox_core::{BlockId, CHUNK_SIZE, Chunk, ChunkPos, LocalPos};

/// Block ids used by the placeholder generator. These now come from the
/// canonical block registry in vox-core (Milestone 03); re-exported here so
/// existing call sites (`blocks::STONE`, etc.) keep working unchanged.
pub mod blocks {
    pub use vox_core::registry::{AIR, DIRT, GRASS, STONE};
}

/// Number of dirt blocks below the surface grass layer.
const DIRT_DEPTH: i64 = 4;

/// Terrain generator for one world seed.
#[derive(Clone, Copy, Debug)]
pub struct Generator {
    seed: u64,
    elevation: Elevation,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            elevation: Elevation::new(seed),
        }
    }

    /// The elevation field backing [`Self::surface_height`]. Exposed so callers
    /// can query individual stages (debug views, tuning) without duplicating
    /// the composition.
    pub fn elevation(&self) -> &Elevation {
        &self.elevation
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Surface height (the Y of the topmost solid/grass block) at world
    /// column (wx, wz). Smooth value noise: bilinearly interpolate a hashed
    /// lattice so adjacent columns agree and chunk borders line up exactly.
    ///
    /// Independent of Y and of chunk boundaries — two chunks stacked
    /// vertically compute the same surface for the same column, which is
    /// what keeps cubic chunks seamless.
    pub fn surface_height(&self, wx: i64, wz: i64) -> i64 {
        self.elevation.height(wx, wz)
    }

    /// Generate the chunk at `pos` independently. Empty (all-air) chunks —
    /// the common case far above the surface — return a uniform chunk for
    /// free (no per-voxel work, O(1) storage).
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let origin = pos.origin();
        let chunk_min_y = origin.y;
        let chunk_max_y = origin.y + CHUNK_SIZE as i64 - 1;

        // Fast rejection: find the surface range over this chunk's columns.
        // If the whole chunk is above the highest surface, it's all air; if
        // entirely well below the lowest surface, it's all stone.
        let mut min_surface = i64::MAX;
        let mut max_surface = i64::MIN;
        for lz in 0..CHUNK_SIZE as i64 {
            for lx in 0..CHUNK_SIZE as i64 {
                let h = self.surface_height(origin.x + lx, origin.z + lz);
                min_surface = min_surface.min(h);
                max_surface = max_surface.max(h);
            }
        }

        if chunk_min_y > max_surface {
            return Chunk::filled(blocks::AIR);
        }
        if chunk_max_y < min_surface - DIRT_DEPTH {
            return Chunk::filled(blocks::STONE);
        }

        // Mixed chunk: fill per column.
        let mut chunk = Chunk::new_air();
        for lz in 0..CHUNK_SIZE as u8 {
            for lx in 0..CHUNK_SIZE as u8 {
                let height = self.surface_height(origin.x + lx as i64, origin.z + lz as i64);
                for ly in 0..CHUNK_SIZE as u8 {
                    let wy = chunk_min_y + ly as i64;
                    let block = self.block_at(wy, height);
                    if !block.is_air() {
                        chunk.set(LocalPos::new(lx, ly, lz), block);
                    }
                }
            }
        }
        // The chunk was built with set(), which marks it modified; but this
        // IS the canonical generated state, so clear the flag. Only later
        // edits should mark it modified (and thus needing a save).
        chunk.mark_unmodified();
        chunk
    }

    /// Build the 32x32 **heightfield** for a coarse LOD node: the surface
    /// height of each coarse column, sampled straight from the seed without
    /// generating any full-resolution chunks.
    ///
    /// Each cell takes the MINIMUM surface over its footprint, so coarse
    /// terrain never rises above the real surface — LOD underlaps the
    /// full-resolution region, and anything that overshoots pokes through real
    /// ground and is solid to walk into. Probing is exact up to 8 columns per
    /// axis, which covers every level fine enough to abut full-res.
    ///
    /// Feed the result to `vox_mesh::mesh_lod_heightfield`. Unlike the voxel
    /// path this preserves EXACT vertical detail — the reason distant terrain
    /// reads as terrain rather than stacked terraces.
    pub fn lod_heightfield(&self, origin_x: i64, origin_z: i64, h_stride: i64) -> Vec<i32> {
        debug_assert!(h_stride >= 1);
        let n = CHUNK_SIZE as i64;
        let probes = h_stride.min(8);
        let step = (h_stride / probes).max(1);
        let mut out = vec![0i32; (n * n) as usize];
        for cz in 0..n {
            for cx in 0..n {
                let bx = origin_x + cx * h_stride;
                let bz = origin_z + cz * h_stride;
                let mut h = i64::MAX;
                for pz in 0..probes {
                    for px in 0..probes {
                        h = h.min(self.surface_height(bx + px * step, bz + pz * step));
                    }
                }
                out[(cz * n + cx) as usize] = h as i32;
            }
        }
        out
    }

    /// The block at world height `wy` for a column whose surface is at
    /// `height`. The single source of truth for the vertical profile, used
    /// by both the per-column fill and any future queries.
    fn block_at(&self, wy: i64, height: i64) -> BlockId {
        if wy > height {
            blocks::AIR
        } else if wy == height {
            blocks::GRASS
        } else if wy >= height - DIRT_DEPTH {
            blocks::DIRT
        } else {
            blocks::STONE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vox_core::WorldPos;

    /// THE invariant: same seed + same position → byte-identical chunk.
    #[test]
    fn generation_is_deterministic() {
        let worldgen = Generator::new(0x0007_E22A_C0DE);
        let pos = ChunkPos::new(3, 0, -2);
        let a = worldgen.generate_chunk(pos);
        let b = worldgen.generate_chunk(pos);
        for p in LocalPos::iter() {
            assert_eq!(a.get(p), b.get(p), "nondeterministic at {p:?}");
        }
    }

    /// Different seeds should (almost always) produce different terrain.
    ///
    /// Compares SURFACE HEIGHTS across a wide spread rather than one chunk's
    /// blocks. Since M10 the world is mostly ocean thousands of blocks below
    /// Y = 0, so the chunk at the origin is usually empty air under every seed
    /// — a comparison that would pass or fail on where the coastline happened
    /// to fall rather than on whether the seeds differ.
    #[test]
    fn different_seeds_differ() {
        let a = Generator::new(1);
        let b = Generator::new(2);
        let differ = (0..64).any(|i| {
            let (x, z) = (i * 1_511, i * 977 - 30_000);
            a.surface_height(x, z) != b.surface_height(x, z)
        });
        assert!(differ, "two seeds produced identical terrain");
    }

    /// Per-chunk independence: a column's blocks must be identical whether
    /// read from a chunk at y=0 or the chunk stacked directly above it.
    /// This is the cubic-chunk seam guarantee.
    #[test]
    fn vertically_stacked_chunks_are_seamless() {
        let worldgen = Generator::new(42);
        let lower = worldgen.generate_chunk(ChunkPos::new(0, 0, 0));
        let upper = worldgen.generate_chunk(ChunkPos::new(0, 1, 0));

        // For a few columns, walk world Y across the seam and confirm the
        // surface/dirt/stone profile is continuous.
        for &(lx, lz) in &[(0u8, 0u8), (7, 19), (31, 31), (15, 3)] {
            let wx = lx as i64;
            let wz = lz as i64;
            let height = worldgen.surface_height(wx, wz);
            // Lower chunk covers wy 0..32, upper covers 32..64.
            for wy in 0..(2 * CHUNK_SIZE as i64) {
                let expected = if wy > height {
                    blocks::AIR
                } else if wy == height {
                    blocks::GRASS
                } else if wy >= height - DIRT_DEPTH {
                    blocks::DIRT
                } else {
                    blocks::STONE
                };
                let from_world = WorldPos::new(wx, wy, wz);
                let (chunk_pos, local) = from_world.split();
                let chunk = if chunk_pos.y == 0 { &lower } else { &upper };
                assert_eq!(
                    chunk.get(local),
                    expected,
                    "seam mismatch at column ({wx},{wz}) wy={wy}"
                );
            }
        }
    }

    /// Chunks far above any surface are all air and stored uniformly.
    #[test]
    fn high_chunks_are_uniform_air() {
        let worldgen = Generator::new(7);
        let chunk = worldgen.generate_chunk(ChunkPos::new(0, 100, 0)); // y 3200+
        assert!(chunk.is_all_air());
        assert!(chunk.is_uniform());
    }

    /// Chunks far below any surface are all stone and stored uniformly.
    #[test]
    fn deep_chunks_are_uniform_stone() {
        let worldgen = Generator::new(7);
        let chunk = worldgen.generate_chunk(ChunkPos::new(0, -100, 0)); // y -3200..
        assert!(chunk.is_uniform());
        // Confirm it's stone, not air.
        assert_eq!(chunk.get(LocalPos::new(0, 0, 0)), blocks::STONE);
    }

    /// Surface height is independent of which chunk asks for it (no
    /// chunk-local coordinate leaking into the noise).
    #[test]
    fn surface_height_is_global() {
        let worldgen = Generator::new(99);
        // Column at world x=32 is local x=0 of chunk 1 and "x=32" globally;
        // it must have one canonical height regardless.
        let h = worldgen.surface_height(32, 5);
        assert_eq!(h, worldgen.surface_height(32, 5));
        // Adjacent columns differ by small amounts (smoothness sanity).
        let h_next = worldgen.surface_height(33, 5);
        assert!((h - h_next).abs() <= 3, "terrain implausibly jagged");
    }

    /// Noise stays in range so heights are bounded and sane.
    #[test]
    fn noise_is_bounded() {
        let worldgen = Generator::new(123);
        for x in -100..100 {
            for z in (-100..100).step_by(7) {
                let n = worldgen.elevation().value_noise(x, z, 16);
                assert!((-1.0..=1.0).contains(&n), "noise out of range: {n}");
            }
        }
    }

    // ---- LOD heightfield generation (M09) ----

    const S: i64 = 8; // test horizontal stride

    /// Same seed + origin + stride -> identical heightfield, always.
    #[test]
    fn lod_heightfield_is_deterministic() {
        let g = Generator::new(0x0007_E22A_C0DE);
        assert_eq!(
            g.lod_heightfield(-256, 256, S),
            g.lod_heightfield(-256, 256, S)
        );
    }

    /// THE safety contract: the coarse height never exceeds the real surface
    /// anywhere in the cell. LOD underlaps full-res, so an overshoot pokes
    /// through real ground and is solid to walk into. Checks every real column.
    #[test]
    fn lod_heightfield_never_exceeds_real_terrain() {
        let g = Generator::new(42);
        let (ox, oz) = (0i64, 0i64);
        let hf = g.lod_heightfield(ox, oz, S);
        let n = CHUNK_SIZE as i64;
        for cz in 0..n {
            for cx in 0..n {
                let coarse = hf[(cz * n + cx) as usize] as i64;
                for pz in 0..S {
                    for px in 0..S {
                        let real = g.surface_height(ox + cx * S + px, oz + cz * S + pz);
                        assert!(coarse <= real, "coarse {coarse} above real {real}");
                    }
                }
            }
        }
    }

    /// The heightfield tracks real terrain closely (it is the minimum over the
    /// cell, not an arbitrary sample).
    #[test]
    fn lod_heightfield_tracks_the_surface() {
        let g = Generator::new(7);
        let hf = g.lod_heightfield(0, 0, 1); // stride 1: exact
        let n = CHUNK_SIZE as i64;
        for cz in 0..n {
            for cx in 0..n {
                assert_eq!(hf[(cz * n + cx) as usize] as i64, g.surface_height(cx, cz));
            }
        }
    }

    #[test]
    fn lod_heightfield_has_one_entry_per_cell() {
        let g = Generator::new(1);
        assert_eq!(g.lod_heightfield(0, 0, 4).len(), CHUNK_SIZE * CHUNK_SIZE);
    }
}

#[cfg(test)]
mod modflag_tests {
    use super::*;
    #[test]
    fn generated_chunks_are_unmodified() {
        let g = Generator::new(0x0007_E22A_C0DE);
        // Mixed (surface) chunk and uniform chunks alike must be unmodified.
        assert!(!g.generate_chunk(ChunkPos::new(0, 0, 0)).is_modified());
        assert!(!g.generate_chunk(ChunkPos::new(0, 100, 0)).is_modified()); // air
        assert!(!g.generate_chunk(ChunkPos::new(0, -100, 0)).is_modified()); // stone
    }
}
