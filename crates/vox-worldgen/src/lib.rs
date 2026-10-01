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
use vox_core::{BlockId, CHUNK_SIZE, Chunk, ChunkPos, LocalPos, SEA_LEVEL_BLOCKS, WorldShape};

/// Block ids used by the placeholder generator. These now come from the
/// canonical block registry in vox-core (Milestone 03); re-exported here so
/// existing call sites (`blocks::STONE`, etc.) keep working unchanged.
pub mod blocks {
    pub use vox_core::registry::{AIR, DIRT, GRASS, STONE, WATER};
}

/// Number of dirt blocks below the surface grass layer.
const DIRT_DEPTH: i64 = 4;

/// How densely [`Generator::lod_heightfield`] samples each coarse cell.
///
/// The choice is the caller's, because it depends on where the node sits
/// relative to full-resolution terrain, which the generator cannot know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodSampling {
    /// Every column: the cell never rises above the real surface. For levels
    /// that can overlap full-resolution terrain.
    Exact,
    /// A 4x4 grid per cell: cheap at any stride, may sit slightly above a
    /// narrow dip. For levels that never meet full-resolution terrain.
    Sparse,
}

/// Samples per axis of a sparse LOD cell. Chosen so a node costs ~16 000
/// `surface_height` calls at any stride — the M10 audit measured ~3 ms.
const LOD_SPARSE_PROBES: i64 = 4;

/// Terrain generator for one world seed.
#[derive(Clone, Copy, Debug)]
pub struct Generator {
    seed: u64,
    shape: WorldShape,
    elevation: Elevation,
}

/// Version of the terrain this generator produces for a given seed and world
/// size. Recorded in `world.meta`; a world made by a different version is
/// refused rather than opened with its edited chunks stranded in new terrain.
///
/// **Bump this whenever terrain output changes** — any change to the elevation
/// field's constants, stages or noise. `terrain_fingerprint_is_pinned` fails
/// when output changes, so a change cannot slip through unversioned: it forces
/// a decision to bump this and re-pin the fingerprint in the same commit.
///
/// History: 1 — the M10 torus (ADR-0012), the first version recorded.
/// 2 — sea water fills every block above the ground up to sea level (M10 A1,
/// ADR-0011). Surface heights are unchanged; chunk contents are not, which is
/// why the fingerprint now covers chunks as well as heights.
pub const GENERATOR_VERSION: u32 = 2;

