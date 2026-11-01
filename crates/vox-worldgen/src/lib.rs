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

pub mod biome;
pub mod climate;
pub mod drainage;
pub mod elevation;
#[cfg(test)]
mod map;
pub mod materials;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use biome::Column;
use climate::ClimateMap;
use drainage::{Body, DrainageMap, Water};
use elevation::Elevation;
use vox_core::{BlockId, CHUNK_SIZE, Chunk, ChunkPos, LocalPos, SEA_LEVEL_BLOCKS, WorldShape};

/// Block ids used by the placeholder generator. These now come from the
/// canonical block registry in vox-core (Milestone 03); re-exported here so
/// existing call sites (`blocks::STONE`, etc.) keep working unchanged.
pub mod blocks {
    pub use vox_core::registry::{AIR, DIRT, GRASS, STONE, WATER};
}

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
///
/// `Clone`, cheaply: the stage-1 climate map is shared behind an `Arc` and
/// built on first use (ADR-0017), so a generator used only for heights never
/// pays for it.
#[derive(Clone, Debug)]
pub struct Generator {
    seed: u64,
    shape: WorldShape,
    elevation: Elevation,
    climate: Arc<OnceLock<ClimateMap>>,
    drainage: Arc<OnceLock<DrainageMap>>,
}

/// The LOD's mark for a dry cell (`vox_mesh::LOD_DRY`; vox-worldgen does not
/// depend on vox-mesh).
const fn vox_mesh_dry() -> i32 {
    i32::MIN
}

/// The top of a column's standing water, or `i64::MIN` where it has none.
fn wet_top(w: &Water) -> i64 {
    match w.body {
        Body::River(_) | Body::Lake | Body::SaltLake => w.level,
        Body::Dry | Body::SaltFlat => i64::MIN,
    }
}

/// The drainage network's cache, in the world folder (ADR-0019 §6).
pub const DRAINAGE_CACHE_FILE: &str = "drainage.cache";

/// Where [`Generator::drainage_cached`] got the network.
#[derive(Debug)]
pub enum DrainageSource {
    /// Read from the world folder's cache.
    Cache,
    /// Built, and the cache written.
    Built,
    /// Built, but the cache could not be written.
    BuiltUnsaved(std::io::Error),
    /// This generator already had it.
    AlreadyBuilt,
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
/// 3 — climate and biomes (M13): every column's surface materials come from
/// its biome (`materials`), polar seas freeze at the surface, and small
/// worlds short of mountain ranges widen their belts (elevation's
/// `calibrate_ranges`; worlds of the default size and larger keep their
/// heights exactly).
/// 4 — valleys (M14 task 2): the surface is cut by the erosion stage, river
/// valleys carved by stream power from the drainage network.
pub const GENERATOR_VERSION: u32 = 4;

impl Generator {
    /// A generator for one world: its seed and its size. The size is part of
    /// the terrain, not a detail — noise tiles with the world's period
    /// (ADR-0012), so the same seed makes different ground at different sizes.
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        Self {
            seed,
            shape,
            elevation: Elevation::new(seed, shape),
            climate: Arc::new(OnceLock::new()),
            drainage: Arc::new(OnceLock::new()),
        }
    }

    /// The world's stage-1 climate map, built on first use (~0.5 s at the
    /// default size) and shared by every clone of this generator. Call it
    /// once at world load so the cost is paid there, not by the first chunk.
    pub fn climate(&self) -> &ClimateMap {
        self.climate
            .get_or_init(|| ClimateMap::build(&self.elevation, self.shape))
    }

    /// The world's drainage network and erosion stage (M14), built on first
    /// use after the climate it needs (a few seconds at the default size)
    /// and shared by every clone. Call it once at world load, with
    /// [`Self::climate`], so the cost is paid there.
    pub fn drainage(&self) -> &DrainageMap {
        self.drainage
            .get_or_init(|| DrainageMap::build(&self.elevation, self.climate(), self.shape))
    }

