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
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self { seed }
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
        // Placeholder terrain, tuned for LOD testing (M08 task 5, owner-
        // approved amendment): three octaves of value noise. The mountain
        // octave is CUBED — flats stay flat, extremes get pushed out — giving
        // real peaks (~+116) and valleys (~-64) instead of uniform rolling
        // hills. Range must stay inside the LOD node Y band
        // ([LOD_Y_ORIGIN_BLOCKS, +128) in vox-app) and adjacent-column slope
        // under the smoothness test's bound. Real geology replaces all of this
        // in a future milestone (ADR-0008 records the coarse-query constraint
        // it must preserve).
        let base = 26.0;
        let m = self.value_noise(wx, wz, 192);
        let mountains = m * m * m * 64.0; // cubed: dramatic peaks, flat plains
        let hills = self.value_noise(wx, wz, 48) * 20.0; // broad rolling hills
        let detail = self.value_noise(wx, wz, 12) * 6.0; // finer bumps
        (base + mountains + hills + detail).round() as i64
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

    /// Generate a coarse LOD node (M08, ADR-0008): one 32³ `Chunk` that stands
    /// in for a `(CHUNK_SIZE·stride)³`-block region of terrain, sampled directly
    /// from the seed WITHOUT generating the underlying full-res chunks. Each
    /// coarse cell `(cx,cy,cz)` represents the `stride³`-block cube starting at
    /// `origin + (cell·stride)`.
    ///
    /// `origin` is the world-space minimum corner of the region; the streamer
    /// aligns it to the node size (`CHUNK_SIZE·stride`) so nodes tile without
    /// gaps or overlap. The returned node meshes with the ordinary greedy mesher
    /// and renders through the ordinary chunk shader — an LOD node is just a
    /// scaled chunk (the whole point of ADR-0008).
    ///
    /// The surface is sampled once per coarse column (at the cell's centre), and
    /// the cube is filled up to it, **rounding down**: the topmost solid cube is
    /// the highest one lying entirely at or below the surface (GRASS; fully
    /// buried cubes below are STONE, cubes above are AIR). Coarse terrain thus
    /// never rises above the true surface — it hides beneath full-res chunks at
    /// the boundary instead of poking through them. Dirt is dropped at this
    /// scale (a 4-block band is invisible under 8-block cubes); coarse cell
    /// classification is deliberately simple and is revisited under real
    /// geology (ADR-0008 open question).
    ///
    /// Skylight is baked here rather than run through the (too-expensive-at-
    /// distance) 3D relight: LOD is a heightfield with no caves, so every AIR
    /// cell is sky-exposed and gets full skylight (15). The day/night `sky_scale`
    /// uniform then dims the node with the near field for free (ADR-0005/0007).
    pub fn generate_lod_node(&self, origin: vox_core::WorldPos, stride: i64) -> Chunk {
        debug_assert!(stride >= 1, "LOD stride must be >= 1");
        let mut chunk = Chunk::new_air();
        for cz in 0..CHUNK_SIZE as i64 {
            for cx in 0..CHUNK_SIZE as i64 {
                // Sample the full-res surface at this coarse column's centre.
                let wx = origin.x + cx * stride + stride / 2;
                let wz = origin.z + cz * stride + stride / 2;
                let height = self.surface_height(wx, wz);
                for cy in 0..CHUNK_SIZE as i64 {
                    let cell_bottom = origin.y + cy * stride;
                    let cell_top = cell_bottom + stride - 1;
                    // Round DOWN: a cube is solid only if it lies entirely at or
                    // below the surface (`cell_top <= height`). The coarse
                    // surface therefore never rises above the true terrain —
                    // essential at the full-res boundary, where LOD underlaps
                    // real chunks and must hide BENEATH them, never poke
                    // through. At distance this reads as terrain sitting up to
                    // `stride-1` blocks low, which is invisible; skirts cover
                    // the seams (ADR-0008).
                    let block = if cell_top > height {
                        blocks::AIR
                    } else if cell_top + stride > height {
                        blocks::GRASS // the topmost fully-buried cube
                    } else {
                        blocks::STONE
                    };
                    let lp = LocalPos::new(cx as u8, cy as u8, cz as u8);
                    if block.is_air() {
                        // Sky-exposed (heightfield, no caves at LOD): full sky.
                        chunk.set_sky_light(lp, 15);
                    } else {
                        chunk.set(lp, block);
                    }
                }
            }
        }
        // Canonical generated state — not a user edit.
        chunk.mark_unmodified();
        chunk
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

    /// Value noise in [-1, 1] at a lattice spacing of `cell` blocks.
    /// Hash the four surrounding lattice corners, smoothstep-interpolate.
    fn value_noise(&self, wx: i64, wz: i64, cell: i64) -> f32 {
        let x0 = wx.div_euclid(cell);
        let z0 = wz.div_euclid(cell);
        let fx = (wx.rem_euclid(cell)) as f32 / cell as f32;
        let fz = (wz.rem_euclid(cell)) as f32 / cell as f32;

        let c00 = self.lattice_value(x0, z0);
        let c10 = self.lattice_value(x0 + 1, z0);
        let c01 = self.lattice_value(x0, z0 + 1);
        let c11 = self.lattice_value(x0 + 1, z0 + 1);

        let sx = smoothstep(fx);
        let sz = smoothstep(fz);
        let top = lerp(c00, c10, sx);
        let bottom = lerp(c01, c11, sx);
        lerp(top, bottom, sz)
    }

    /// Deterministic hashed value in [-1, 1] for a lattice point.
    fn lattice_value(&self, lx: i64, lz: i64) -> f32 {
        let h = hash3(self.seed, lx as u64, lz as u64);
        // Map u64 → [-1, 1].
        (h as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0
    }
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Mix three u64s into a well-distributed hash (SplitMix-style finalizer
/// over a seeded combination). Deterministic and platform-independent.
fn hash3(seed: u64, a: u64, b: u64) -> u64 {
    let mut z = seed;
    for v in [a, b] {
        z = z.wrapping_add(v).wrapping_add(0x9E3779B97F4A7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
    }
    z
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
    #[test]
    fn different_seeds_differ() {
        let a = Generator::new(1).generate_chunk(ChunkPos::new(0, 0, 0));
        let b = Generator::new(2).generate_chunk(ChunkPos::new(0, 0, 0));
        let differ = LocalPos::iter().any(|p| a.get(p) != b.get(p));
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
                let n = worldgen.value_noise(x, z, 16);
                assert!((-1.0..=1.0).contains(&n), "noise out of range: {n}");
            }
        }
    }

    // ---- M08: coarse LOD node generation (ADR-0008) ----

    const S: i64 = 8; // test stride
    const NODE: i64 = CHUNK_SIZE as i64 * S; // node covers NODE³ blocks

    /// Same seed + node origin + stride → byte-identical node, always.
    #[test]
    fn lod_node_is_deterministic() {
        let g = Generator::new(0x0007_E22A_C0DE);
        let origin = WorldPos::new(-NODE, 0, NODE);
        let a = g.generate_lod_node(origin, S);
        let b = g.generate_lod_node(origin, S);
        for p in LocalPos::iter() {
            assert_eq!(a.get(p), b.get(p), "block mismatch at {p:?}");
            assert_eq!(a.sky_light(p), b.sky_light(p), "sky mismatch at {p:?}");
        }
    }

    /// Round-down contract: the topmost solid cube lies entirely at or below
    /// the true surface, and within one stride of it — so coarse terrain never
    /// rises above real terrain (boundary safety) and never sinks more than
    /// `stride-1` blocks (visual fidelity).
    #[test]
    fn lod_node_surface_cube_matches_heightmap() {
        let g = Generator::new(42);
        // Node whose Y range straddles the surface near y≈0.
        let origin = WorldPos::new(0, -NODE / 2, 0);
        let node = g.generate_lod_node(origin, S);
        for cz in 0..CHUNK_SIZE as i64 {
            for cx in 0..CHUNK_SIZE as i64 {
                let wx = origin.x + cx * S + S / 2;
                let wz = origin.z + cz * S + S / 2;
                let height = g.surface_height(wx, wz);
                // Find the topmost solid cube in this column.
                let mut top_solid: Option<i64> = None;
                for cy in (0..CHUNK_SIZE as i64).rev() {
                    let lp = LocalPos::new(cx as u8, cy as u8, cz as u8);
                    if !node.get(lp).is_air() {
                        top_solid = Some(cy);
                        break;
                    }
                }
                // Only assert for columns whose surface falls inside this node.
                if height >= origin.y + S && height < origin.y + NODE - S {
                    let cy = top_solid.expect("surface inside node → a solid cube");
                    let cube_top = origin.y + cy * S + S - 1;
                    assert!(
                        cube_top <= height,
                        "coarse surface (cube top {cube_top}) above true surface {height}"
                    );
                    assert!(
                        height - cube_top < S,
                        "coarse surface {cube_top} more than a stride below {height}"
                    );
                    assert_eq!(
                        node.get(LocalPos::new(cx as u8, cy as u8, cz as u8)),
                        blocks::GRASS,
                        "topmost solid cube should be grass"
                    );
                }
            }
        }
    }

    /// Air cubes are sky-exposed (15); solid cubes carry no sky light. This is
    /// what lets the node light itself without the 3D relight pipeline.
    #[test]
    fn lod_node_air_is_full_skylight_solid_is_dark() {
        let g = Generator::new(7);
        let origin = WorldPos::new(0, -NODE / 2, 0);
        let node = g.generate_lod_node(origin, S);
        for p in LocalPos::iter() {
            if node.get(p).is_air() {
                assert_eq!(node.sky_light(p), 15, "air cube must be full skylight");
            } else {
                assert_eq!(node.sky_light(p), 0, "solid cube must carry no skylight");
            }
        }
    }

    /// A node far above any terrain is all air (and fully sky-lit); a node far
    /// below is all stone (and dark).
    #[test]
    fn lod_node_uniform_extremes() {
        let g = Generator::new(123);
        let high = g.generate_lod_node(WorldPos::new(0, 100 * NODE, 0), S);
        assert!(LocalPos::iter().all(|p| high.get(p).is_air()));
        assert!(LocalPos::iter().all(|p| high.sky_light(p) == 15));

        let low = g.generate_lod_node(WorldPos::new(0, -100 * NODE, 0), S);
        assert!(LocalPos::iter().all(|p| low.get(p) == blocks::STONE));
    }

    /// Generated nodes are canonical state, not user edits.
    #[test]
    fn lod_node_is_unmodified() {
        let g = Generator::new(1);
        assert!(!g.generate_lod_node(WorldPos::new(0, 0, 0), S).is_modified());
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