impl Generator {
    /// A generator for one world: its seed and its size. The size is part of
    /// the terrain, not a detail — noise tiles with the world's period
    /// (ADR-0012), so the same seed makes different ground at different sizes.
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        Self {
            seed,
            shape,
            elevation: Elevation::new(seed, shape),
        }
    }

    /// The size of the world this generator makes.
    pub fn shape(&self) -> WorldShape {
        self.shape
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
    /// column (wx, wz), which may be on any lap of the world. See
    /// [`Elevation::height`].
    ///
    /// Independent of Y and of chunk boundaries — two chunks stacked
    /// vertically compute the same surface for the same column, which is
    /// what keeps cubic chunks seamless.
    pub fn surface_height(&self, wx: i64, wz: i64) -> i64 {
        self.elevation.height(wx, wz)
    }

    /// The inclusive chunk-Y span of the terrain surface across one chunk
    /// column `(cx, cz)` — the layers that streaming keeps around the ground.
    ///
    /// Sampled at the column's four corners and its centre rather than one
    /// point: a 32-block-wide column can hold a cliff, and a single sample
    /// would load its top and leave a hole down the face. Five samples, not
    /// 1 024, because the streamer calls this for every column in its disc
    /// every frame; the margins around the span absorb what they miss.
    ///
    /// The one sampler: the streamer, the first-mesh gate and LOD coverage
    /// all judge residency from it, and they must agree to the layer.
    pub fn surface_span_chunks(&self, cx: i64, cz: i64) -> (i64, i64) {
        let s = CHUNK_SIZE as i64;
        let (bx, bz) = (cx * s, cz * s);
        let e = s - 1;
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        for (ox, oz) in [(0, 0), (e, 0), (0, e), (e, e), (e / 2, e / 2)] {
            let h = self.surface_height(bx + ox, bz + oz);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        (lo.div_euclid(s), hi.div_euclid(s))
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

        // Wholly above the ground: sky, sea, or the sea surface between them.
        if chunk_min_y > max_surface {
            if chunk_min_y > SEA_LEVEL_BLOCKS {
                return Chunk::filled(blocks::AIR);
            }
            if chunk_max_y <= SEA_LEVEL_BLOCKS {
                // Open ocean between the seabed and the surface — most of an
                // ocean's volume, generated without a single height query more.
                return Chunk::filled(blocks::WATER);
            }
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
    /// Each cell takes the MINIMUM of the surface heights it samples, so it
    /// errs downward. How many it samples is `sampling`:
    ///
    /// - [`LodSampling::Exact`] reads every column of the cell, so the cell
    ///   never rises above the real surface anywhere in it. Required where LOD
    ///   can overlap full-resolution terrain, which it underlaps: an overshoot
    ///   there pokes up through real ground (ADR-0008's never-exceed contract).
    /// - [`LodSampling::Sparse`] reads a 4x4 grid, 16 columns per cell at any
    ///   stride, so a node costs the same at stride 32 as at stride 4. It can
    ///   sit a little above a narrow dip it missed — invisible from a level
    ///   that never meets full-res, and the reason distant levels are cheap
    ///   (ADR-0008, M10 amendment). At strides of 4 or less it reads every
    ///   column anyway and equals `Exact`.
    ///
    /// Feed the result to `vox_mesh::mesh_lod_heightfield`. Unlike the voxel
    /// path this preserves EXACT vertical detail — the reason distant terrain
    /// reads as terrain rather than stacked terraces.
    pub fn lod_heightfield(
        &self,
        origin_x: i64,
        origin_z: i64,
        h_stride: i64,
        sampling: LodSampling,
    ) -> Vec<i32> {
        debug_assert!(h_stride >= 1);
        let n = CHUNK_SIZE as i64;
        let probes = match sampling {
            LodSampling::Exact => h_stride,
            LodSampling::Sparse => h_stride.min(LOD_SPARSE_PROBES),
        };
        let step = h_stride / probes;
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
            // Above the ground: the sea fills up to sea level (ADR-0011).
            if wy <= SEA_LEVEL_BLOCKS {
                blocks::WATER
            } else {
                blocks::AIR
            }
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
        let worldgen = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
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
        let a = Generator::new(1, WorldShape::DEFAULT);
        let b = Generator::new(2, WorldShape::DEFAULT);
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
        let worldgen = Generator::new(42, WorldShape::DEFAULT);
        let lower = worldgen.generate_chunk(ChunkPos::new(0, 0, 0));
        let upper = worldgen.generate_chunk(ChunkPos::new(0, 1, 0));

        // For a few columns, walk world Y across the seam and confirm both
        // chunks follow the one vertical profile, `block_at` — compared
        // against it rather than a second copy of it, which is what went stale
        // when the sea arrived.
        for &(lx, lz) in &[(0u8, 0u8), (7, 19), (31, 31), (15, 3)] {
            let wx = lx as i64;
            let wz = lz as i64;
            let height = worldgen.surface_height(wx, wz);
            // Lower chunk covers wy 0..32, upper covers 32..64.
            for wy in 0..(2 * CHUNK_SIZE as i64) {
                let expected = worldgen.block_at(wy, height);
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

    /// A column whose ground is at least `depth` below sea level, found by
    /// walking a line — oceans cover most of the world, so this is short.
    fn ocean_column(g: &Generator, depth: i64) -> (i64, i64) {
        (0..200_000i64)
            .step_by(97)
            .map(|x| (x, 5_000))
            .find(|&(x, z)| g.surface_height(x, z) < SEA_LEVEL_BLOCKS - depth)
            .expect("no ocean that deep along the search line")
    }

    /// THE point of version 2: the sea fills every block above the ground up
    /// to sea level, and not one block higher.
    #[test]
    fn the_sea_fills_to_sea_level_and_no_higher() {
        let g = Generator::new(42, WorldShape::DEFAULT);
        let (x, z) = ocean_column(&g, 100);
        let ground = g.surface_height(x, z);
        let block = |y: i64| {
            let (chunk, local) = vox_core::WorldPos::new(x, y, z).split();
            g.generate_chunk(chunk).get(local)
        };
        assert_ne!(block(ground), blocks::WATER, "the seabed itself is water");
        for y in [
            ground + 1,
            ground + 40,
            SEA_LEVEL_BLOCKS - 1,
            SEA_LEVEL_BLOCKS,
        ] {
            assert_eq!(block(y), blocks::WATER, "no water at {y} (ground {ground})");
        }
        assert_eq!(
            block(SEA_LEVEL_BLOCKS + 1),
            blocks::AIR,
            "the sea overflows"
        );
    }

    /// Dry land keeps no water: the fill is above the GROUND, below sea level
    /// — a column above sea level has none, even in the chunk holding the sea
    /// surface elsewhere.
    #[test]
    fn land_above_sea_level_stays_dry() {
        let g = Generator::new(42, WorldShape::DEFAULT);
        let (x, z) = (0..200_000i64)
            .step_by(97)
            .map(|x| (x, 5_000))
            .find(|&(x, z)| g.surface_height(x, z) > SEA_LEVEL_BLOCKS + 10)
            .expect("no land along the search line");
        let ground = g.surface_height(x, z);
        for y in ground - 30..=ground + 30 {
            assert_ne!(
                g.block_at(y, ground),
                blocks::WATER,
                "water at {y} on dry land"
            );
        }
    }

    /// Every fast path agrees with the per-block profile: an all-water chunk
    /// of open ocean, the sea-surface chunk above it, and the air above that.
    #[test]
    fn ocean_chunks_match_the_profile_through_every_fast_path() {
        let g = Generator::new(42, WorldShape::DEFAULT);
        let s = CHUNK_SIZE as i64;
        let (x, z) = ocean_column(&g, 150);
        let (cx, cz) = (x.div_euclid(s), z.div_euclid(s));
        let open_ocean = ChunkPos::new(cx, vox_core::SEA_SURFACE_CHUNK_Y - 2, cz);
        assert!(
            g.generate_chunk(open_ocean).is_uniform(),
            "setup: expected the all-water fast path"
        );
        for cy in [
            vox_core::SEA_SURFACE_CHUNK_Y - 2,
            vox_core::SEA_SURFACE_CHUNK_Y,
            vox_core::SEA_SURFACE_CHUNK_Y + 1,
        ] {
            let pos = ChunkPos::new(cx, cy, cz);
            let chunk = g.generate_chunk(pos);
            for local in LocalPos::iter() {
                let (wx, wy, wz) = (
                    pos.x * s + local.x() as i64,
                    pos.y * s + local.y() as i64,
                    pos.z * s + local.z() as i64,
                );
                assert_eq!(
                    chunk.get(local),
                    g.block_at(wy, g.surface_height(wx, wz)),
                    "chunk {pos:?} disagrees with the profile at ({wx}, {wy}, {wz})"
                );
            }
        }
    }

    /// Chunks far above any surface are all air and stored uniformly.
    #[test]
    fn high_chunks_are_uniform_air() {
        let worldgen = Generator::new(7, WorldShape::DEFAULT);
        let chunk = worldgen.generate_chunk(ChunkPos::new(0, 100, 0)); // y 3200+
        assert!(chunk.is_all_air());
        assert!(chunk.is_uniform());
    }

    /// Chunks far below any surface are all stone and stored uniformly.
    #[test]
    fn deep_chunks_are_uniform_stone() {
        let worldgen = Generator::new(7, WorldShape::DEFAULT);
        let chunk = worldgen.generate_chunk(ChunkPos::new(0, -100, 0)); // y -3200..
        assert!(chunk.is_uniform());
        // Confirm it's stone, not air.
        assert_eq!(chunk.get(LocalPos::new(0, 0, 0)), blocks::STONE);
    }

    /// Surface height is independent of which chunk asks for it (no
    /// chunk-local coordinate leaking into the noise).
    #[test]
    fn surface_height_is_global() {
        let worldgen = Generator::new(99, WorldShape::DEFAULT);
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
    fn stages_stay_in_their_ranges() {
        let e = *Generator::new(123, WorldShape::DEFAULT).elevation();
        for x in (-4_000..4_000).step_by(37) {
            for z in (-4_000..4_000).step_by(211) {
                for v in [e.continent(x, z), e.detail(x, z)] {
                    assert!((-1.0..=1.0).contains(&v), "signed stage out of range: {v}");
                }
                for v in [e.relief(x, z), e.orogeny(x, z)] {
                    assert!((0.0..=1.0).contains(&v), "unit stage out of range: {v}");
                }
            }
        }
    }

    /// Terrain sampled at fixed places, folded into one number.
    fn terrain_fingerprint() -> u64 {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        let mut h = 0xCBF2_9CE4_8422_2325u64; // FNV-1a
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ b as u64).wrapping_mul(0x0100_0000_01B3);
            }
        };
        for i in 0..512i64 {
            let (x, z) = (i * 7_919 - 1_000_000, i * 104_729 + 3);
            eat(&g.surface_height(x, z).to_le_bytes());
        }
        // Chunk contents too: what fills the space around the surface can
        // change without moving a single height — the sea did (version 2).
        // Several layers around the surface of scattered columns, land and
        // ocean alike, catch the fill rules and every fast path.
        let s = CHUNK_SIZE as i64;
        for i in 0..24i64 {
            let (cx, cz) = (i * 1_987 - 20_000, i * 3_571 + 11);
            let surface = g.surface_height(cx * s, cz * s).div_euclid(s);
            let top = surface.max(vox_core::SEA_SURFACE_CHUNK_Y);
            for cy in surface - 1..=top + 1 {
                let chunk = g.generate_chunk(ChunkPos::new(cx, cy, cz));
                for pos in LocalPos::iter() {
                    eat(&chunk.get(pos).0.to_le_bytes());
                }
            }
        }
        h
    }

    /// **Terrain output is pinned to [`GENERATOR_VERSION`].**
    ///
    /// Saves record the generator version so a world is never reopened under
    /// different terrain. That only works if the version actually changes when
    /// the terrain does. This fails on ANY change to terrain output, forcing the
    /// person making it to bump the version and re-pin in the same commit.
    ///
    /// It is also a determinism check: the same code must produce the same
    /// terrain on every machine. If this fails on one platform and passes on
    /// another with no code change, that is a real bug, not a stale pin.
    #[test]
    fn terrain_fingerprint_is_pinned() {
        const PINNED: (u32, u64) = (2, 0x14c7_a1a5_93b3_debf);
        let got = terrain_fingerprint();
        assert_eq!(
            (GENERATOR_VERSION, got),
            PINNED,
            "terrain output changed. If intended, bump GENERATOR_VERSION and set \
             PINNED to (new version, {got:#018x}); existing worlds will then be refused \
             rather than opened with mismatched terrain."
        );
    }

    // ---- LOD heightfield generation (M09) ----

    const S: i64 = 8; // test horizontal stride

    /// Same seed + origin + stride -> identical heightfield, always.
    #[test]
    fn lod_heightfield_is_deterministic() {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        assert_eq!(
            g.lod_heightfield(-256, 256, S, LodSampling::Sparse),
            g.lod_heightfield(-256, 256, S, LodSampling::Sparse)
        );
    }

    /// THE safety contract, where it applies: an EXACT heightfield never
    /// exceeds the real surface anywhere in the cell. LOD underlaps full-res,
    /// so an overshoot pokes through real ground. Checks every real column, at
    /// a stride the old sampler (8 probes per axis) got wrong.
    #[test]
    fn exact_lod_heightfield_never_exceeds_real_terrain() {
        let g = Generator::new(42, WorldShape::DEFAULT);
        let n = CHUNK_SIZE as i64;
        for stride in [S, 16] {
            let (ox, oz) = (-(n * stride) / 2, 4096i64);
            let hf = g.lod_heightfield(ox, oz, stride, LodSampling::Exact);
            for cz in 0..n {
                for cx in 0..n {
                    let coarse = hf[(cz * n + cx) as usize] as i64;
                    for pz in 0..stride {
                        for px in 0..stride {
                            let real =
                                g.surface_height(ox + cx * stride + px, oz + cz * stride + pz);
                            assert!(
                                coarse <= real,
                                "stride {stride}: coarse {coarse} above real {real}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Sparse samples a subset of what exact does, so its minimum can only be
    /// equal or higher — never lower. And at strides of 4 or less the subset
    /// is everything, so the two agree exactly.
    #[test]
    fn sparse_is_exact_at_fine_strides_and_never_below_it_beyond() {
        let g = Generator::new(9, WorldShape::DEFAULT);
        for stride in [1, 2, 4] {
            assert_eq!(
                g.lod_heightfield(640, -320, stride, LodSampling::Sparse),
                g.lod_heightfield(640, -320, stride, LodSampling::Exact),
                "stride {stride}"
            );
        }
        for stride in [8, 16, 32] {
            let sparse = g.lod_heightfield(640, -320, stride, LodSampling::Sparse);
            let exact = g.lod_heightfield(640, -320, stride, LodSampling::Exact);
            for (s, e) in sparse.iter().zip(&exact) {
                assert!(s >= e, "stride {stride}: sparse {s} below exact {e}");
            }
        }
    }

    /// The heightfield tracks real terrain closely (it is the minimum over the
    /// cell, not an arbitrary sample).
    #[test]
    fn lod_heightfield_tracks_the_surface() {
        let g = Generator::new(7, WorldShape::DEFAULT);
        let hf = g.lod_heightfield(0, 0, 1, LodSampling::Sparse); // stride 1: every column
        let n = CHUNK_SIZE as i64;
        for cz in 0..n {
            for cx in 0..n {
                assert_eq!(hf[(cz * n + cx) as usize] as i64, g.surface_height(cx, cz));
            }
        }
    }

    #[test]
    fn lod_heightfield_has_one_entry_per_cell() {
        let g = Generator::new(1, WorldShape::DEFAULT);
        assert_eq!(
            g.lod_heightfield(0, 0, 4, LodSampling::Sparse).len(),
            CHUNK_SIZE * CHUNK_SIZE
        );
    }

    /// The span covers the surface wherever it samples it — every corner and
    /// the centre of the column — in negative columns too, where the chunk
    /// layer of a height needs floor division.
    #[test]
    fn surface_span_covers_the_sampled_surface() {
        let g = Generator::new(11, WorldShape::DEFAULT);
        let s = CHUNK_SIZE as i64;
        for (cx, cz) in [(0, 0), (-1, -1), (37, -12), (-400, 250), (5_000, 3)] {
            let (lo, hi) = g.surface_span_chunks(cx, cz);
            assert!(lo <= hi);
            let e = s - 1;
            for (ox, oz) in [(0, 0), (e, 0), (0, e), (e, e), (e / 2, e / 2)] {
                let layer = g.surface_height(cx * s + ox, cz * s + oz).div_euclid(s);
                assert!(
                    (lo..=hi).contains(&layer),
                    "column ({cx}, {cz}): surface layer {layer} outside span ({lo}, {hi})"
                );
            }
        }
    }
}

#[cfg(test)]
mod modflag_tests {
    use super::*;
    #[test]
    fn generated_chunks_are_unmodified() {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        // Mixed (surface) chunk and uniform chunks alike must be unmodified.
        assert!(!g.generate_chunk(ChunkPos::new(0, 0, 0)).is_modified());
        assert!(!g.generate_chunk(ChunkPos::new(0, 100, 0)).is_modified()); // air
        assert!(!g.generate_chunk(ChunkPos::new(0, -100, 0)).is_modified()); // stone
    }
}