    /// [`Self::drainage`], read from the world folder's cache when it holds
    /// this world's network (ADR-0019 §6), else built and written there for
    /// next time. A cache that is missing, stale or damaged costs a rebuild,
    /// never wrong terrain; one that cannot be written costs the next load a
    /// rebuild, nothing more.
    pub fn drainage_cached(&self, folder: &Path) -> (&DrainageMap, DrainageSource) {
        let path = folder.join(DRAINAGE_CACHE_FILE);
        let key = drainage::CacheKey {
            seed: self.seed,
            shape: self.shape,
            generator_version: GENERATOR_VERSION,
        };
        let mut source = DrainageSource::AlreadyBuilt;
        let map = self.drainage.get_or_init(|| {
            if let Ok(bytes) = std::fs::read(&path)
                && let Some(map) = DrainageMap::from_cache_bytes(&bytes, key)
            {
                source = DrainageSource::Cache;
                return map;
            }
            let map = DrainageMap::build(&self.elevation, self.climate(), self.shape);
            let tmp = path.with_extension("tmp");
            source = match std::fs::write(&tmp, map.to_cache_bytes(key))
                .and_then(|()| std::fs::rename(&tmp, &path))
            {
                Ok(()) => DrainageSource::Built,
                Err(e) => DrainageSource::BuiltUnsaved(e),
            };
            map
        });
        (map, source)
    }

    /// The surface at a column: the elevation field's, cut by the erosion
    /// stage (M14). THE surface: chunks, the LOD and every readout take it
    /// from here; climate alone reads the uneroded base (ADR-0017).
    pub fn surface(&self, wx: i64, wz: i64) -> elevation::Surface {
        self.ground(wx, wz).0
    }

    /// The surface and the water on it (M14): the eroded ground, its river
    /// channels cut and their banks shaped, and the river, lake or salt
    /// flat the column holds.
    pub fn ground(&self, wx: i64, wz: i64) -> (elevation::Surface, Water) {
        let mut s = self.eroded_surface(wx, wz);
        let (h, water) = self.drainage().shape_column(wx, wz, s.height, |x, z, l| {
            self.elevation.meander_warp(x, z, l)
        });
        s.height = h;
        (s, water)
    }

    /// The elevation field cut by the erosion stage, before rivers shape
    /// it: the ground the drainage network was built on.
    pub fn eroded_surface(&self, wx: i64, wz: i64) -> elevation::Surface {
        let mut s = self.elevation.surface(wx, wz);
        let cut = self.drainage().erosion_at(wx as f64, wz as f64);
        s.height += cut.round() as i64;
        s
    }

    /// A column's surface, climate and biome (M13): the single place they
    /// are put together, so the chunk generator, the LOD sampler and the
    /// player's readout agree on every column.
    pub fn column(&self, wx: i64, wz: i64) -> Column {
        self.column_with(wx, wz, self.ground(wx, wz))
    }

    /// [`Self::column`] for a surface and water already computed.
    fn column_with(
        &self,
        wx: i64,
        wz: i64,
        (surface, water): (elevation::Surface, Water),
    ) -> Column {
        let (sea_level_c, precip) = self.climate().sample(wx, wz);
        let mut col = biome::classify(
            surface,
            sea_level_c,
            precip,
            self.elevation.boundary_noise(wx, wz),
        );
        col.patch = self.elevation.patch_noise(wx, wz);
        col.water = water;
        col
    }

    /// The block at a world position, as generated: THE per-block profile,
    /// which every chunk path must reproduce (tests compare against it).
    pub fn block_at(&self, wx: i64, wy: i64, wz: i64) -> BlockId {
        self.block_in(&self.column(wx, wz), wy)
    }

    /// The block at height `wy` of a classified column.
    fn block_in(&self, col: &Column, wy: i64) -> BlockId {
        materials::block_in(col, wy, |y| materials::stratum_block(self.seed, y))
    }

