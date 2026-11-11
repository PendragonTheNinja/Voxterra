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
pub mod process;
pub mod tectonics;

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Stage-1 landform processes (M15, ADR-0023), built on first use.
    process: Arc<OnceLock<process::ProcessMap>>,
    /// Recently generated chunk columns' ground, shared by every clone
    /// ([`GroundCache`]): a speed-up only, never a source of truth.
    grounds: Arc<GroundCache>,
}

/// How many chunk columns [`GroundCache`] remembers: 64 KB each, so ~16 MB.
/// Sized for the streamer's frontier — the columns whose layers are still
/// arriving, a ring one or two chunks thick at the load radius — at the
/// largest radius the settings allow.
const GROUND_CACHE_COLUMNS: usize = 256;

/// One chunk column's ground: [`Generator::ground`] at each of its 32x32
/// block columns, indexed `x + z * 32`, with the bounds every chunk of the
/// stack is first tested against.
#[derive(Debug)]
struct ColumnGround {
    cells: Box<[(elevation::Surface, Water)]>,
    /// The lowest ground in the column.
    min_surface: i64,
    /// The highest ground or standing water in the column.
    max_surface: i64,
}

/// The ground of the chunk columns generated most recently.
///
/// A chunk's contents follow from the ground of its 1 024 block columns,
/// and that ground is the same for every chunk of a vertical stack — but
/// the streamer asks for a stack's chunks one at a time (seven or so per
/// column: the surface and its margins), and each used to work the ground
/// out again before it could tell it was all air or all stone. That was
/// most of generation: 0.48 ms of a uniform chunk's 0.48, and of a mixed
/// chunk's 1.0 (2026-11 audit, dev build). Remembering the column makes
/// every chunk after the first in its stack nearly free.
///
/// **Only a cache.** Any chunk still generates alone, from the seed, in any
/// order (CLAUDE.md): a miss computes the column, and a hit returns what
/// that same computation returned before. `ground` is a pure function of
/// the world, so the two cannot differ —
/// `the_ground_cache_never_changes_a_chunk` holds them to it.
///
/// Bounded first-in-first-out: generation arrives nearest-first, so a
/// column's chunks come close together and recency is all that matters.
/// The lock is held for a lookup or an insert, never while computing; two
/// workers that miss the same column at once both compute it, and agree.
#[derive(Default)]
struct GroundCache {
    inner: Mutex<GroundCacheInner>,
}

#[derive(Default)]
struct GroundCacheInner {
    columns: HashMap<(i64, i64), Arc<ColumnGround>>,
    /// Keys in insertion order, oldest first.
    order: VecDeque<(i64, i64)>,
}

impl std::fmt::Debug for GroundCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A poisoned lock only means a worker panicked mid-insert; the map
        // is still a map of whole entries.
        let len = self.inner.lock().map_or(0, |inner| inner.columns.len());
        write!(f, "GroundCache({len} columns)")
    }
}

impl GroundCache {
    fn get(&self, key: (i64, i64)) -> Option<Arc<ColumnGround>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.columns.get(&key).cloned()
    }

    fn insert(&self, key: (i64, i64), column: Arc<ColumnGround>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.columns.insert(key, column).is_none() {
            inner.order.push_back(key);
            while inner.order.len() > GROUND_CACHE_COLUMNS {
                if let Some(old) = inner.order.pop_front() {
                    inner.columns.remove(&old);
                }
            }
        }
    }
}

/// The LOD's mark for a dry cell (`vox_mesh::LOD_DRY`; vox-worldgen does not
/// depend on vox-mesh).
const fn vox_mesh_dry() -> i32 {
    i32::MIN
}

/// The top of a column's standing water, or `i64::MIN` where it has none.
fn wet_top(w: &Water) -> i64 {
    match w.body {
        Body::River(_) | Body::Lake | Body::SaltLake | Body::Pool => w.level,
        Body::Dry | Body::SaltFlat | Body::Wetland | Body::Floodplain | Body::Delta => i64::MIN,
    }
}

/// Lowland rivers (task 4): below this slope a river has a floodplain.
const LOWLAND_SLOPE: f32 = 0.003;
/// How far beyond its banks a lowland river's flats reach: this many of
/// its widths, at least [`LOWLAND_MIN_REACH`] blocks.
const LOWLAND_WIDTHS: f32 = 4.0;
const LOWLAND_MIN_REACH: f32 = 16.0;
/// How far below a floodplain's or wetland's level ground may lie and
/// still count as one, blocks; flats never raise ground (the owner's walk).
const LOWLAND_BELOW: i64 = 2;
/// Ground this far above a flat's level is left as it is; below, it is cut
/// down to the flat, easing out (blocks).
const LOWLAND_ABOVE: i64 = 4;