    /// The size of the world this generator makes.
    pub fn shape(&self) -> WorldShape {
        self.shape
    }

    /// The base elevation field, before the erosion stage
    /// [`Self::surface`] adds. Exposed so callers can query individual
    /// stages (debug views, tuning, climate) without duplicating the
    /// composition.
    pub fn elevation(&self) -> &Elevation {
        &self.elevation
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Surface height (the Y of the topmost solid/grass block) at world
    /// column (wx, wz), which may be on any lap of the world. See
    /// [`Self::surface`].
    ///
    /// Independent of Y and of chunk boundaries — two chunks stacked
    /// vertically compute the same surface for the same column, which is
    /// what keeps cubic chunks seamless.
    pub fn surface_height(&self, wx: i64, wz: i64) -> i64 {
        self.surface(wx, wz).height
    }

    /// The inclusive chunk-Y span of the terrain surface across one chunk
    /// column `(cx, cz)` — the layers that streaming keeps around the ground
    /// and the rivers and lakes standing on it.
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
            // Standing water counts as surface (M14): a lake's surface is
            // tens of blocks above its bed, in layers the ground alone would
            // never load.
            let (s, w) = self.ground(bx + ox, bz + oz);
            lo = lo.min(s.height);
            hi = hi.max(s.height).max(wet_top(&w));
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
        // If the whole chunk is above the highest surface, it's all air (or
        // sea); if wholly below every column's surface profile, all stone.
        let n = CHUNK_SIZE as i64;
        let mut surfaces = Vec::with_capacity((n * n) as usize);
        let mut min_surface = i64::MAX;
        let mut max_surface = i64::MIN;
        for lz in 0..n {
            for lx in 0..n {
                let g = self.ground(origin.x + lx, origin.z + lz);
                min_surface = min_surface.min(g.0.height);
                // Rivers and lakes stand above the ground: the chunk is not
                // all air below their surface.
                max_surface = max_surface.max(g.0.height).max(wet_top(&g.1));
                surfaces.push(g);
            }
        }

        // Wholly above the ground: sky, or open sea below its surface layer
        // (which can freeze, so the chunk holding it is never uniform).
        if chunk_min_y > max_surface {
            if chunk_min_y > SEA_LEVEL_BLOCKS {
                return Chunk::filled(blocks::AIR);
            }
            if chunk_max_y < SEA_LEVEL_BLOCKS {
                // Open ocean between the seabed and the surface — most of an
                // ocean's volume, generated without a single column classified.
                return Chunk::filled(blocks::WATER);
            }
        }
        // Wholly below every surface profile: stone (`materials`' promise).
        if chunk_max_y < min_surface - materials::MAX_PROFILE_DEPTH {
            return Chunk::filled(blocks::STONE);
        }

        // Mixed chunk: fill per column, classifying a column only where this
        // chunk meets its profile or its sea surface. Elsewhere it is stone
        // below the profile and air or water above, whatever the biome.
        let mut chunk = Chunk::new_air();
        for lz in 0..CHUNK_SIZE as u8 {
            for lx in 0..CHUNK_SIZE as u8 {
                let g = surfaces[lz as usize * CHUNK_SIZE + lx as usize];
                let (wx, wz) = (origin.x + lx as i64, origin.z + lz as i64);
                let h = g.0.height;
                let profile_lo = h - materials::MAX_PROFILE_DEPTH + 1;
                let touches_profile = chunk_max_y >= profile_lo && chunk_min_y <= h;
                let touches_sea_surface =
                    h < SEA_LEVEL_BLOCKS && (chunk_min_y..=chunk_max_y).contains(&SEA_LEVEL_BLOCKS);
                let touches_water =
                    wet_top(&g.1) > h && chunk_max_y > h && chunk_min_y <= g.1.level;
                let col = (touches_profile || touches_sea_surface || touches_water)
                    .then(|| self.column_with(wx, wz, g));
                for ly in 0..CHUNK_SIZE as u8 {
                    let wy = chunk_min_y + ly as i64;
                    let block = match &col {
                        Some(c) => self.block_in(c, wy),
                        None if wy < profile_lo => blocks::STONE,
                        None if wy <= SEA_LEVEL_BLOCKS => blocks::WATER,
                        None => blocks::AIR,
                    };
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
    /// [`Self::lod_heightfield`] with each cell's top block (M13), for
    /// `vox_mesh::mesh_lod_cells`: what full resolution shows at the column
    /// that sets the cell's height — its lowest sample — classified exactly
    /// as the chunk generator classifies it, so a distant desert is sand, an
    /// ice cap snow, a seabed mud, sand or gravel.
    ///
    /// A polar sea's cell rises to sea level as ice: ice is the solid top
    /// there (ADR-0011's `solid` is the terrain top the LOD draws), and with
    /// it the LOD sea surface stops at the ice edge as the near sea does.
    ///
    /// And each cell's water (M14), for `vox_mesh::mesh_lod_cells`: the top
    /// block of the water standing on that same lowest column — a river's,
    /// a lake's, the sea's — or `i32::MIN` (`vox_mesh::LOD_DRY`) for none. A
    /// frozen lake, like a polar sea, rises to its ice.
    pub fn lod_surface(
        &self,
        origin_x: i64,
        origin_z: i64,
        h_stride: i64,
        sampling: LodSampling,
    ) -> (Vec<i32>, Vec<BlockId>, Vec<i32>) {
        debug_assert!(h_stride >= 1);
        let n = CHUNK_SIZE as i64;
        let probes = match sampling {
            LodSampling::Exact => h_stride,
            LodSampling::Sparse => h_stride.min(LOD_SPARSE_PROBES),
        };
        let step = h_stride / probes;
        let mut heights = vec![0i32; (n * n) as usize];
        let mut tops = vec![blocks::AIR; (n * n) as usize];
        let mut water = vec![vox_mesh_dry(); (n * n) as usize];
        for cz in 0..n {
            for cx in 0..n {
                let bx = origin_x + cx * h_stride;
                let bz = origin_z + cz * h_stride;
                // The lowest sample, kept with its surface so the column is
                // classified without computing it twice.
                let mut lowest = (i64::MAX, 0, 0, None);
                for pz in 0..probes {
                    for px in 0..probes {
                        let (x, z) = (bx + px * step, bz + pz * step);
                        let g = self.ground(x, z);
                        if g.0.height < lowest.0 {
                            lowest = (g.0.height, x, z, Some(g));
                        }
                    }
                }
                let (h, x, z, s) = lowest;
                let col = self.column_with(x, z, s.expect("at least one sample"));
                let at = (cz * n + cx) as usize;
                let wet = wet_top(&col.water);
                if col.biome == biome::Biome::PolarSea {
                    heights[at] = SEA_LEVEL_BLOCKS as i32;
                    tops[at] = vox_core::registry::ICE;
                } else if wet > h && self.block_in(&col, wet) == vox_core::registry::ICE {
                    // A frozen lake: its ice is the top, as sea ice is.
                    heights[at] = wet as i32;
                    tops[at] = vox_core::registry::ICE;
                } else {
                    heights[at] = h as i32;
                    tops[at] = self.block_in(&col, h);
                    if wet > h {
                        water[at] = wet as i32; // a river's or a lake's (M14)
                    } else if h < SEA_LEVEL_BLOCKS {
                        water[at] = SEA_LEVEL_BLOCKS as i32;
                    }
                }
            }
        }
        (heights, tops, water)
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
            // Lower chunk covers wy 0..32, upper covers 32..64.
            for wy in 0..(2 * CHUNK_SIZE as i64) {
                let expected = worldgen.block_at(wx, wy, wz);
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
        for y in [ground + 1, ground + 40, SEA_LEVEL_BLOCKS - 1] {
            assert_eq!(block(y), blocks::WATER, "no water at {y} (ground {ground})");
        }
        // The surface layer is water, or sea ice where the sea is polar (M13).
        let surface = block(SEA_LEVEL_BLOCKS);
        assert!(
            surface == blocks::WATER || surface == vox_core::registry::ICE,
            "{surface:?} at sea level"
        );
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
                g.block_at(x, y, z),
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
                    g.block_at(wx, wy, wz),
                    "chunk {pos:?} disagrees with the profile at ({wx}, {wy}, {wz})"
                );
            }
        }
    }

    /// M13: in every biome, every block of the chunks through the surface —
    /// the one above, the one holding it, the one below, and the one at the
    /// bottom of the deepest profile — equals the per-block profile. Covers
    /// the lazy path (columns classified only where a chunk meets them) and
    /// the stacked seams together.
    #[test]
    fn chunks_match_the_profile_in_every_biome() {
        use crate::biome::Biome;
        let g = Generator::new(0xB10_3E5, WorldShape::new(65_536, 65_536).unwrap());
        let s = CHUNK_SIZE as i64;
        let mut found = std::collections::BTreeMap::new();
        'search: for j in 0..160i64 {
            for i in 0..160i64 {
                let (x, z) = (i * 409 + 17, j * 409 + 5);
                found.entry(g.column(x, z).biome).or_insert((x, z));
                if found.len() == Biome::ALL.len() {
                    break 'search;
                }
            }
        }
        assert!(found.len() >= 18, "only {} biomes found", found.len());
        for (biome, (x, z)) in found {
            let top = g.surface_height(x, z).max(SEA_LEVEL_BLOCKS);
            let cy = top.div_euclid(s);
            let deepest = (top - materials::MAX_PROFILE_DEPTH).div_euclid(s);
            for chunk_y in [cy + 1, cy, cy - 1, deepest] {
                let pos = ChunkPos::new(x.div_euclid(s), chunk_y, z.div_euclid(s));
                let chunk = g.generate_chunk(pos);
                for local in LocalPos::iter() {
                    let (wx, wy, wz) = (
                        pos.x * s + local.x() as i64,
                        pos.y * s + local.y() as i64,
                        pos.z * s + local.z() as i64,
                    );
                    assert_eq!(
                        chunk.get(local),
                        g.block_at(wx, wy, wz),
                        "{biome:?}: chunk {pos:?} disagrees at ({wx}, {wy}, {wz})"
                    );
                }
            }
        }
    }

    /// M13 criterion 7, near and far agree: at stride 1 a LOD cell is one
    /// column, and its height and top block are exactly the solid top full
    /// resolution generates there — in every biome, polar sea ice included.
    /// At coarser strides the cell's top is the block at its lowest sample.
    #[test]
    fn lod_surface_matches_full_resolution_in_every_biome() {
        use crate::biome::Biome;
        let g = Generator::new(0xB10_3E5, WorldShape::new(65_536, 65_536).unwrap());
        let mut found = std::collections::BTreeMap::new();
        for j in 0..120i64 {
            for i in 0..120i64 {
                let (x, z) = (i * 541 + 3, j * 541 + 11);
                found.entry(g.column(x, z).biome).or_insert((x, z));
            }
        }
        assert!(found.contains_key(&Biome::PolarSea) && found.contains_key(&Biome::HotDesert));
        for (biome, (x, z)) in found {
            let (ox, oz) = (x - 16, z - 16);
            let (heights, tops, _) = g.lod_surface(ox, oz, 1, LodSampling::Exact);
            for cz in 0..32i64 {
                for cx in 0..32i64 {
                    let at = (cz * 32 + cx) as usize;
                    let (wx, wz) = (ox + cx, oz + cz);
                    let h = i64::from(heights[at]);
                    assert_eq!(tops[at], g.block_at(wx, h, wz), "{biome:?} at ({wx}, {wz})");
                    // The solid top: nothing solid above it, and it is solid.
                    assert_ne!(g.block_at(wx, h + 1, wz), vox_core::registry::ICE);
                    assert!(
                        ![blocks::AIR, blocks::WATER].contains(&tops[at]),
                        "{biome:?}: LOD top {:?} at ({wx}, {wz})",
                        tops[at]
                    );
                }
            }
            // Coarser: heights are the plain heightfield's, except polar
            // seas and frozen lakes (M14), raised to their ice.
            let (h8, t8, _) = g.lod_surface(ox, oz, 8, LodSampling::Sparse);
            let plain = g.lod_heightfield(ox, oz, 8, LodSampling::Sparse);
            for k in 0..h8.len() {
                if t8[k] == vox_core::registry::ICE {
                    assert!(plain[k] <= h8[k]);
                } else {
                    assert_eq!(h8[k], plain[k], "{biome:?}");
                }
            }
        }
    }

    /// The torus (ADR-0012) for M13's content: a column's biome, climate,
    /// patch and every block of its profile are the same on every lap, and
    /// columns either side of the seam are neighbours like any others —
    /// boundary and patch noise tile with the world as the terrain does.
    #[test]
    fn columns_and_their_blocks_wrap_with_the_world() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0x51D_E5EA, shape);
        let (sx, sz) = (shape.size_x(), shape.size_z());
        for k in 0..200i64 {
            let (x, z) = (k * 3_331 - 50_000, k * 7_919 + 13);
            let a = g.column(x, z);
            for (dx, dz) in [(sx, 0), (0, sz), (-2 * sx, 3 * sz)] {
                let b = g.column(x + dx, z + dz);
                assert_eq!(a, b, "({x}, {z}) and its copy {dx}, {dz} away");
                let h = a.surface.height;
                for y in [h - 5, h, h + 1, SEA_LEVEL_BLOCKS] {
                    assert_eq!(g.block_at(x, y, z), g.block_at(x + dx, y, z + dz));
                }
            }
        }
        // Across the seam itself: the last column and the first are
        // neighbours, so their patch noise is as close as any neighbours'.
        for z in (0..sz).step_by(1_009) {
            let (last, first) = (g.column(sx - 1, z), g.column(sx, z));
            assert!(
                (last.patch - first.patch).abs() < 0.2,
                "patch jumps at the seam, z = {z}"
            );
            // What stands there — ground, or the water of a lake or river.
            let top = |c: &Column| c.surface.height.max(wet_top(&c.water));
            assert!((top(&last) - top(&first)).abs() <= 3, "z = {z}");
        }
    }

    /// The erosion stage (M14 task 2) is part of the torus: the same on
    /// every lap, continuous across the seam — and the drainage network's
    /// ground is the generator's own surface, so the rivers it routes run
    /// on the ground the chunks are made of.
    #[test]
    fn the_erosion_stage_wraps_and_is_the_networks_ground() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0xE20_5104, shape);
        let d = g.drainage();
        let (sx, sz) = (shape.size_x(), shape.size_z());
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        for j in (0..nz).step_by(7) {
            for i in (0..nx).step_by(5) {
                let x = ((i as f64 + 0.5) * cw).floor() as i64;
                let z = ((j as f64 + 0.5) * ch).floor() as i64;
                assert_eq!(i64::from(d.cell(i, j).0), g.eroded_surface(x, z).height);
            }
        }
        let mut cut_somewhere = false;
        for z in (0..sz).step_by(97) {
            let zf = z as f64;
            let (last, first) = (
                d.erosion_at((sx - 1) as f64, zf),
                d.erosion_at(sx as f64, zf),
            );
            assert!(
                (last - first).abs() < 2.0,
                "the cut jumps at the seam, z = {z}"
            );
            for x in [5, 20_000, 40_001] {
                let here = d.erosion_at(x as f64, zf);
                assert!(here <= 0.0, "erosion only lowers");
                cut_somewhere |= here < -10.0;
                assert_eq!(here, d.erosion_at((x + sx) as f64, zf + sz as f64));
                assert_eq!(here, d.erosion_at((x - 3 * sx) as f64, zf));
            }
        }
        assert!(cut_somewhere, "no valleys at all");
    }

    /// The world folder's drainage cache (ADR-0019 §6): the first load
    /// builds and writes it, the next reads it, and the terrain is the same
    /// either way.
    #[test]
    fn the_drainage_cache_is_written_then_read() {
        let dir = std::env::temp_dir().join(format!("voxterra-drainage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shape = WorldShape::new(32_768, 32_768).unwrap();
        let first = Generator::new(0xCAC4E, shape);
        assert!(matches!(
            first.drainage_cached(&dir).1,
            DrainageSource::Built
        ));
        assert!(dir.join(DRAINAGE_CACHE_FILE).exists());
        assert!(matches!(
            first.drainage_cached(&dir).1,
            DrainageSource::AlreadyBuilt
        ));
        let second = Generator::new(0xCAC4E, shape);
        assert!(matches!(
            second.drainage_cached(&dir).1,
            DrainageSource::Cache
        ));
        for k in 0..64i64 {
            let (x, z) = (k * 257 - 3_000, k * 911 + 17);
            assert_eq!(first.surface(x, z), second.surface(x, z));
        }
        // Another world in the same folder does not take it.
        let other = Generator::new(0xCAC4F, shape);
        assert!(matches!(
            other.drainage_cached(&dir).1,
            DrainageSource::Built
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Measures the cache at the default size: run with `--ignored
    /// --nocapture` (release) for its file size and load time.
    #[test]
    #[ignore]
    fn drainage_cache_cost() {
        let dir = std::env::temp_dir().join(format!("voxterra-cost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        g.climate();
        let t = std::time::Instant::now();
        let (_, s) = g.drainage_cached(&dir);
        println!("{s:?} in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        let bytes = std::fs::metadata(dir.join(DRAINAGE_CACHE_FILE))
            .unwrap()
            .len();
        let h = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        let t = std::time::Instant::now();
        let (_, s) = h.drainage_cached(&dir);
        println!(
            "{s:?} in {:.0} ms; file {:.1} MB",
            t.elapsed().as_secs_f64() * 1000.0,
            bytes as f64 / 1e6
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Rivers and lakes (M14 task 3): standing water is never held by air.
    /// Around rivers and lakes of every size, each column's water is met at
    /// each side by ground as high or by water — the same body, or a river
    /// stepping down its long profile in whole blocks.
    #[test]
    fn water_is_never_held_by_air() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0x41_7E45, shape);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        // Places to look: cells of every size of river, and lakes.
        let mut sites = Vec::new();
        for j in (0..nz).step_by(3) {
            for i in (0..nx).step_by(3) {
                let (_, _, q, _) = d.cell(i, j);
                let bucket = if d.is_lake(i, j) {
                    Some(4)
                } else if q >= 1.0 {
                    Some(3)
                } else if q >= 0.1 {
                    Some(2)
                } else if q >= drainage::STREAM_FROM_M3S {
                    Some(1)
                } else {
                    None
                };
                if let Some(b) = bucket
                    && sites.iter().filter(|&&(k, _, _)| k == b).count() < 6
                {
                    sites.push((b, i, j));
                }
            }
        }
        assert!(
            sites.len() >= 20,
            "too few rivers and lakes: {}",
            sites.len()
        );
        let top = |x: i64, z: i64| {
            let (s, w) = g.ground(x, z);
            (s.height, wet_top(&w))
        };
        let (mut wet, mut falls, mut held_by_air) = (0u32, 0u32, Vec::new());
        for &(_, i, j) in &sites {
            let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
            for z in (z0..z0 + 128).step_by(2) {
                for x in (x0..x0 + 128).step_by(2) {
                    let (h, level) = top(x, z);
                    if level <= h {
                        continue;
                    }
                    wet += 1;
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        // Ground as high holds it; lower water beside it is
                        // the river stepping down its long profile — a block
                        // (rapids) or more (a waterfall).
                        let (nh, nl) = top(x + dx, z + dz);
                        if nl > nh && nl < level - 1 {
                            falls += 1;
                        }
                        if nh < level && nl <= nh {
                            held_by_air.push((x, z, level, nh, nl));
                        }
                    }
                }
            }
        }
        eprintln!("{wet} wet columns, {falls} sides stepping down 2+ blocks");
        assert!(wet > 1_000, "only {wet} wet columns");
        assert!(
            held_by_air.is_empty(),
            "{} of {wet} wet columns held by air, e.g. {:?}",
            held_by_air.len(),
            &held_by_air[..held_by_air.len().min(8)]
        );
    }

    /// A lake's surface is in the span streaming loads (M14): it can stand
    /// tens of blocks above its bed, in chunk layers above the ground's.
    #[test]
    fn the_surface_span_reaches_standing_water() {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut checked = 0;
        'cells: for j in (0..nz).step_by(5) {
            for i in (0..nx).step_by(5) {
                let x = ((i as f64 + 0.5) * cw) as i64;
                let z = ((j as f64 + 0.5) * ch) as i64;
                let (s, w) = g.ground(x, z);
                if wet_top(&w) < s.height + 2 * CHUNK_SIZE as i64 {
                    continue;
                }
                let (lo, hi) = g.surface_span_chunks(x >> 5, z >> 5);
                assert!(lo <= s.height >> 5 && (w.level >> 5) <= hi, "({x}, {z})");
                checked += 1;
                if checked == 20 {
                    break 'cells;
                }
            }
        }
        assert!(checked > 0, "no deep lake to check");
    }

    /// Criterion 8 for water (M14 task 5): at full resolution the LOD's cell
    /// is the column, so its water is the column's own — a river's, a
    /// lake's or the sea's, at that level — or its ice.
    #[test]
    fn lod_water_is_the_columns_water() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0x41_7E45, shape);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut wet = 0;
        for j in (0..nz).step_by(37) {
            for i in (0..nx).step_by(37) {
                let (ox, oz) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
                let (heights, tops, water) = g.lod_surface(ox, oz, 1, LodSampling::Exact);
                for k in 0..heights.len() {
                    let (x, z) = (ox + (k % 32) as i64, oz + (k / 32) as i64);
                    let (s, w) = g.ground(x, z);
                    let top = wet_top(&w);
                    if tops[k] == vox_core::registry::ICE {
                        continue; // frozen: the ice is the top
                    }
                    let expect = if top > s.height {
                        top as i32
                    } else if s.height < SEA_LEVEL_BLOCKS {
                        SEA_LEVEL_BLOCKS as i32
                    } else {
                        i32::MIN
                    };
                    assert_eq!(water[k], expect, "({x}, {z})");
                    wet += usize::from(top > s.height);
                }
            }
        }
        assert!(wet > 0, "no river or lake sampled");
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
        // Biomes and surfaces (M13), at enough scattered columns to land on
        // biome edges: a change to classification or materials shows here
        // even where it moves no height and misses the chunks above.
        for i in 0..4_096i64 {
            let (x, z) = (i * 5_003 - 77_777, i * 70_001 + 19);
            let col = g.column(x, z);
            eat(&[col.biome as u8]);
            eat(&g.block_at(x, col.surface.height, z).0.to_le_bytes());
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
        const PINNED: (u32, u64) = (4, 0x6845_14bc_0518_b11c);
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