/// Hermite smoothstep of `e` over 0..1.
fn smooth(e: f32) -> f32 {
    let e = e.clamp(0.0, 1.0);
    e * e * (3.0 - 2.0 * e)
}
/// Precipitation over potential evaporation from which a lowland is a
/// wetland rather than a floodplain.
const WETLAND_HUMIDITY: f32 = 1.0;
/// Patch noise below which a wetland column holds a pool.
const POOL_BELOW: f32 = -0.3;
/// A delta's distributaries: the wavelength of the noise whose zero line
/// they follow, blocks, and how far from zero counts as channel (~4 blocks
/// wide).
const DISTRIBUTARY_WAVELENGTH: f32 = 128.0;
const DISTRIBUTARY_WIDTH: f32 = 0.06;
/// Oxbow lakes: on floodplains of rivers at least this wide (blocks); how
/// thick the crescent is, as noise; and the share of the zero line that is
/// lake (above this on a second noise).
const OXBOW_FROM_WIDTH: f32 = 6.0;
const OXBOW_THICKNESS: f32 = 0.05;
const OXBOW_ARCS: f32 = 0.35;

/// The least discharge that builds a delta at its mouth, m³/s, in a world
/// of the default size or larger ([`Generator::delta_from`]).
const DELTA_FROM_M3S: f32 = 0.5;

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
/// valleys carved by stream power from the drainage network. Rivers, lakes,
/// wetlands, deltas and waterfalls (tasks 3–5) came in under it before any
/// world was saved with it.
/// 5 — rivers sit in their ground (M14, the owner's walk): reaches seated
/// under the full-resolution ground across their meander belt, waterfalls
/// where the ground falls away and at most 32 blocks high, fills sloping to
/// their sides, lakes held by banks, banks blended smoothly into the land,
/// floodplains laid only near their river's level.
/// 6 — more of the owner's walk: floodplains, wetlands and deltas cut ground
/// down to their level and blend into the land, never raise it; river
/// shaping ends where a lake or the sea covers the ground (no dikes out
/// across the water); a lake's banks stand only where no water as high
/// covers them; river banks stand level with the water, not a block above.
/// 7 — the owner's walk, continued (ADR-0020 §10): each reach's long profile
/// follows its ground (stored in stage 1, drainage cache format 4), seated
/// at its node and never under a lake; ponds where a river crosses a hollow
/// below its grade; fills cut before banks; lakes, fills and salt flats
/// reach wavy discs about their cells, not squares; lake banks only where
/// the lake reaches them; channels run on into lakes.
/// 8 — the owner's third flight: sea and coast biomes only within ~150
/// blocks of the sea, land below sea level farther inland a lake at sea
/// level; rivers end at any ground below sea level; a braided river's bars
/// only in its middle, never across it.
/// 9 — milestone 16: the tectonic framework places the terrain (ADR-0025).
/// Continents are continental crust, mountain belts stand along
/// collisions, arcs and old welds, trenches lie off subduction zones; new
/// are mid-ocean ridges and rift valleys. Every world's ground moved.
pub const GENERATOR_VERSION: u32 = 9;

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
            process: Arc::new(OnceLock::new()),
            grounds: Arc::new(GroundCache::default()),
        }
    }

    /// The world's tectonic framework (milestone 16, ADR-0025): its
    /// terranes and plates, and what every boundary between them is.
    /// The terrain's continents, mountain belts, trenches, ocean ridges
    /// and rifts are placed by it (`Elevation`).
    pub fn tectonics(&self) -> &tectonics::Tectonics {
        self.elevation.tectonics()
    }

    /// The world's landform-process map (M15, ADR-0023): where wind, flash
    /// floods, ice, deep weathering and rain each shape the land. Built on
    /// first use after the climate it reads (a fraction of a second at the
    /// default size) and shared by every clone. Nothing in generation reads
    /// it yet — the landform layers (M15 tasks 2–4) will.
    pub fn process(&self) -> &process::ProcessMap {
        self.process
            .get_or_init(|| process::ProcessMap::build(&self.elevation, self.climate(), self.shape))
    }

    /// The ground of chunk column `(cx, cz)`, from the cache or computed
    /// and remembered ([`GroundCache`]).
    fn column_ground(&self, cx: i64, cz: i64) -> Arc<ColumnGround> {
        if let Some(column) = self.grounds.get((cx, cz)) {
            return column;
        }
        let n = CHUNK_SIZE as i64;
        let (ox, oz) = (cx * n, cz * n);
        let mut cells = Vec::with_capacity((n * n) as usize);
        let mut min_surface = i64::MAX;
        let mut max_surface = i64::MIN;
        for lz in 0..n {
            for lx in 0..n {
                let g = self.ground(ox + lx, oz + lz);
                min_surface = min_surface.min(g.0.height);
                // Rivers and lakes stand above the ground: a chunk is not
                // all air below their surface.
                max_surface = max_surface.max(g.0.height).max(wet_top(&g.1));
                cells.push(g);
            }
        }
        let column = Arc::new(ColumnGround {
            cells: cells.into_boxed_slice(),
            min_surface,
            max_surface,
        });
        self.grounds.insert((cx, cz), Arc::clone(&column));
        column
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
        let (h, water) = self.drainage().shape_column(
            wx,
            wz,
            s.height,
            |x, z, l| self.elevation.meander_warp(x, z, l),
            |x, z| self.eroded_surface(x, z).height,
        );
        let (h, water) = self.lowlands(wx, wz, h, water, true);
        s.height = h;
        (s, water)
    }

    /// The least discharge that builds a delta in this world: large for its
    /// world. Discharge grows with catchment area, which grows with the
    /// world's area, so a small world's largest rivers are tens of times
    /// smaller than the default world's (as M13 widened small worlds'
    /// mountain belts only where needed, `Elevation::calibrate_ranges`).
    fn delta_from(&self) -> f32 {
        let d = WorldShape::DEFAULT;
        let area = (self.shape.size_x() as f64 * self.shape.size_z() as f64)
            / (d.size_x() as f64 * d.size_z() as f64);
        DELTA_FROM_M3S * area.min(1.0) as f32
    }

    /// [`Self::ground`]'s top — its ground, or water standing on it —
    /// without wetland pools: what a pool's neighbours must stand at for it
    /// to hold.
    fn ground_without_pools(&self, wx: i64, wz: i64) -> i64 {
        let s = self.eroded_surface(wx, wz);
        let (h, water) = self.drainage().shape_column(
            wx,
            wz,
            s.height,
            |x, z, l| self.elevation.meander_warp(x, z, l),
            |x, z| self.eroded_surface(x, z).height,
        );
        let (h, water) = self.lowlands(wx, wz, h, water, false);
        // Water as high holds a pool as ground does: a river's channel
        // beside it had failed the test, leaving a strip of mud along every
        // river crossing a wetland, an outline (the owner's walk).
        h.max(wet_top(&water))
    }

    /// The flat ground beside lowland rivers (M14 task 4): wetlands where
    /// it is wet, floodplains where it is not, deltas at large mouths.
    /// Each is shaped flat at its river's level, as the river's floods and
    /// sediment make it; a wetland is waterlogged, with shallow pools.
    fn lowlands(&self, wx: i64, wz: i64, h: i64, water: Water, pools: bool) -> (i64, Water) {
        let Some(n) = water.near else {
            return (h, water);
        };
        if water.body != Body::Dry || n.out < 0.0 {
            return (h, water);
        }
        let reach = (LOWLAND_WIDTHS * n.width).max(LOWLAND_MIN_REACH);
        let pool = self.elevation.patch_noise(wx, wz) < POOL_BELOW;
        let with = |body| Water { body, ..water };
        // Flats are cut, never built: ground above `target` is lowered to
        // it — fully near the river and within a block or so above it,
        // easing out to the natural ground by `span` from the river and by
        // LOWLAND_ABOVE above it, so they meet the land without a step.
        // Ground below it is left as it is (a raised strip a block above
        // flat desert, with a step at its edge, was the owner's "raised
        // blocks around them"). Returns the ground, and whether it is flat
        // at the target — within LOWLAND_BELOW under it.
        //
        // And never below a lake standing beside it: where a river leaves
        // a lake a block below the lake's surface, a flat cut to the
        // river's level took the lake's shore away, and the lake stood a
        // block over the wetland with nothing to hold it (found when
        // milestone 16 moved the terrain under
        // `lake_shores_hold_their_water`).
        let flatten = |target: i64, span: f32| {
            let above = h - target;
            if above <= 0 {
                return (h, above >= -LOWLAND_BELOW);
            }
            let w = (1.0 - smooth(n.out / span))
                * (1.0 - smooth((above - 1) as f32 / (LOWLAND_ABOVE - 1) as f32));
            let g = (h as f32 - above as f32 * w).round() as i64;
            let g = match self
                .drainage()
                .lake_beside(wx, wz, |x, z, l| self.elevation.meander_warp(x, z, l))
            {
                Some(lake) if g < lake => lake.min(h),
                _ => g,
            };
            (g, g <= target)
        };
        // A large river's mouth on a gentle coast — low ground beside it,
        // however steeply its last reach fell: flats at sea level, the sea
        // in their pools.
        if n.mouth && n.discharge >= self.delta_from() && n.out < 2.0 * reach && h <= 3 {
            // Distributaries: the river splits across its delta in winding
            // channels — where a smooth noise crosses zero, lines that wind
            // and branch — cut below the sea, which fills them.
            let (braid, _) = self.elevation.meander_warp(wx, wz, DISTRIBUTARY_WAVELENGTH);
            let target = if braid.abs() < DISTRIBUTARY_WIDTH {
                -2
            } else if pool {
                -1
            } else {
                1
            };
            let (g, flat) = flatten(target, 2.0 * reach);
            return if flat && g >= -3 {
                (g, with(Body::Delta))
            } else {
                (g, water)
            };
        }
        if n.slope >= LOWLAND_SLOPE || n.out >= reach {
            return (h, water);
        }
        let (sea_level_c, precip) = self.climate().sample(wx, wz);
        let pet = climate::potential_evaporation(climate::temperature_at(sea_level_c, h));
        if precip >= WETLAND_HUMIDITY * pet {
            let (g, flat) = flatten(n.level, reach);
            if !flat {
                return (g, water);
            }
            // A pool only where every side stands as high as its water —
            // inside its own wetland, not beside another river's lower one.
            if pools
                && pool
                && g == n.level
                && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .all(|&(dx, dz)| self.ground_without_pools(wx + dx, wz + dz) >= n.level)
            {
                let w = Water {
                    level: n.level,
                    body: Body::Pool,
                    near: water.near,
                };
                return (n.level - 1, w);
            }
            return (g, with(Body::Wetland));
        }
        let (g, flat) = flatten(n.level + 1, reach);
        if !flat {
            return (g, water);
        }
        // Oxbow lakes: on a wide meandering river's floodplain, crescents
        // of water where old bends were cut off — short arcs of a
        // meander-scale noise's zero line — each only where every side
        // holds it, as a wetland's pool.
        if pools && n.width >= OXBOW_FROM_WIDTH && g == n.level + 1 {
            let (arc, cut) = self.elevation.meander_warp(wx, wz, 12.0 * n.width);
            if arc.abs() < OXBOW_THICKNESS
                && cut > OXBOW_ARCS
                && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .all(|&(dx, dz)| self.ground_without_pools(wx + dx, wz + dz) >= n.level)
            {
                let w = Water {
                    level: n.level,
                    body: Body::Lake,
                    near: water.near,
                };
                return (n.level - 2, w);
            }
        }
        (g, with(Body::Floodplain))
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
            self.drainage().is_coastal(wx, wz),
        );
        col.patch = self.elevation.patch_noise(wx, wz);
        col.water = water;
        if let Some(b) = biome::water_biome(water.body, col.temperature, col.sea_level_temperature)
        {
            col.biome = b;
        }
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

        // Fast rejection: the surface range over this chunk's columns, which
        // every chunk of the stack shares ([`GroundCache`]). If the whole
        // chunk is above the highest surface, it's all air (or sea); if
        // wholly below every column's surface profile, all stone.
        let ground = self.column_ground(pos.x, pos.z);
        let surfaces = &ground.cells;
        let (min_surface, max_surface) = (ground.min_surface, ground.max_surface);

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
                // Sea ice is on the sea: a polar-sea column whose ground
                // stands exactly at sea level has no water to freeze, and
                // its top is its ground (the chunk generator's, which the
                // LOD said was ice until milestone 16 moved the terrain
                // onto such a column in
                // `lod_surface_matches_full_resolution_in_every_biome`).
                if col.biome == biome::Biome::PolarSea && h < SEA_LEVEL_BLOCKS {
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

    /// A generator sharing `g`'s stage-1 maps but remembering no ground:
    /// what any chunk generates as when it is the first of its column.
    fn cold(g: &Generator) -> Generator {
        Generator {
            grounds: Arc::new(GroundCache::default()),
            ..g.clone()
        }
    }

    /// The ground cache is only a cache (CLAUDE.md: any chunk generates
    /// alone, in any order). Chunks generated through a warm cache — their
    /// column's ground remembered from another chunk of the stack, evicted
    /// and recomputed, visited in either order — are byte-identical to the
    /// same chunks generated cold.
    #[test]
    fn the_ground_cache_never_changes_a_chunk() {
        let shape = WorldShape::new(32_768, 32_768).unwrap();
        let warm = Generator::new(0x00C0_FFEE, shape);
        // More columns than the cache holds, so the second pass meets
        // columns that were evicted, and some that were not.
        let columns: Vec<(i64, i64)> = (0..GROUND_CACHE_COLUMNS as i64 + 40)
            .map(|i| (i * 3 - 200, (i * 7) % 97 - 40))
            .collect();
        let mut mixed = 0;
        for pass in 0..2 {
            for (i, &(cx, cz)) in columns.iter().enumerate() {
                let (lo, hi) = warm.surface_span_chunks(cx, cz);
                let mut layers = [hi + 2, lo - 2, lo, hi];
                if pass == 1 {
                    layers.reverse();
                }
                for y in layers {
                    let pos = ChunkPos::new(cx, y, cz);
                    let chunk = warm.generate_chunk(pos);
                    mixed += usize::from(!chunk.is_uniform());
                    // Every warm chunk is generated; one column in eight is
                    // also generated cold and compared, to keep this quick.
                    if i % 8 == pass {
                        assert_eq!(
                            chunk.serialize(),
                            cold(&warm).generate_chunk(pos).serialize(),
                            "chunk {pos:?} differs through the cache (pass {pass})"
                        );
                    }
                }
            }
        }
        assert!(
            mixed > 100,
            "only {mixed} mixed chunks: the test saw no terrain"
        );
        let inner = warm.grounds.inner.lock().unwrap();
        assert_eq!(
            inner.columns.len(),
            GROUND_CACHE_COLUMNS,
            "the cache is bounded"
        );
        assert_eq!(inner.order.len(), GROUND_CACHE_COLUMNS);
    }

    /// A remembered column is exactly `ground` at each of its block columns,
    /// in negative coordinates too.
    #[test]
    fn a_cached_column_is_the_ground_of_its_columns() {
        let g = Generator::new(7, WorldShape::new(32_768, 32_768).unwrap());
        for (cx, cz) in [(0, 0), (-1, -1), (-37, 12), (500, -500)] {
            let column = g.column_ground(cx, cz);
            let n = CHUNK_SIZE as i64;
            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
            for lz in 0..n {
                for lx in 0..n {
                    let want = g.ground(cx * n + lx, cz * n + lz);
                    assert_eq!(column.cells[(lx + lz * n) as usize], want);
                    lo = lo.min(want.0.height);
                    hi = hi.max(want.0.height).max(wet_top(&want.1));
                }
            }
            assert_eq!((column.min_surface, column.max_surface), (lo, hi));
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
        // Water standing on a column: a river's or a lake's, or the sea's
        // over ground below sea level.
        let top = |x: i64, z: i64| {
            let (s, w) = g.ground(x, z);
            let sea = if s.height < SEA_LEVEL_BLOCKS {
                SEA_LEVEL_BLOCKS
            } else {
                i64::MIN
            };
            (s.height, wet_top(&w).max(sea))
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

    /// Lake shores hold their water (the owner's walk, M14): a lake reaches
    /// past its cells into its neighbours' low ground, and where the ground
    /// beyond that was lower still, its water stood over air — 17 blocks of
    /// it at the bench's waterfall. Scans the cells around lakes' edges,
    /// where `water_is_never_held_by_air`'s sites never looked.
    #[test]
    fn lake_shores_hold_their_water() {
        let shape = WorldShape::DEFAULT;
        let g = Generator::new(0x7e22ac0de, shape);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut shores = Vec::new();
        for j in (0..nz).step_by(5) {
            for i in (0..nx).step_by(5) {
                let dry_beside = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(di, dj)| {
                    let (ii, jj) = (
                        (i + nx).wrapping_add_signed(di) % nx,
                        (j + nz).wrapping_add_signed(dj) % nz,
                    );
                    !d.is_lake(ii, jj) && !d.is_sea(ii, jj)
                });
                if d.is_lake(i, j) && dry_beside && shores.len() < 40 {
                    shores.push((i as i64, j as i64));
                }
            }
        }
        assert!(shores.len() >= 12, "too few lake shores: {}", shores.len());
        let top = |x: i64, z: i64| {
            let (s, w) = g.ground(x, z);
            let sea = if s.height < SEA_LEVEL_BLOCKS {
                SEA_LEVEL_BLOCKS
            } else {
                i64::MIN
            };
            (s.height, wet_top(&w).max(sea))
        };
        let (mut wet, mut held_by_air) = (0u32, Vec::new());
        for &(i, j) in &shores {
            let (x0, z0) = (((i - 1) as f64 * cw) as i64, ((j - 1) as f64 * ch) as i64);
            let span = (3.0 * cw) as i64;
            for z in (z0..z0 + span).step_by(2) {
                for x in (x0..x0 + span).step_by(2) {
                    let (h, level) = top(x, z);
                    if level <= h {
                        continue;
                    }
                    wet += 1;
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let (nh, nl) = top(x + dx, z + dz);
                        if nh < level && nl <= nh {
                            held_by_air.push((x, z, level, nh));
                        }
                    }
                }
            }
        }
        assert!(wet > 1_000, "only {wet} wet columns");
        assert!(
            held_by_air.is_empty(),
            "{} sides of {wet} wet columns held by air, e.g. {:?}",
            held_by_air.len(),
            &held_by_air[..held_by_air.len().min(8)]
        );
    }

    /// Rivers sit in their ground, never on walls (the owner's walk, M14):
    /// river shaping had built ground up to hold rivers whose levels, from
    /// the 128-block network, stood above the full-resolution ground — an
    /// 85-block wall under a mountain waterfall's pool, ridges along rivers
    /// crossing hillsides, raised platforms of floodplain. Around rivers of
    /// every size, counts the steps shaping makes between neighbouring
    /// columns over 10 blocks steeper than the natural ground's, and the
    /// columns it raises more than 16 blocks without a fill (sediment
    /// filling a hollow to a river's grade, flat and meeting its sides).
    #[test]
    fn rivers_build_no_walls() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0x41_7E45, shape);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let filled = |i: usize, j: usize| {
            let (h, l, _, _) = d.cell(i, j);
            l > h && !d.is_lake(i, j)
        };
        let (mut columns, mut cliffs, mut raised) = (0u32, Vec::new(), Vec::new());
        for j in (0..nz).step_by(4) {
            for i in (0..nx).step_by(4) {
                let (_, _, q, _) = d.cell(i, j);
                if q < drainage::STREAM_FROM_M3S || d.is_sea(i, j) || d.is_lake(i, j) {
                    continue;
                }
                let near_fill =
                    (0..9).any(|k| filled((i + nx + k % 3 - 1) % nx, (j + nz + k / 3 - 1) % nz));
                let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
                for z in (z0..z0 + ch as i64).step_by(2) {
                    for x in (x0..x0 + cw as i64).step_by(2) {
                        let (s, w) = g.ground(x, z);
                        if w.near.is_none_or(|n| n.out > 48.0) {
                            continue;
                        }
                        columns += 1;
                        let nat = g.eroded_surface(x, z).height;
                        let wet = |w: &Water| !matches!(w.body, drainage::Body::Dry);
                        if !wet(&w) && !near_fill && s.height - nat > 16 {
                            raised.push((x, z, s.height - nat));
                        }
                        let (sn, wn) = g.ground(x + 1, z);
                        if wet(&w) || wet(&wn) {
                            continue;
                        }
                        let nn = g.eroded_surface(x + 1, z).height;
                        let excess = (s.height - sn.height).abs() - (nat - nn).abs();
                        if excess > 10 {
                            cliffs.push((x, z, excess));
                        }
                    }
                }
            }
        }
        eprintln!(
            "{columns} columns by rivers: {} cliffs, {} raised",
            cliffs.len(),
            raised.len()
        );
        assert!(columns > 50_000, "only {columns} columns by rivers");
        assert!(
            cliffs.len() * 2_000 < columns as usize,
            "{} cliffs shaping made among {columns} columns, e.g. {:?}",
            cliffs.len(),
            &cliffs[..cliffs.len().min(6)]
        );
        assert!(
            raised.len() * 2_000 < columns as usize,
            "{} columns raised 16+ blocks among {columns}, e.g. {:?}",
            raised.len(),
            &raised[..raised.len().min(6)]
        );
    }

    /// Rivers end at still water, and flats are cut, never built (the
    /// owner's walk, M14): a reach running on to its lake's or the sea's
    /// cell centre had built its channel and levees out across the water
    /// as a dike, and floodplains set a block above their river raised
    /// strips with steps at their edges. Around rivers of every size: no
    /// floodplain, wetland or delta stands above the ground the river (and
    /// any fill) left it, and no ground under a lake's cell or the sea is
    /// raised out of the water.
    #[test]
    fn rivers_end_at_still_water_and_flats_are_cut() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let g = Generator::new(0x41_7E45, shape);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let (mut flats, mut drowned) = (0u32, 0u32);
        let (mut raised_flats, mut raised_under) = (Vec::new(), Vec::new());
        for j in (0..nz).step_by(3) {
            for i in (0..nx).step_by(3) {
                let (_, _, q, (ri, rj)) = d.cell(i, j);
                if q < drainage::STREAM_FROM_M3S || d.is_sea(i, j) {
                    continue;
                }
                // The cell and its receiver: where a river meets the water.
                for (ci, cj) in [(i, j), (ri, rj)] {
                    let (x0, z0) = ((ci as f64 * cw) as i64, (cj as f64 * ch) as i64);
                    let (_, lake_level, _, _) = d.cell(ci, cj);
                    let still = if d.is_sea(ci, cj) {
                        Some(0)
                    } else if d.is_lake(ci, cj) {
                        Some(i64::from(lake_level))
                    } else {
                        None
                    };
                    for z in (z0..z0 + ch as i64).step_by(2) {
                        for x in (x0..x0 + cw as i64).step_by(2) {
                            let (s, w) = g.ground(x, z);
                            let nat = g.eroded_surface(x, z).height;
                            if matches!(w.body, Body::Floodplain | Body::Wetland | Body::Delta) {
                                flats += 1;
                                // The ground as the river and any fill left
                                // it, before the flats are laid.
                                let (before, _) = d.shape_column(
                                    x,
                                    z,
                                    nat,
                                    |x, z, l| g.elevation.meander_warp(x, z, l),
                                    |x, z| g.eroded_surface(x, z).height,
                                );
                                if s.height > before {
                                    raised_flats.push((x, z, s.height - before));
                                }
                            }
                            if let Some(level) = still
                                && nat < level
                            {
                                drowned += 1;
                                // Raised out of the water: a dike. (Slopes
                                // under it — a fill's or a higher lake's
                                // bank running down into it — are not.)
                                if s.height > nat && s.height >= level {
                                    raised_under.push((x, z, s.height - nat));
                                }
                            }
                        }
                    }
                }
            }
        }
        eprintln!("{flats} flat columns, {drowned} under still water");
        assert!(
            flats > 5_000 && drowned > 5_000,
            "{flats} flats, {drowned} drowned"
        );
        assert!(
            raised_flats.is_empty(),
            "{} of {flats} flats raised, e.g. {:?}",
            raised_flats.len(),
            &raised_flats[..raised_flats.len().min(6)]
        );
        assert!(
            raised_under.is_empty(),
            "{} of {drowned} columns under still water raised, e.g. {:?}",
            raised_under.len(),
            &raised_under[..raised_under.len().min(6)]
        );
    }

    /// Rivers run in valleys, not slots (the owner's walk, M14): beside a
    /// mountain river a lake's bank, a fill's slope and a ledge each raised
    /// the ground back up after the channel's banks were cut, and the river
    /// was seated 27 blocks below the lake it drains — water in a slot one
    /// block wide and 13 deep, the owner's "insanely deep grooves", at
    /// their VIEW (−51 679, −82 227). There, and around rivers of every size
    /// in another world, the ground a block or two from a channel stands no
    /// more than a few blocks above its water (beside a waterfall, where the
    /// pool above is near, it may).
    #[test]
    fn rivers_run_in_valleys_not_slots() {
        let slots = |g: &Generator, columns: &mut dyn Iterator<Item = (i64, i64)>| {
            let (mut banks, mut slots) = (0u32, Vec::new());
            for (x, z) in columns {
                let (s, w) = g.ground(x, z);
                let Some(n) = w.near else { continue };
                if w.body != Body::Dry || !(0.0..2.0).contains(&n.out) {
                    continue;
                }
                banks += 1;
                // A 45° bank from a block above the water rises ~3 here;
                // uneven ground and stepping water allow a few more.
                if s.height <= n.level + 8 {
                    continue;
                }
                // Beside a waterfall the bank of the water above stands
                // that far over the water below, and should: it is what
                // holds the upper river. A slot is ground high above ALL
                // the water near it. (Milestone 16 gave rivers mountain
                // chains to come down, and torrents with falls every few
                // blocks went from rare to common.)
                let highest = (-3..=3)
                    .flat_map(|dz| (-3..=3).map(move |dx| (dx, dz)))
                    .map(|(dx, dz)| wet_top(&g.ground(x + dx, z + dz).1))
                    .max()
                    .unwrap_or(i64::MIN);
                if s.height > highest + 8 {
                    slots.push((x, z, s.height - n.level));
                }
            }
            (banks, slots)
        };
        // The default world's largest rivers. (Until generator version 9
        // this was the owner's own view of the slot, at a place the
        // tectonic framework has since moved.)
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut largest: Vec<(f32, usize, usize)> = (0..nz)
            .flat_map(|j| (0..nx).map(move |i| (i, j)))
            .filter(|&(i, j)| !d.is_sea(i, j) && !d.is_lake(i, j))
            .map(|(i, j)| (d.cell(i, j).2, i, j))
            .collect();
        largest.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut here = largest.iter().take(6).flat_map(|&(_, i, j)| {
            let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
            (z0..z0 + ch as i64).flat_map(move |z| (x0..x0 + cw as i64).map(move |x| (x, z)))
        });
        let (banks, found) = slots(&g, &mut here);
        assert!(
            banks > 100,
            "only {banks} bank columns on the largest rivers"
        );
        assert!(
            found.len() * 200 < banks as usize,
            "{} of {banks} banks on the largest rivers stand 9+ above them, e.g. {:?}",
            found.len(),
            &found[..found.len().min(8)]
        );
        // Rivers of every size.
        let g = Generator::new(0x41_7E45, WorldShape::new(65_536, 65_536).unwrap());
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut cells = (0..nz)
            .step_by(3)
            .flat_map(|j| (0..nx).step_by(3).map(move |i| (i, j)));
        let mut all = Vec::new();
        for (i, j) in &mut cells {
            let (_, _, q, _) = d.cell(i, j);
            if q < drainage::STREAM_FROM_M3S || d.is_sea(i, j) || d.is_lake(i, j) {
                continue;
            }
            let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
            all.extend(
                (z0..z0 + ch as i64)
                    .flat_map(|z| (x0..x0 + cw as i64).step_by(2).map(move |x| (x, z))),
            );
        }
        let (banks, found) = slots(&g, &mut all.into_iter());
        eprintln!("{banks} bank columns, {} in slots", found.len());
        assert!(banks > 20_000, "only {banks} bank columns");
        assert!(
            found.len() * 200 < banks as usize,
            "{} of {banks} banks stand 9+ above their river, e.g. {:?}",
            found.len(),
            &found[..found.len().min(8)]
        );
    }

    /// The sea is only by the sea (the owner's third flight, M14): a lake at
    /// sea level in a hot desert, far from the coast, had a ring of "rocky
    /// coast" stone round it, coast and sea being classed by height alone;
    /// and an inland basin below sea level, which the sea's rule fills with
    /// water, had a dry river carried across it on a causeway.
    ///
    /// Until generator version 9 this looked at the owner's two VIEWs.
    /// The tectonic framework moved those places, and gave the world many
    /// more like the first — a rift's floor drops below sea level far
    /// inland — so now it looks everywhere deep inside a continent, at all
    /// ground that began below sea level: no coast or sea biome there.
    ///
    /// The second view's check, that ground below sea level is never
    /// raised, was true of that basin and is not true in general: M14
    /// fills hollows with sediment to their river's grade, and a rift is
    /// a hollow. What it guarded — a river built out across standing
    /// water as a causeway — is `rivers_end_at_still_water_and_flats_are_cut`
    /// and `rivers_build_no_walls`, which look at every river.
    #[test]
    fn the_sea_is_only_by_the_sea() {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        let size = WorldShape::DEFAULT.size_x();
        let (mut inland, mut below) = (0u32, 0u32);
        let mut by_the_sea = Vec::new();
        for k in 0..400_000i64 {
            let (x, z) = (k * 7_919 % size, k * 104_729 % size);
            // Three kilometres inside continental crust: farther than any
            // coast wanders from its edge.
            if g.tectonics().continentalness(x, z) < 3_000.0 {
                continue;
            }
            inland += 1;
            if g.eroded_surface(x, z).height >= 0 {
                continue;
            }
            below += 1;
            let biome = g.column(x, z).biome;
            if biome.is_coast() || biome.is_sea() {
                by_the_sea.push((x, z, biome));
            }
        }
        assert!(
            inland > 20_000 && below > 100,
            "{below} of {inland} inland columns began below the sea"
        );
        assert!(
            by_the_sea.is_empty(),
            "{} of {below}: coast or sea inland: {:?}",
            by_the_sea.len(),
            &by_the_sea[..by_the_sea.len().min(8)]
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
        let e = Generator::new(123, WorldShape::DEFAULT).elevation().clone();
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
        // Water (M14): scattered columns almost never land on a river, so a
        // change to channels, banks, lakes, waterfalls or wetlands moved no
        // pin. Sites chosen from the network itself — every kind of river,
        // waterfalls, lakes, closed basins — each sampled across its banks.
        let d = g.drainage();
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let mut sites: Vec<(usize, usize)> = Vec::new();
        // Four kinds, waterfalls, lakes, sinks, large mouths (deltas), wide
        // lowland rivers (floodplains, oxbows).
        let mut wanted = [8, 8, 8, 8, 8, 8, 8, 8, 32usize]; // oxbows are patchy
        for j in (0..nz).step_by(3) {
            for i in (0..nx).step_by(3) {
                let (_, _, q, (ri, rj)) = d.cell(i, j);
                let slot = if !d.is_sea(i, j) && d.is_sea(ri, rj) && q >= 0.5 {
                    Some(7)
                } else if q >= 1.0 && d.river_kind_at(i, j) == Some(drainage::RiverKind::Meandering)
                {
                    Some(8)
                } else if d.waterfall_at(i, j) {
                    Some(4)
                } else if let Some(k) = d.river_kind_at(i, j) {
                    Some(k as usize)
                } else if d.is_sink(i, j) {
                    Some(6)
                } else if d.is_lake(i, j) {
                    Some(5)
                } else {
                    None
                };
                if let Some(k) = slot
                    && wanted[k] > 0
                {
                    wanted[k] -= 1;
                    sites.push((i, j));
                }
            }
        }
        for (i, j) in sites {
            let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
            for z in (z0 - 32..z0 + 160).step_by(3) {
                for x in (x0 - 32..x0 + 160).step_by(3) {
                    let (sf, w) = g.ground(x, z);
                    eat(&sf.height.to_le_bytes());
                    eat(&wet_top(&w).to_le_bytes());
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
        const PINNED: (u32, u64) = (9, 0x70e2_8e98_4d73_d901);
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

    /// Tool: what the real callers pay (`cargo test -p vox-worldgen
    /// --release generation_cost -- --ignored --nocapture`): generating
    /// the surface chunks of a chunk column, cold, and building one LOD
    /// node's surface at a near, a middle and a far level. Best of five.
    #[test]
    #[ignore]
    fn generation_cost() {
        let g = Generator::new(0x0007_E22A_C0DE, WorldShape::DEFAULT);
        g.drainage();
        g.climate();
        let places: Vec<(i64, i64)> = (0..48i64)
            .map(|k| (k * 7_919 % 6_400, k * 104_729 % 6_400))
            .collect();
        let best = |run: &dyn Fn() -> u64| {
            let mut best = f64::MAX;
            let mut sum = 0;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                sum = run();
                best = best.min(start.elapsed().as_secs_f64() * 1e3);
            }
            (best, sum)
        };
        let (ms, sum) = best(&|| {
            let g = Generator {
                grounds: Arc::new(GroundCache::default()),
                ..g.clone()
            };
            let mut sum = 0u64;
            for &(cx, cz) in &places {
                let (lo, hi) = g.surface_span_chunks(cx, cz);
                for cy in lo..=hi {
                    sum += u64::from(
                        g.generate_chunk(ChunkPos::new(cx, cy, cz))
                            .get(LocalPos::new(1, 2, 3))
                            .0,
                    );
                }
            }
            sum
        });
        println!(
            "surface chunks of a column  {:7.3} ms   (checksum {sum})",
            ms / places.len() as f64
        );
        for stride in [2i64, 16, 128] {
            let (ms, sum) = best(&|| {
                let g = Generator {
                    grounds: Arc::new(GroundCache::default()),
                    ..g.clone()
                };
                places
                    .iter()
                    .take(24)
                    .map(|&(cx, cz)| {
                        let (h, _, _) =
                            g.lod_surface(cx * 32, cz * 32, stride, LodSampling::Sparse);
                        h.iter()
                            .map(|&v| v as i64 as u64)
                            .fold(0u64, u64::wrapping_add)
                    })
                    .fold(0u64, u64::wrapping_add)
            });
            println!(
                "LOD node, stride {stride:3}        {:7.3} ms   (checksum {sum})",
                ms / 24.0
            );
        }
    }

    /// Tool: what creating a world costs, stage by stage (`cargo test -p
    /// vox-worldgen --release creation_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn creation_cost() {
        for seed in [0x0007_E22A_C0DE_u64, 1, 2] {
            let start = std::time::Instant::now();
            let g = Generator::new(seed, WorldShape::DEFAULT);
            let elevation = start.elapsed().as_secs_f64();
            g.climate();
            let climate = start.elapsed().as_secs_f64() - elevation;
            g.drainage();
            let drainage = start.elapsed().as_secs_f64() - elevation - climate;
            println!(
                "seed {seed:#x}: elevation {elevation:.2} s, climate {climate:.2} s, drainage {drainage:.2} s"
            );
        }
    }
}
