//! Multi-level LOD ring selection (Milestone 09, ADR-0008; per-level centring
//! ADR-0014).
//!
//! Pure bookkeeping, mirroring [`Streamer`](crate::streaming::Streamer): given
//! the camera's chunk, [`LodRing`] tracks which coarse LOD *nodes* are loaded
//! across several resolution levels and computes which to load and unload. It
//! owns no geometry and does no generation — *how* a node is filled (generate
//! coarse from seed, downsample from real chunks, mesh, upload) is the caller's
//! concern.
//!
//! ## Levels, nodes, and the nesting property
//!
//! A **level** is a resolution: `stride` chunks per node side (so one node
//! covers `stride × CHUNK_SIZE` blocks per side, and each of its 32³ cells
//! stands for a `stride`-block cube). Level 0 is the finest LOD, sitting just
//! outside the full-resolution region; each subsequent level is coarser and
//! further away. M08's single level is the `levels.len() == 1` case.
//!
//! Strides **must be powers of two, each a multiple of the previous**, so the
//! node grids *nest*: one stride-8 node is exactly 2x2 stride-4 nodes, which is
//! exactly 4x4 stride-2 nodes.
//!
//! ## The partition: one square per level (ADR-0014)
//!
//! Level `i` owns its **square** `Q_i` — half-width `outer_i` chunks around
//! the camera's chunk floored to that level's **snap**: the next coarser
//! level's stride, or its own for the coarsest. Its nodes are those inside
//! `Q_i` and outside `Q_{i-1}`, the square of the level inside it (for level 0,
//! the `lod_inner_chunks` square, snapped to level 0's own stride, or nothing).
//!
//! Exactness rests on alignment. `Q_i`'s edges lie on its snap grid — level
//! `i+1`'s grid, which is also level `i`'s — so a node of either level is
//! wholly inside or wholly outside it. And every square holds the one inside
//! it, from any camera position, because the constructor demands
//! `outer_i - outer_{i-1} >= snap_i - snap_{i-1}`: the most the two snapped
//! centres can differ. A column is then covered by exactly the first level
//! whose square holds it. No gaps (holes in the world), no overlaps (double
//! terrain and z-fighting).
//!
//! Until M11 every level shared ONE centre, snapped to the coarsest stride.
//! That was exact too, but put each level's edges up to a coarsest stride from
//! where they belong — 8 km at a 64 km horizon. Per-level snapping keeps level
//! `i` within one snap step of the camera, so the fine levels hug the player
//! however coarse the outermost becomes.
//!
//! ## Where LOD begins, and why it underlaps
//!
//! `inner_0` is where LOD starts. Because the squares are snapped, an edge can
//! sit up to a snap step from the true camera position, so an `inner_0` set
//! flush against the full-resolution radius would leave a gap on the far side.
//! LOD therefore **underlaps**: `inner_0` is chosen small enough (0 is always
//! safe) that LOD covers everything full-res does and more, and full-res draws
//! on top. Wasted coarse nodes under the near field are cheap; a hole in the
//! world is not.
//!
//! ## Hysteresis
//!
//! Node unloading uses slack beyond each level's outer edge — at least one
//! snap step, so a camera standing on a snap boundary doesn't reload a row of
//! nodes every time it crosses. Handover between levels stays crisp: a node
//! inside the finer level's square is dropped at once, its region picked up by
//! the finer level in the same update.

use std::collections::{HashMap, HashSet};

use crate::coords::{CHUNK_SIZE, ChunkPos};
use crate::planet::WorldShape;

/// A LOD node: its level plus its `(x, z)` cell in that level's node grid.
/// The grid is fixed to the world, not camera-relative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LodNodeId {
    pub level: u32,
    pub x: i64,
    pub z: i64,
}

impl LodNodeId {
    #[inline]
    pub const fn new(level: u32, x: i64, z: i64) -> Self {
        Self { level, x, z }
    }
}

/// One resolution level: how coarse, and how far out it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodLevel {
    /// Chunks per node side. Powers of two, each a multiple of the previous.
    pub stride: i64,
    /// Half-width of this level's square, in chunks around its snapped centre.
    pub outer_chunks: i64,
}

impl LodLevel {
    /// `base` extended by `extra` coarser levels, each doubling stride and
    /// radius, with every radius rounded onto the grid [`LodRing::new`]
    /// requires. The one place that rule is applied: the settings menu and the
    /// tests build their rings through it, so no reachable setting panics.
    ///
    /// Appending levels rather than scaling radii keeps the node count per
    /// level roughly constant, so view distance costs linearly in levels —
    /// scaling the radii alone is quadratic (x4 distance once produced 10 240
    /// nodes and 4.3 GB).
    pub fn extended(base: &[LodLevel], extra: u32, lod_inner_chunks: i64) -> Vec<LodLevel> {
        let mut levels = base.to_vec();
        for _ in 0..extra {
            let last = *levels.last().expect("at least one base level");
            levels.push(LodLevel {
                stride: last.stride * 2,
                outer_chunks: last.outer_chunks * 2,
            });
        }
        // Each outer edge goes onto its snap grid and at least one snap step
        // beyond the previous, which satisfies the nesting condition (the gap
        // need only be `snap_i - snap_{i-1}`).
        let mut prev = lod_inner_chunks;
        for i in 0..levels.len() {
            let snap = snap_of(&levels, i);
            let want = levels[i].outer_chunks.max(prev + snap);
            levels[i].outer_chunks = (want + snap - 1).div_euclid(snap) * snap;
            prev = levels[i].outer_chunks;
        }
        levels
    }
}

/// The grid level `i`'s square snaps to: the next coarser stride, so that
/// level's nodes never straddle the square's edge; the coarsest snaps to its
/// own.
fn snap_of(levels: &[LodLevel], i: usize) -> i64 {
    levels.get(i + 1).unwrap_or(&levels[i]).stride
}

#[derive(Debug, Clone, Copy)]
struct LevelCfg {
    stride: i64,
    inner_chunks: i64,
    outer_chunks: i64,
    /// Grid this level's square snaps to (see [`snap_of`]).
    snap: i64,
}

/// What the [`LodRing`] wants the caller to do this update.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LodUpdate {
    /// Nodes newly in range: build (generate or downsample), mesh, upload,
    /// then confirm via [`LodRing::mark_loaded`].
    pub to_load: Vec<LodNodeId>,
    /// Nodes now out of range, or taken over by another level: drop and
    /// confirm via [`LodRing::mark_unloaded`].
    pub to_unload: Vec<LodNodeId>,
}

impl LodUpdate {
    pub fn is_empty(&self) -> bool {
        self.to_load.is_empty() && self.to_unload.is_empty()
    }
}

/// A square of chunk columns: `[x0, x1) × [z0, z1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Square {
    x0: i64,
    x1: i64,
    z0: i64,
    z1: i64,
}

impl Square {
    /// Half-width `half` around `(cx, cz)` floored to the `snap` grid.
    fn snapped(cam: ChunkPos, snap: i64, half: i64) -> Self {
        let cx = cam.x.div_euclid(snap) * snap;
        let cz = cam.z.div_euclid(snap) * snap;
        Self {
            x0: cx - half,
            x1: cx + half,
            z0: cz - half,
            z1: cz + half,
        }
    }

    fn grown(self, by: i64) -> Self {
        Self {
            x0: self.x0 - by,
            x1: self.x1 + by,
            z0: self.z0 - by,
            z1: self.z1 + by,
        }
    }

    fn is_empty(self) -> bool {
        self.x0 >= self.x1
    }

    /// Whether the region `[ox, ox+s) × [oz, oz+s)` lies wholly inside.
    fn contains_region(self, ox: i64, oz: i64, s: i64) -> bool {
        ox >= self.x0 && ox + s <= self.x1 && oz >= self.z0 && oz + s <= self.z1
    }

    /// Whether that region lies wholly outside.
    fn misses_region(self, ox: i64, oz: i64, s: i64) -> bool {
        self.is_empty() || ox >= self.x1 || ox + s <= self.x0 || oz >= self.z1 || oz + s <= self.z0
    }
}

/// Tracks the loaded LOD-node set across levels and computes ring deltas.
pub struct LodRing {
    levels: Vec<LevelCfg>,
    loaded: HashSet<LodNodeId>,
    /// Extra chunks beyond a level's outer edge before a node is unloaded
    /// (hysteresis); each level uses at least its own snap step.
    unload_margin_chunks: i64,
    /// Coarsest stride.
    coarsest_stride: i64,
    /// World vertical band that LOD must cover, in blocks (inclusive min,
    /// exclusive max). Nodes stack vertically to span it.
    world_y: (i64, i64),
    /// Every square at the last recompute — the whole input to `update`
    /// besides the loaded set, so an unchanged list means nothing to do.
    last_squares: Option<Vec<Square>>,
}

impl LodRing {
    /// Build a ring.
    ///
    /// - `lod_inner_chunks`: where LOD begins (see "underlaps" in the module
    ///   docs — `0` is always gap-safe).
    /// - `levels`: finest to coarsest, each with its stride and outer edge.
    ///   [`LodLevel::extended`] produces valid ones.
    /// - `unload_margin_chunks`: hysteresis slack.
    ///
    /// Panics if the configuration cannot produce an exact partition: strides
    /// must be >= 1, powers of two, and each a multiple of the previous; each
    /// edge must lie on the grid it snaps to (`lod_inner_chunks` on level 0's
    /// stride, each `outer` on its level's snap); and each square must clear
    /// the one inside it by the nesting condition in the module docs. These
    /// are programming errors — a bad value would otherwise surface as double
    /// terrain or a gap at a level boundary.
    pub fn new(
        lod_inner_chunks: i64,
        levels: &[LodLevel],
        unload_margin_chunks: i64,
        world_y_blocks: (i64, i64),
    ) -> Self {
        assert!(
            world_y_blocks.0 < world_y_blocks.1,
            "world Y band must be non-empty"
        );
        assert!(!levels.is_empty(), "at least one LOD level required");
        assert!(lod_inner_chunks >= 0, "LOD inner radius must be >= 0");
        assert!(unload_margin_chunks > 0, "unload margin must be > 0");

        let mut cfgs = Vec::with_capacity(levels.len());
        let mut inner = lod_inner_chunks;
        let mut prev_stride = 0i64;
        for (i, lvl) in levels.iter().enumerate() {
            assert!(lvl.stride >= 1, "level {i}: stride must be >= 1");
            assert!(
                (lvl.stride as u64).is_power_of_two(),
                "level {i}: stride {} must be a power of two (grids must nest)",
                lvl.stride
            );
            if prev_stride > 0 {
                assert!(
                    lvl.stride >= prev_stride && lvl.stride % prev_stride == 0,
                    "level {i}: stride {} must be a multiple of the previous ({prev_stride})",
                    lvl.stride
                );
            }
            assert!(
                lvl.outer_chunks > inner,
                "level {i}: outer {} must exceed inner {inner}",
                lvl.outer_chunks
            );
            prev_stride = lvl.stride;
            cfgs.push(LevelCfg {
                stride: lvl.stride,
                inner_chunks: inner,
                outer_chunks: lvl.outer_chunks,
                snap: 0, // set below, once every stride is known
            });
            inner = lvl.outer_chunks;
        }
        for (i, c) in cfgs.iter_mut().enumerate() {
            c.snap = snap_of(levels, i);
        }
        // The inner square of level 0 snaps to level 0's own stride.
        let mut prev_snap = cfgs[0].stride;
        for (i, c) in cfgs.iter().enumerate() {
            // Both edges must land on this level's grid, or a node would
            // straddle a square's edge and be half-owned by two levels. The
            // inner edge is the previous square, on its snap (= this stride).
            assert!(
                c.inner_chunks % c.stride == 0,
                "level {i}: inner radius {} must be a multiple of stride {}",
                c.inner_chunks,
                c.stride
            );
            assert!(
                c.outer_chunks % c.snap == 0,
                "level {i}: outer radius {} must be a multiple of {} (the stride its square snaps to)",
                c.outer_chunks,
                c.snap
            );
            // Nesting: the square must hold the one inside it from every
            // camera position. Vacuous for an empty inner square.
            if c.inner_chunks > 0 {
                assert!(
                    c.outer_chunks - c.inner_chunks >= c.snap - prev_snap,
                    "level {i}: outer {} must clear inner {} by at least {} (snap {} vs {prev_snap})",
                    c.outer_chunks,
                    c.inner_chunks,
                    c.snap - prev_snap,
                    c.snap
                );
            }
            prev_snap = c.snap;
        }

        let coarsest_stride = cfgs.last().expect("levels non-empty").stride;
        Self {
            levels: cfgs,
            loaded: HashSet::new(),
            unload_margin_chunks,
            coarsest_stride,
            world_y: world_y_blocks,
            last_squares: None,
        }
    }

    /// Height in blocks of one cell at any level. A node is 32 cells tall and
    /// spans the entire world Y band, so this is independent of the horizontal
    /// stride — which is exactly what lets a fine level cover the full terrain
    /// height in ONE node (no vertical stacking, and therefore no node with
    /// terrain at its top boundary emitting unlit faces).
    #[inline]
    pub fn v_stride(&self) -> i64 {
        let span = self.world_y.1 - self.world_y.0;
        let cells = CHUNK_SIZE as i64;
        (span + cells - 1) / cells
    }

    /// Bottom of the world band, in blocks — every node's Y origin.
    #[inline]
    pub fn world_y_min(&self) -> i64 {
        self.world_y.0
    }

    /// Convenience: the M08-style single level.
    pub fn single_level(
        lod_inner_chunks: i64,
        stride: i64,
        outer_chunks: i64,
        unload_margin_chunks: i64,
        world_y_blocks: (i64, i64),
    ) -> Self {
        Self::new(
            lod_inner_chunks,
            &[LodLevel {
                stride,
                outer_chunks,
            }],
            unload_margin_chunks,
            world_y_blocks,
        )
    }

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Chunks per node side at `level`.
    #[inline]
    pub fn stride(&self, level: u32) -> i64 {
        self.levels[level as usize].stride
    }

    /// Distance in BLOCKS from `level`'s snapped centre at which it hands
    /// over to the next coarser one — where geomorph must be complete
    /// (ADR-0009). The shader measures from the camera, which is within one
    /// snap step of that centre.
    ///
    /// Returns `None` for the coarsest level, which has nothing to morph
    /// toward: the caller holds its morph factor at 0.
    #[inline]
    pub fn morph_end_blocks(&self, level: u32) -> Option<i64> {
        if level as usize + 1 >= self.levels.len() {
            return None;
        }
        Some(self.levels[level as usize].outer_chunks * CHUNK_SIZE as i64)
    }

    /// World size in blocks of one node side at `level`.
    #[inline]
    pub fn span_blocks(&self, level: u32) -> i64 {
        self.stride(level) * CHUNK_SIZE as i64
    }

    /// The node's origin chunk (its minimum corner) on the X/Z axes.
    #[inline]
    pub fn node_origin_chunk_xz(&self, id: LodNodeId) -> (i64, i64) {
        let s = self.stride(id.level);
        (id.x * s, id.z * s)
    }

    /// The node's origin in world blocks, all three axes. Y is always the
    /// bottom of the world band (one node per column).
    #[inline]
    pub fn node_origin_blocks(&self, id: LodNodeId) -> (i64, i64, i64) {
        let s = self.stride(id.level);
        (
            id.x * s * CHUNK_SIZE as i64,
            self.world_y.0,
            id.z * s * CHUNK_SIZE as i64,
        )
    }

    /// The node at `level` covering a chunk column.
    ///
    /// Used to invalidate LOD after a block edit: the coarse node still holds
    /// the pre-edit surface, and because LOD underlaps the full-resolution
    /// region, that stale surface shows through the hole the player just dug —
    /// a "ghost block" where the terrain used to be.
    #[inline]
    pub fn node_containing(&self, level: u32, chunk_x: i64, chunk_z: i64) -> LodNodeId {
        let s = self.stride(level);
        LodNodeId::new(level, chunk_x.div_euclid(s), chunk_z.div_euclid(s))
    }

    pub fn loaded(&self) -> &HashSet<LodNodeId> {
        &self.loaded
    }

    /// The coarsest stride, in chunks.
    pub fn coarsest_stride(&self) -> i64 {
        self.coarsest_stride
    }

    /// The step, in chunks, by which `level`'s inner edge moves as the camera
    /// travels: the snap of the square inside it, which is `level`'s own
    /// stride. A node retired at that edge can be up to this much nearer the
    /// camera by the time its replacement lands.
    pub fn inner_snap_chunks(&self, level: u32) -> i64 {
        self.stride(level)
    }

    /// The closest any node of `level` can come to the camera while that level
    /// owns it: the Chebyshev distance, in chunks, from the camera's chunk to
    /// the node's nearest chunk (0 = the camera stands in it).
    ///
    /// The square inside `level` has half-width `inner` around the camera
    /// floored to `level`'s stride `S`, so the camera sits in `[c, c + S - 1]`
    /// of its centre `c` and a node must clear `[c - inner, c + inner)`. The
    /// near side is the binding one: `inner - S + 1`. A level with no square
    /// inside it (inner 0) reaches the camera.
    ///
    /// This is what decides how carefully a level must be built. Only a level
    /// that can reach the full-resolution region has to honour the
    /// never-exceed contract exactly (ADR-0008, M10 amendment); one that
    /// cannot is always seen from further away than full-res reaches.
    pub fn closest_approach_chunks(&self, level: u32) -> i64 {
        let cfg = self.levels[level as usize];
        if cfg.inner_chunks == 0 {
            0
        } else {
            cfg.inner_chunks - cfg.stride + 1
        }
    }

    /// Level `i`'s square for a camera at `cam`; `i = -1` is the square inside
    /// level 0 (`lod_inner_chunks`, possibly empty).
    fn square(&self, i: isize, cam: ChunkPos) -> Square {
        if i < 0 {
            let first = self.levels[0];
            Square::snapped(cam, first.stride, first.inner_chunks)
        } else {
            let c = self.levels[i as usize];
            Square::snapped(cam, c.snap, c.outer_chunks)
        }
    }

    /// Is `id` in its level's annulus, with `extra` chunks of outer slack
    /// (0 for load, the hysteresis margin for unload)?
    ///
    /// Tested on the node's whole REGION: a node counts only if it lies
    /// entirely inside its level's square and entirely outside the square
    /// inside it. Alignment guarantees these are the only two possibilities,
    /// so no node is ever half-owned by two levels.
    fn in_annulus(&self, id: LodNodeId, cam_chunk: ChunkPos, extra: i64) -> bool {
        let level = id.level as isize;
        let s = self.levels[id.level as usize].stride;
        let (ox, oz) = (id.x * s, id.z * s);
        self.square(level, cam_chunk)
            .grown(extra)
            .contains_region(ox, oz, s)
            && self.square(level - 1, cam_chunk).misses_region(ox, oz, s)
    }

    /// Unload slack at `level`: the configured margin, but never less than
    /// the level's snap step, or a camera standing on a snap boundary would
    /// reload a row of nodes each time it crossed.
    fn unload_slack(&self, level: u32) -> i64 {
        self.unload_margin_chunks
            .max(self.levels[level as usize].snap)
    }

    /// Compute the load/unload delta for the camera's chunk position. A no-op
    /// when no level's square has moved.
    pub fn update(&mut self, camera_chunk: ChunkPos) -> LodUpdate {
        let squares: Vec<Square> = (-1..self.levels.len() as isize)
            .map(|i| self.square(i, camera_chunk))
            .collect();
        if self.last_squares.as_ref() == Some(&squares) {
            return LodUpdate::default();
        }
        self.last_squares = Some(squares);

        let mut to_load = Vec::new();
        for (li, cfg) in self.levels.iter().enumerate() {
            let level = li as u32;
            let s = cfg.stride;
            let sq = self.square(li as isize, camera_chunk);
            // Node cells spanning this level's square.
            for nz in sq.z0.div_euclid(s)..=(sq.z1 - 1).div_euclid(s) {
                for nx in sq.x0.div_euclid(s)..=(sq.x1 - 1).div_euclid(s) {
                    let id = LodNodeId::new(level, nx, nz);
                    if self.in_annulus(id, camera_chunk, 0) && !self.loaded.contains(&id) {
                        to_load.push(id);
                    }
                }
            }
        }

        // Unload anything no longer in its own annulus even with the slack:
        // too far, or now owned by a different level.
        let to_unload: Vec<LodNodeId> = self
            .loaded
            .iter()
            .copied()
            .filter(|&id| !self.in_annulus(id, camera_chunk, self.unload_slack(id.level)))
            .collect();

        LodUpdate { to_load, to_unload }
    }

    /// Confirm a node was loaded.
    pub fn mark_loaded(&mut self, id: LodNodeId) {
        self.loaded.insert(id);
    }

    /// Confirm a node was unloaded.
    pub fn mark_unloaded(&mut self, id: LodNodeId) {
        self.loaded.remove(&id);
    }

    /// Forget all loaded nodes (e.g. when LOD is toggled off). The next
    /// `update` re-requests everything.
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.last_squares = None;
    }
}

/// Surface heights for columns the player has CHANGED, kept sparse so that
/// *every* LOD level can honour edits.
///
/// ## Why this exists
///
/// Where LOD can meet full-resolution terrain, a coarse cell's height is the
/// MINIMUM real surface over the cell — the safety contract enforced by
/// `exact_lod_heightfield_never_exceeds_real_terrain` (ADR-0008, M10
/// amendment): coarse may sit below real ground (harmless, it is hidden) but
/// never above it (it pokes through). Seed sampling honours that for untouched terrain,
/// and untouched terrain is nearly all of it. A mined column is exactly where
/// seed and reality diverge, and the seed always reads *higher* — so the
/// coarse surface floats where the player dug, visible through the hole
/// because LOD underlaps the full-resolution region. That is the "ghost
/// block".
///
/// Reading real heights for every column of every node would fix it and cost
/// `32 x 32 x stride^2` map lookups per node — 65 536 at stride 8, nearly all
/// of them misses on columns no chunk has ever loaded. Edits are sparse, so
/// store only the edits and lower the affected cells afterwards. An untouched
/// world costs one `is_empty` check.
///
/// Bucketed by chunk column so a node visits only the edits inside its own
/// footprint, never the whole map.
///
/// ## Persisted with the world (M10 A3)
///
/// The overlay is the LOD's ONLY source of edits, at every level, so it must
/// outlive the session: an in-memory overlay forgets every earlier session's
/// digs, and the seed brings the ghost terrain straight back over them. It is
/// saved beside the chunks ([`WorldStore::save_edited_columns`]) in the format
/// of [`EditedColumns::serialize`].
///
/// [`WorldStore::save_edited_columns`]: crate::storage::WorldStore::save_edited_columns
#[derive(Debug, Clone)]
pub struct EditedColumns {
    /// Canonical chunk (x, z) -> canonical world (x, z) -> surface height.
    ///
    /// CANONICAL, because this is one of the three places the world's seam
    /// exists (ADR-0012, `planet` module docs). An edit made on one lap of the
    /// world has to reach the LOD on every lap, so it is stored under the
    /// canonical position and mapped back to whichever lap a node is on.
    by_chunk: HashMap<(i64, i64), HashMap<(i64, i64), i32>>,
    shape: WorldShape,
}

impl EditedColumns {
    /// An empty overlay for a world of the given shape. No `Default`: an
    /// overlay built for the wrong world size would silently misplace every
    /// edit across the seam.
    pub fn new(shape: WorldShape) -> Self {
        Self {
            by_chunk: HashMap::new(),
            shape,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_chunk.is_empty()
    }

    /// The world shape this overlay was built for.
    pub fn shape(&self) -> WorldShape {
        self.shape
    }

    /// Number of recorded columns.
    pub fn len(&self) -> usize {
        self.by_chunk.values().map(HashMap::len).sum()
    }

    /// Record a column's TRUE surface height (highest solid block), replacing
    /// any previous value. `x`/`z` may be on any lap of the world.
    ///
    /// Replace rather than min: the caller rescans the column, so this is the
    /// current truth, and refilling a hole must be able to undo the drop.
    pub fn record(&mut self, x: i64, z: i64, surface_y: i32) {
        let (x, z) = (self.shape.canonical_x(x), self.shape.canonical_z(z));
        let key = (
            x.div_euclid(CHUNK_SIZE as i64),
            z.div_euclid(CHUNK_SIZE as i64),
        );
        self.by_chunk
            .entry(key)
            .or_default()
            .insert((x, z), surface_y);
    }

    /// The recorded height for a column, if it has been edited. `x`/`z` may be
    /// on any lap of the world.
    pub fn get(&self, x: i64, z: i64) -> Option<i32> {
        let (x, z) = (self.shape.canonical_x(x), self.shape.canonical_z(z));
        let key = (
            x.div_euclid(CHUNK_SIZE as i64),
            z.div_euclid(CHUNK_SIZE as i64),
        );
        self.by_chunk.get(&key)?.get(&(x, z)).copied()
    }

    /// Lower this node's cells to account for the edits inside its footprint.
    ///
    /// `heights` is a `32 x 32` cell grid (row-major, Z-major) as produced by
    /// seed sampling; `origin_x`/`origin_z` are the node's world-block origin
    /// — unwrapped, on whatever lap the node is — and `h_stride` its
    /// blocks-per-cell. `floor_y` bounds the result: a column mined out
    /// entirely reports no terrain, and an unbounded sentinel would mesh a wall
    /// to negative infinity.
    ///
    /// Only lowering is applied. A cell already at or below the edit keeps its
    /// value, and a column the player built UP does not raise the cell —
    /// raising would break the never-exceed-real-terrain contract for the
    /// other columns sharing that cell.
    pub fn apply_to_node(
        &self,
        heights: &mut [i32],
        origin_x: i64,
        origin_z: i64,
        h_stride: i64,
        floor_y: i32,
    ) {
        if self.by_chunk.is_empty() {
            return;
        }
        let cells = CHUNK_SIZE as i64;
        debug_assert_eq!(heights.len(), (cells * cells) as usize);
        debug_assert!(h_stride >= 1);
        // A node side is `cells * h_stride` blocks, which is exactly `h_stride`
        // chunks, and its origin is chunk-aligned — so the footprint is a
        // h_stride x h_stride block of chunk columns.
        let c0x = origin_x.div_euclid(cells);
        let c0z = origin_z.div_euclid(cells);
        for cz in c0z..c0z + h_stride {
            for cx in c0x..c0x + h_stride {
                // Look the bucket up canonically, then carry each edit back to
                // the lap this footprint chunk is on.
                let canon = self.shape.canonical_chunk(ChunkPos::new(cx, 0, cz));
                let Some(bucket) = self.by_chunk.get(&(canon.x, canon.z)) else {
                    continue;
                };
                let (lap_x, lap_z) = ((cx - canon.x) * cells, (cz - canon.z) * cells);
                for (&(wx, wz), &h) in bucket {
                    let ix = (wx + lap_x - origin_x).div_euclid(h_stride);
                    let iz = (wz + lap_z - origin_z).div_euclid(h_stride);
                    if !(0..cells).contains(&ix) || !(0..cells).contains(&iz) {
                        continue;
                    }
                    let idx = (iz * cells + ix) as usize;
                    let h = h.max(floor_y);
                    if h < heights[idx] {
                        heights[idx] = h;
                    }
                }
            }
        }
    }
}

/// Magic at the start of a saved edit overlay.
const EDITS_MAGIC: [u8; 4] = *b"VXTE";

/// On-disk format version of the edit overlay. Independent of the chunk and
/// world-metadata versions.
pub const EDITS_FORMAT_VERSION: u8 = 1;

/// Header: magic, version, entry count (`u64`).
const EDITS_HEADER_LEN: usize = 4 + 1 + 8;

/// One entry: canonical x (`i64`), canonical z (`i64`), height (`i32`).
const EDITS_ENTRY_LEN: usize = 8 + 8 + 4;

/// Why a saved edit overlay failed to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditsDecodeError {
    /// Not an edit overlay.
    BadMagic,
    /// Written by a format this build does not understand.
    UnsupportedVersion(u8),
    /// Shorter than its header or its entry count says.
    Truncated,
    /// Longer than its entry count says.
    TrailingBytes,
    /// Entries out of order or repeated. The encoder writes them strictly
    /// ascending, so anything else is corruption.
    Unordered,
    /// A column outside the canonical range — from a world of another size,
    /// or corruption. Accepting it would misplace an edit across the seam.
    NotCanonical { x: i64, z: i64 },
}

impl std::fmt::Display for EditsDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not an edit overlay (bad magic)"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported edit overlay version {v}"),
            Self::Truncated => write!(f, "edit overlay is truncated"),
            Self::TrailingBytes => write!(f, "edit overlay has trailing bytes"),
            Self::Unordered => write!(f, "edit overlay entries are out of order"),
            Self::NotCanonical { x, z } => {
                write!(f, "edit overlay column ({x}, {z}) is outside the world")
            }
        }
    }
}

impl std::error::Error for EditsDecodeError {}

impl EditedColumns {
    /// Encode for saving.
    ///
    /// Layout (little-endian): magic `VXTE`, version `u8`, entry count `u64`,
    /// then per entry canonical x `i64`, canonical z `i64`, height `i32`, in
    /// strictly ascending `(x, z)` order. The order makes the encoding
    /// deterministic (identical overlays, identical bytes) and lets the
    /// decoder reject repeated or shuffled entries as corruption.
    ///
    /// The world size is not stored: the overlay lives inside a world whose
    /// `world.meta` records it, and every entry is checked against that shape
    /// on the way back in.
    pub fn serialize(&self) -> Vec<u8> {
        let mut entries: Vec<(i64, i64, i32)> = self
            .by_chunk
            .values()
            .flat_map(|bucket| bucket.iter().map(|(&(x, z), &h)| (x, z, h)))
            .collect();
        entries.sort_unstable_by_key(|&(x, z, _)| (x, z));
        let mut bytes = Vec::with_capacity(EDITS_HEADER_LEN + entries.len() * EDITS_ENTRY_LEN);
        bytes.extend_from_slice(&EDITS_MAGIC);
        bytes.push(EDITS_FORMAT_VERSION);
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for (x, z, h) in entries {
            bytes.extend_from_slice(&x.to_le_bytes());
            bytes.extend_from_slice(&z.to_le_bytes());
            bytes.extend_from_slice(&h.to_le_bytes());
        }
        bytes
    }

    /// Decode a saved overlay for a world of the given shape. Strict: any
    /// deviation from what [`EditedColumns::serialize`] writes is an error.
    pub fn deserialize(bytes: &[u8], shape: WorldShape) -> Result<Self, EditsDecodeError> {
        if bytes.len() < EDITS_HEADER_LEN {
            return Err(EditsDecodeError::Truncated);
        }
        if bytes[0..4] != EDITS_MAGIC {
            return Err(EditsDecodeError::BadMagic);
        }
        if bytes[4] != EDITS_FORMAT_VERSION {
            return Err(EditsDecodeError::UnsupportedVersion(bytes[4]));
        }
        let count = u64::from_le_bytes(bytes[5..13].try_into().expect("8 bytes"));
        // Checked: a corrupt count must not overflow into a plausible length.
        let body_len = usize::try_from(count)
            .ok()
            .and_then(|n| n.checked_mul(EDITS_ENTRY_LEN))
            .ok_or(EditsDecodeError::Truncated)?;
        let body = &bytes[EDITS_HEADER_LEN..];
        match body.len().cmp(&body_len) {
            std::cmp::Ordering::Less => return Err(EditsDecodeError::Truncated),
            std::cmp::Ordering::Greater => return Err(EditsDecodeError::TrailingBytes),
            std::cmp::Ordering::Equal => {}
        }

        let mut edits = Self::new(shape);
        let mut previous: Option<(i64, i64)> = None;
        for entry in body.chunks_exact(EDITS_ENTRY_LEN) {
            let x = i64::from_le_bytes(entry[0..8].try_into().expect("8 bytes"));
            let z = i64::from_le_bytes(entry[8..16].try_into().expect("8 bytes"));
            let h = i32::from_le_bytes(entry[16..20].try_into().expect("4 bytes"));
            if shape.canonical_x(x) != x || shape.canonical_z(z) != z {
                return Err(EditsDecodeError::NotCanonical { x, z });
            }
            if previous.is_some_and(|p| p >= (x, z)) {
                return Err(EditsDecodeError::Unordered);
            }
            previous = Some((x, z));
            edits.record(x, z, h);
        }
        Ok(edits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(cx: i64, cz: i64) -> ChunkPos {
        ChunkPos::new(cx, 0, cz)
    }

    /// Chebyshev distance in chunks from `cam` to the NEAREST chunk of a node's
    /// region — the value the annulus bounds are expressed in.
    fn nearest_chunk_dist(ring: &LodRing, id: LodNodeId, cam: ChunkPos) -> i64 {
        let s = ring.stride(id.level);
        let (ox, oz) = ring.node_origin_chunk_xz(id);
        let axis = |lo: i64, c: i64| -> i64 {
            let hi = lo + s - 1;
            if c < lo {
                lo - c
            } else if c > hi {
                c - hi
            } else {
                0
            }
        };
        axis(ox, cam.x).max(axis(oz, cam.z))
    }

    /// Three nested levels, LOD underlapping from the centre (gap-safe):
    /// stride 2 out to 16 chunks, stride 4 to 32, stride 8 to 64.
    /// Terrain band the tests cover vertically (the placeholder worldgen spans
    /// about -59..108 blocks).
    const TEST_Y: (i64, i64) = (-128, 128);

    fn three_levels() -> LodRing {
        LodRing::new(
            0,
            &[
                LodLevel {
                    stride: 2,
                    outer_chunks: 16,
                },
                LodLevel {
                    stride: 4,
                    outer_chunks: 32,
                },
                LodLevel {
                    stride: 8,
                    outer_chunks: 64,
                },
            ],
            8,
            TEST_Y,
        )
    }

    /// THE milestone invariant: every chunk column beyond the full-res radius
    /// and inside the outermost level is covered by EXACTLY ONE node. No gaps
    /// (holes in the world), no overlaps (double terrain and z-fighting).
    #[test]
    fn levels_partition_exactly_no_gaps_no_overlaps() {
        let mut ring = three_levels();
        for id in ring.update(cp(0, 0)).to_load {
            ring.mark_loaded(id);
        }
        let outermost: i64 = 64;
        for cz in -outermost..outermost {
            for cx in -outermost..outermost {
                let covers = ring
                    .loaded()
                    .iter()
                    .filter(|id| {
                        let s = ring.stride(id.level);
                        let (ox, oz) = ring.node_origin_chunk_xz(**id);
                        cx >= ox && cx < ox + s && cz >= oz && cz < oz + s
                    })
                    .count();
                assert_eq!(
                    covers, 1,
                    "chunk ({cx},{cz}) covered by {covers} nodes, want 1"
                );
            }
        }
    }

    /// The partition holds from an arbitrary, non-origin camera position too
    /// (the grids are world-fixed, so this is a genuinely different alignment).
    #[test]
    fn partition_holds_off_origin() {
        let mut ring = three_levels();
        let cam = cp(37, -53);
        for id in ring.update(cam).to_load {
            ring.mark_loaded(id);
        }
        // Sample a window well inside the outermost level, allowing for the
        // snapped centre being up to coarsest_stride-1 chunks from the camera.
        for dz in -50..50 {
            for dx in -50..50 {
                let (cx, cz) = (cam.x + dx, cam.z + dz);
                let covers = ring
                    .loaded()
                    .iter()
                    .filter(|id| {
                        let s = ring.stride(id.level);
                        let (ox, oz) = ring.node_origin_chunk_xz(**id);
                        cx >= ox && cx < ox + s && cz >= oz && cz < oz + s
                    })
                    .count();
                assert_eq!(
                    covers, 1,
                    "chunk ({cx},{cz}) covered by {covers} nodes, want 1"
                );
            }
        }
    }

    /// Each level's nodes sit only within that level's annulus. Measured from
    /// the snapped centre, which for a camera at the origin is the origin.
    #[test]
    fn each_level_owns_its_annulus() {
        let mut ring = three_levels();
        let cam = cp(0, 0);
        let bounds = [(0i64, 16i64), (16, 32), (32, 64)];
        for id in ring.update(cam).to_load {
            let near = nearest_chunk_dist(&ring, id, cam);
            let (lo, hi) = bounds[id.level as usize];
            assert!(
                near >= lo && near < hi,
                "level {} node nearest {near} outside [{lo},{hi})",
                id.level
            );
        }
    }

    /// Coarser levels are strictly further away than finer ones.
    #[test]
    fn finer_levels_are_nearer() {
        let mut ring = three_levels();
        let cam = cp(0, 0);
        let mut farthest = [i64::MIN; 3];
        let mut nearest = [i64::MAX; 3];
        for id in ring.update(cam).to_load {
            let d = nearest_chunk_dist(&ring, id, cam);
            let l = id.level as usize;
            farthest[l] = farthest[l].max(d);
            nearest[l] = nearest[l].min(d);
        }
        assert!(
            farthest[0] < nearest[1],
            "level 0 must end before level 1 starts"
        );
        assert!(
            farthest[1] < nearest[2],
            "level 1 must end before level 2 starts"
        );
        assert!(ring.stride(0) < ring.stride(1) && ring.stride(1) < ring.stride(2));
    }

    /// Standing still (moving within the finest node) is a no-op — no thrash.
    #[test]
    fn stationary_update_is_noop() {
        let mut ring = three_levels();
        for id in ring.update(cp(0, 0)).to_load {
            ring.mark_loaded(id);
        }
        assert!(ring.update(cp(1, 1)).is_empty(), "same finest node");
    }

    /// Moving one node streams a bounded delta, not the whole set.
    #[test]
    fn moving_streams_only_a_delta() {
        let mut ring = three_levels();
        let first = ring.update(cp(0, 0));
        let full = first.to_load.len();
        for id in first.to_load {
            ring.mark_loaded(id);
        }
        let next = ring.update(cp(2, 0));
        assert!(
            next.to_load.len() < full / 4,
            "delta {} vs full {full}",
            next.to_load.len()
        );
    }

    /// Nodes dropped on the move are genuinely out of their annulus, and
    /// nothing is requested that is already loaded (no double-draw window).
    #[test]
    fn handover_never_double_loads() {
        let mut ring = three_levels();
        for id in ring.update(cp(0, 0)).to_load {
            ring.mark_loaded(id);
        }
        let cam = cp(40, 0);
        let update = ring.update(cam);
        for id in &update.to_unload {
            assert!(!ring.in_annulus(*id, cam, ring.unload_margin_chunks));
        }
        for id in &update.to_load {
            assert!(
                !ring.loaded().contains(id),
                "re-requested an already loaded node"
            );
        }
    }

    /// Hysteresis: nodes inside the slack band are not dropped.
    #[test]
    fn outer_hysteresis_prevents_thrash() {
        let mut ring = LodRing::single_level(0, 8, 64, 16, TEST_Y);
        for id in ring.update(cp(0, 0)).to_load {
            ring.mark_loaded(id);
        }
        let cam = cp(8, 0);
        for id in &ring.update(cam).to_unload {
            assert!(
                !ring.in_annulus(*id, cam, 16),
                "dropped a node still in the slack band"
            );
        }
    }

    /// One node per column spans the entire world band at every level, so no
    /// node ever has terrain at its top boundary — the case that produced
    /// unlit black faces when nodes were stacked (no neighbour above to
    /// sample). Vertical cell height is independent of horizontal stride.
    #[test]
    fn one_node_spans_the_world_band() {
        let ring = three_levels();
        let cells = CHUNK_SIZE as i64;
        assert_eq!(ring.v_stride() * cells, TEST_Y.1 - TEST_Y.0);
        assert_eq!(ring.world_y_min(), TEST_Y.0);
        // Same vertical resolution regardless of how fine the level is.
        for level in 0..ring.level_count() as u32 {
            let (_, y, _) = ring.node_origin_blocks(LodNodeId::new(level, 0, 0));
            assert_eq!(y, TEST_Y.0, "level {level} must start at the band bottom");
        }
    }

    /// Each column is requested exactly once (no stack, no duplicates).
    #[test]
    fn each_column_requested_once() {
        let mut ring = three_levels();
        let update = ring.update(cp(0, 0));
        let unique: std::collections::HashSet<LodNodeId> = update.to_load.iter().copied().collect();
        assert_eq!(
            unique.len(),
            update.to_load.len(),
            "duplicate node requests"
        );
    }

    #[test]
    fn node_origins_are_grid_aligned() {
        let ring = three_levels();
        let id = LodNodeId::new(2, -1, 2); // stride 8
        assert_eq!(ring.node_origin_chunk_xz(id), (-8, 16));
        assert_eq!(
            ring.node_origin_blocks(id),
            (-8 * CHUNK_SIZE as i64, TEST_Y.0, 16 * CHUNK_SIZE as i64)
        );
        assert_eq!(ring.span_blocks(2), 8 * CHUNK_SIZE as i64);
    }

    #[test]
    fn single_level_matches_m08_shape() {
        let mut ring = LodRing::single_level(0, 8, 40, 8, TEST_Y);
        assert_eq!(ring.level_count(), 1);
        let update = ring.update(cp(0, 0));
        assert!(!update.to_load.is_empty());
        assert!(update.to_load.iter().all(|id| id.level == 0));
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn rejects_non_power_of_two_stride() {
        LodRing::single_level(0, 3, 24, 8, TEST_Y);
    }

    #[test]
    #[should_panic(expected = "must be a multiple of 4")]
    fn rejects_misaligned_outer_radius() {
        // 30 is not a multiple of stride 4 → nodes would straddle the edge.
        LodRing::single_level(0, 4, 30, 8, TEST_Y);
    }

    #[test]
    #[should_panic(expected = "must be a multiple of stride")]
    fn rejects_misaligned_inner_radius() {
        // LOD inner radius 6 is not a multiple of stride 4.
        LodRing::single_level(6, 4, 32, 8, TEST_Y);
    }

    #[test]
    #[should_panic(expected = "multiple of the previous")]
    fn rejects_non_nesting_strides() {
        // Coarse-then-fine does not nest.
        LodRing::new(
            0,
            &[
                LodLevel {
                    stride: 8,
                    outer_chunks: 16,
                },
                LodLevel {
                    stride: 4,
                    outer_chunks: 32,
                },
            ],
            8,
            TEST_Y,
        );
    }
    /// Reproduces the in-game bug: after a settings change the app throws the
    /// ring away and builds a new one at the SAME camera position. That fresh
    /// ring must re-request everything, or LOD never comes back until the
    /// player happens to cross a node boundary.
    #[test]
    fn fresh_ring_at_same_position_requests_everything() {
        let cam = cp(5, 7);
        let mut old = three_levels();
        for id in old.update(cam).to_load {
            old.mark_loaded(id);
        }
        assert!(!old.loaded().is_empty());

        // What apply_view_settings does: a brand new ring, same camera.
        let mut fresh = three_levels();
        let update = fresh.update(cam);
        assert!(
            !update.to_load.is_empty(),
            "fresh ring returned nothing; LOD would stay empty until the camera moved"
        );
    }

    /// With inner radius 0, the node the camera is standing in must still be
    /// requested — it is the one directly underfoot.
    #[test]
    fn camera_own_node_is_included_when_inner_is_zero() {
        let mut ring = three_levels();
        let cam = cp(0, 0);
        let update = ring.update(cam);
        let own = LodNodeId::new(0, 0, 0);
        assert!(
            update.to_load.contains(&own),
            "the node under the camera was never requested"
        );
    }

    /// Every configuration the settings menu can produce must construct
    /// without panicking. The ring asserts that radii AND the unload margin are
    /// multiples of the coarsest stride; adding a coarse level raises that
    /// stride, and a caller that rounds the radii but forgets the margin
    /// crashes the game from a slider drag.
    #[test]
    fn every_menu_reachable_config_constructs() {
        // Up to the eight levels (strides 2..256) M11's 64 km horizon needs.
        for extra in 0..=5 {
            let built = LodLevel::extended(&base_levels(), extra, 0);
            // Must not panic for any reachable slider value.
            let ring = LodRing::new(0, &built, 8, TEST_Y);
            assert_eq!(ring.level_count(), 3 + extra as usize);
        }
        // The shipped three levels come through unchanged.
        assert_eq!(LodLevel::extended(&base_levels(), 0, 0), base_levels());
    }

    /// The shipped base levels (vox-app's `LOD_LEVELS`).
    fn base_levels() -> Vec<LodLevel> {
        [(2, 16), (4, 32), (8, 64)]
            .into_iter()
            .map(|(stride, outer_chunks)| LodLevel {
                stride,
                outer_chunks,
            })
            .collect()
    }

    /// Eight levels, strides 2..256, out to 2 048 chunks (64 km).
    fn eight_levels() -> LodRing {
        LodRing::new(0, &LodLevel::extended(&base_levels(), 5, 0), 8, TEST_Y)
    }

    /// How many loaded nodes cover chunk column `(x, z)`, one lookup per level.
    fn coverage(ring: &LodRing, x: i64, z: i64) -> usize {
        (0..ring.level_count() as u32)
            .filter(|&l| ring.loaded().contains(&ring.node_containing(l, x, z)))
            .count()
    }

    /// Chunks per lap of the default world (ADR-0012): the ring works in the
    /// unwrapped frame, so positions several laps out must behave alike.
    const LAP: i64 = crate::planet::DEFAULT_WORLD_SIZE_BLOCKS / CHUNK_SIZE as i64;

    /// Criterion 1: every column within the horizon is covered by exactly one
    /// level, at many camera positions — every snap phase of the coarsest
    /// grid, negative coordinates, several laps out.
    #[test]
    fn eight_levels_partition_exactly_anywhere() {
        let cams = [
            cp(0, 0),
            cp(255, -1),
            cp(-257, 129),
            cp(37, -53),
            cp(3 * LAP + 191, -2 * LAP - 77),
            cp(-5 * LAP - 13, 4 * LAP + 250),
        ];
        for cam in cams {
            let mut ring = eight_levels();
            for id in ring.update(cam).to_load {
                ring.mark_loaded(id);
            }
            // The outermost square, exactly as the ring places it.
            let last = ring.levels[ring.level_count() - 1];
            let sq = Square::snapped(cam, last.snap, last.outer_chunks);
            let check = |x: i64, z: i64| {
                let n = coverage(&ring, x, z);
                assert_eq!(n, 1, "camera {cam:?}: column ({x},{z}) covered {n} times");
            };
            // Densely around the camera, where the fine levels meet ...
            for z in cam.z - 80..cam.z + 80 {
                for x in cam.x - 80..cam.x + 80 {
                    check(x, z);
                }
            }
            // ... every column on lines through the camera to the far edge,
            // crossing every level boundary on both sides ...
            for x in sq.x0..sq.x1 {
                check(x, cam.z);
            }
            for z in sq.z0..sq.z1 {
                check(cam.x, z);
            }
            // ... and a sparse lattice over the whole square.
            for z in (sq.z0..sq.z1).step_by(37) {
                for x in (sq.x0..sq.x1).step_by(37) {
                    check(x, z);
                }
            }
            // Just outside the square, nothing.
            assert_eq!(coverage(&ring, sq.x1, cam.z), 0);
            assert_eq!(coverage(&ring, cam.x, sq.z0 - 1), 0);
        }
    }

    /// The point of per-level centring (ADR-0014): a fine level stays within
    /// one snap step of the camera however coarse the outermost level is.
    /// Under the shared centre, a camera at x = 255 had every level centred
    /// on x = 0, its fine ring 255 chunks (8 km) off.
    #[test]
    fn fine_levels_hug_the_camera() {
        let mut ring = eight_levels();
        let cam = cp(255, -1);
        let update = ring.update(cam);
        let (outer0, snap0) = (ring.levels[0].outer_chunks, ring.levels[0].snap);
        for id in update.to_load.iter().filter(|id| id.level == 0) {
            let d = nearest_chunk_dist(&ring, *id, cam);
            assert!(
                d < outer0 + snap0,
                "level-0 node {id:?} is {d} chunks from the camera"
            );
        }
        // And level 0 reaches out to at least outer - snap on every side.
        for id in update.to_load {
            ring.mark_loaded(id);
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let r = outer0 - snap0;
            let (x, z) = (cam.x + dx * r, cam.z + dz * r);
            assert!(
                ring.loaded().contains(&ring.node_containing(0, x, z)),
                "level 0 does not reach ({x},{z})"
            );
        }
    }

    /// A camera standing on a coarse snap boundary must not reload the far
    /// rim every time it steps across: the unload slack is at least one snap
    /// step. (The inner edge, where a level hands over to the finer one, is
    /// crisp by design — a region inside the finer square is dropped at once
    /// so two levels never draw the same ground — so nodes there may move.)
    #[test]
    fn crossing_a_coarse_snap_boundary_back_and_forth_does_not_thrash() {
        let mut ring = eight_levels();
        let apply = |ring: &mut LodRing, cam: ChunkPos| {
            let u = ring.update(cam);
            for id in &u.to_unload {
                ring.mark_unloaded(*id);
            }
            for id in &u.to_load {
                ring.mark_loaded(*id);
            }
            u
        };
        apply(&mut ring, cp(255, 0));
        apply(&mut ring, cp(256, 0));
        let cam = cp(255, 0);
        let back = apply(&mut ring, cam);
        let coarse = ring.level_count() - 1;
        // Anything the step back moved lies near the inner edge, never out at
        // the rim: within the finer square's reach plus its snap.
        let reach = ring.levels[coarse - 1].outer_chunks + ring.levels[coarse - 1].snap;
        for id in back.to_load.iter().chain(&back.to_unload) {
            if id.level as usize == coarse {
                let d = nearest_chunk_dist(&ring, *id, cam);
                assert!(
                    d < reach,
                    "rim node {id:?} ({d} chunks out) moved on stepping back"
                );
            }
        }
        assert!(
            back.to_load.len() + back.to_unload.len() < ring.loaded().len() / 4,
            "stepping back moved most of the ring"
        );
    }

    /// Stepping one chunk moves only the levels whose snap was crossed, and
    /// the delta stays a small fraction of the ring.
    #[test]
    fn a_step_with_eight_levels_streams_a_small_delta() {
        let mut ring = eight_levels();
        let first = ring.update(cp(0, 0));
        let full = first.to_load.len();
        for id in first.to_load {
            ring.mark_loaded(id);
        }
        let next = ring.update(cp(-1, 0)); // crosses every snap grid at once
        assert!(
            next.to_load.len() < full / 4,
            "delta {} vs full {full}",
            next.to_load.len()
        );
        assert!(
            ring.update(cp(-2, 0)).is_empty(),
            "no square moved, yet the ring recomputed"
        );
    }

    // --- Player edits at every LOD level (M09 ghost-block fix) ---------------

    /// The three shipped levels, for resolving a column to its node at each.
    fn edit_test_ring() -> LodRing {
        LodRing::new(
            0,
            &[
                LodLevel {
                    stride: 2,
                    outer_chunks: 16,
                },
                LodLevel {
                    stride: 4,
                    outer_chunks: 32,
                },
                LodLevel {
                    stride: 8,
                    outer_chunks: 64,
                },
            ],
            8,
            TEST_Y,
        )
    }

    /// Build one node's cell grid the way the app does: flat seed heights,
    /// then the player's edits folded in.
    fn node_heights(
        ring: &LodRing,
        level: u32,
        col: (i64, i64),
        seed: i32,
        edits: &EditedColumns,
    ) -> (Vec<i32>, usize) {
        let cells = CHUNK_SIZE as i64;
        let id = ring.node_containing(level, col.0.div_euclid(cells), col.1.div_euclid(cells));
        let (ox, _oy, oz) = ring.node_origin_blocks(id);
        let stride = ring.stride(level);
        let mut heights = vec![seed; (cells * cells) as usize];
        edits.apply_to_node(&mut heights, ox, oz, stride, TEST_Y.0 as i32);
        let ix = (col.0 - ox).div_euclid(stride);
        let iz = (col.1 - oz).div_euclid(stride);
        let idx = (iz * cells + ix) as usize;
        (heights, idx)
    }

    /// THE reproduction. A mined column must lower the coarse surface at
    /// EVERY level, not just the finest.
    ///
    /// Before the fix, `lod_tick` gated real heights behind `if n.level == 0`,
    /// so strides 4 and 8 were built from the seed forever. The seed always
    /// reads higher than dug ground, so the coarse surface floated above the
    /// hole and showed through it — the reported "ghost block".
    #[test]
    fn a_mined_column_lowers_the_cell_at_every_level() {
        const SEED: i32 = 40;
        const DUG: i32 = 34;
        let col = (37i64, 70i64);
        let ring = edit_test_ring();
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(col.0, col.1, DUG);

        for level in 0..3u32 {
            let (heights, idx) = node_heights(&ring, level, col, SEED, &edits);
            assert_eq!(
                heights[idx],
                DUG,
                "level {level} (stride {}) kept the seed surface over a mined column",
                ring.stride(level)
            );
            let changed = heights.iter().filter(|&&h| h != SEED).count();
            assert_eq!(
                changed, 1,
                "level {level} disturbed cells it should not have"
            );
        }
    }

    /// An edit is visible on EVERY lap of the world (ADR-0012).
    ///
    /// The overlay is one of the three places the seam exists: an edit made at
    /// one unwrapped position must show up in a node covering the same place
    /// one lap east, one lap west, or straight across the seam from where it
    /// was recorded. Before the torus this was structurally impossible to get
    /// wrong; now it is the overlay's whole job.
    #[test]
    fn an_edit_is_seen_on_every_lap_of_the_world() {
        const SEED: i32 = 40;
        const DUG: i32 = 31;
        let lap = WorldShape::DEFAULT.size_x();
        let ring = edit_test_ring();
        // Recorded just WEST of the seam, via a negative unwrapped coordinate.
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(-5, 70, DUG);
        for col in [
            (-5i64, 70i64),     // where it was made
            (lap - 5, 70),      // the same place, canonically
            (-5 + 3 * lap, 70), // three laps east
            (-5 - 2 * lap, 70), // two laps west
            (-5, 70 + 4 * lap), // four laps north
        ] {
            for level in 0..3u32 {
                let (heights, idx) = node_heights(&ring, level, col, SEED, &edits);
                assert_eq!(
                    heights[idx], DUG,
                    "level {level}: the edit is missing at {col:?}"
                );
                assert_eq!(
                    heights.iter().filter(|&&h| h != SEED).count(),
                    1,
                    "level {level} at {col:?} disturbed cells it should not have"
                );
            }
        }
        assert_eq!(edits.get(lap - 5, 70), Some(DUG));
        assert_eq!(edits.get(-5 + 7 * lap, 70 - lap), Some(DUG));
        assert_eq!(
            edits.len(),
            1,
            "one place, one entry, however it was addressed"
        );
    }

    /// Negative world coordinates resolve to the same cell (div_euclid, not
    /// truncating division — CLAUDE.md's coordinate rule).
    #[test]
    fn edits_apply_in_negative_coordinates() {
        const SEED: i32 = 12;
        let col = (-37i64, -70i64);
        let ring = edit_test_ring();
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(col.0, col.1, 5);
        for level in 0..3u32 {
            let (heights, idx) = node_heights(&ring, level, col, SEED, &edits);
            assert_eq!(
                heights[idx], 5,
                "level {level} missed a negative-coord edit"
            );
            assert_eq!(heights.iter().filter(|&&h| h != SEED).count(), 1);
        }
    }

    /// An edit in a neighbouring node must not leak into this one.
    #[test]
    fn edits_outside_the_footprint_are_ignored() {
        let cells = CHUNK_SIZE as i64;
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(-1, -1, 0); // just outside the node at origin (0, 0)
        edits.record(cells * 8, 0, 0); // just past the widest node's far edge
        for stride in [2i64, 4, 8] {
            let mut heights = vec![50i32; (cells * cells) as usize];
            edits.apply_to_node(&mut heights, 0, 0, stride, -128);
            assert!(
                heights.iter().all(|&h| h == 50),
                "stride {stride} pulled in an edit from outside its footprint"
            );
        }
    }

    /// Building UP must not raise a cell: the cell stands for every column in
    /// it, and raising would push coarse terrain above the real ground of the
    /// neighbours sharing it (the never-exceed contract).
    #[test]
    fn a_placed_block_never_raises_a_cell() {
        let cells = CHUNK_SIZE as i64;
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(5, 5, 90);
        let mut heights = vec![40i32; (cells * cells) as usize];
        edits.apply_to_node(&mut heights, 0, 0, 4, -128);
        assert!(heights.iter().all(|&h| h == 40));
    }

    /// A column mined out completely clamps to the band floor rather than
    /// meshing a wall to the sentinel value.
    #[test]
    fn a_fully_mined_column_clamps_to_the_floor() {
        let cells = CHUNK_SIZE as i64;
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(5, 5, i32::MIN);
        let mut heights = vec![40i32; (cells * cells) as usize];
        edits.apply_to_node(&mut heights, 0, 0, 4, -128);
        assert_eq!(heights[(cells + 1) as usize], -128);
    }

    /// The common case — an untouched world — costs nothing and changes
    /// nothing.
    #[test]
    fn no_edits_is_a_no_op() {
        let cells = CHUNK_SIZE as i64;
        let edits = EditedColumns::new(WorldShape::DEFAULT);
        assert!(edits.is_empty());
        let mut heights = vec![7i32; (cells * cells) as usize];
        edits.apply_to_node(&mut heights, 0, 0, 8, -128);
        assert!(heights.iter().all(|&h| h == 7));
    }

    /// Re-recording a column replaces it, so refilling a hole restores the
    /// surface instead of leaving the dug height behind forever.
    #[test]
    fn recording_a_column_twice_keeps_the_latest() {
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(5, 5, 20);
        edits.record(5, 5, 30);
        assert_eq!(edits.get(5, 5), Some(30));
        assert_eq!(edits.len(), 1);
    }

    // ---- M10 A3: how close each level can come (drives LOD sampling) ----

    /// The bound must hold for EVERY camera position: a node closer than
    /// `closest_approach_chunks` would be one built sparsely where full-res
    /// could meet it. Sweeps cameras over two full coarsest periods, negative
    /// coordinates included, and checks every node each update requests.
    fn assert_closest_approach_holds(ring: &mut LodRing) {
        let s = ring.coarsest_stride();
        let mut attained = vec![i64::MAX; ring.level_count()];
        for cz in -s..s {
            for cx in -s..s {
                ring.clear();
                let cam = ChunkPos::new(cx, 0, cz);
                for id in ring.update(cam).to_load {
                    let d = nearest_chunk_dist(ring, id, cam);
                    let bound = ring.closest_approach_chunks(id.level);
                    assert!(
                        d >= bound,
                        "level {} node {id:?} is {d} chunks from camera {cam:?}, \
                         closer than the bound {bound}",
                        id.level
                    );
                    let a = &mut attained[id.level as usize];
                    *a = (*a).min(d);
                }
            }
        }
        // And it is tight: some camera position reaches it. A loose bound
        // would build levels exactly that never needed it.
        for (level, &a) in attained.iter().enumerate() {
            assert_eq!(
                a,
                ring.closest_approach_chunks(level as u32),
                "level {level}: bound is not attained"
            );
        }
    }

    #[test]
    fn closest_approach_is_a_tight_bound_for_three_levels() {
        let mut ring = three_levels();
        assert_closest_approach_holds(&mut ring);
    }

    /// Extra levels grow the coarsest stride, which loosens the snapping — the
    /// case where getting the bound wrong would matter most.
    #[test]
    fn closest_approach_is_a_tight_bound_with_a_coarse_snap() {
        let mut ring = LodRing::new(
            0,
            &[
                LodLevel {
                    stride: 2,
                    outer_chunks: 32,
                },
                LodLevel {
                    stride: 4,
                    outer_chunks: 64,
                },
                LodLevel {
                    stride: 8,
                    outer_chunks: 96,
                },
                LodLevel {
                    stride: 16,
                    outer_chunks: 128,
                },
                LodLevel {
                    stride: 32,
                    outer_chunks: 256,
                },
            ],
            32,
            TEST_Y,
        );
        assert_eq!(ring.closest_approach_chunks(0), 0);
        // Level 2's inner edge snaps to its own stride (8), not the coarsest
        // (32) as under the shared centre (ADR-0014).
        assert_eq!(ring.closest_approach_chunks(2), 64 - 8 + 1);
        assert_closest_approach_holds(&mut ring);
    }

    // ---- M10 A3: the overlay is saved with the world ----

    fn sample_edits() -> EditedColumns {
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(5, 5, 20);
        edits.record(-3, 7, -40); // canonicalised on record
        edits.record(40, -2, i32::MIN / 2);
        edits.record(1000, 64, 3);
        edits
    }

    fn all_entries(e: &EditedColumns) -> Vec<((i64, i64), i32)> {
        let mut v: Vec<_> = e
            .by_chunk
            .values()
            .flat_map(|b| b.iter().map(|(&k, &h)| (k, h)))
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn edits_roundtrip_exactly() {
        let edits = sample_edits();
        let back =
            EditedColumns::deserialize(&edits.serialize(), WorldShape::DEFAULT).expect("decodes");
        assert_eq!(all_entries(&back), all_entries(&edits));
        assert_eq!(back.len(), 4);
    }

    /// Identical overlays encode to identical bytes, whatever order the edits
    /// were made in — hash-map iteration order must not leak into the file.
    #[test]
    fn edits_encode_deterministically() {
        let mut a = EditedColumns::new(WorldShape::DEFAULT);
        let mut b = EditedColumns::new(WorldShape::DEFAULT);
        let cols = [(9, 1, 5), (-4, 2, 6), (300, -70, 7), (0, 0, 8)];
        for &(x, z, h) in &cols {
            a.record(x, z, h);
        }
        for &(x, z, h) in cols.iter().rev() {
            b.record(x, z, h);
        }
        assert_eq!(a.serialize(), b.serialize());
    }

    #[test]
    fn an_empty_overlay_roundtrips() {
        let empty = EditedColumns::new(WorldShape::DEFAULT);
        let back =
            EditedColumns::deserialize(&empty.serialize(), WorldShape::DEFAULT).expect("decodes");
        assert!(back.is_empty());
    }

    /// The seam, through the save: an edit decoded from disk must reach a node
    /// on another lap of the world, exactly as a live one does.
    #[test]
    fn a_saved_edit_reaches_every_lap() {
        let cells = CHUNK_SIZE as i64;
        let shape = WorldShape::DEFAULT;
        let mut edits = EditedColumns::new(shape);
        edits.record(5, 5, -10);
        let back = EditedColumns::deserialize(&edits.serialize(), shape).expect("decodes");
        let mut heights = vec![40i32; (cells * cells) as usize];
        // Stride 1: cell (5, 5) of a node one full world width east.
        back.apply_to_node(&mut heights, shape.size_x(), 0, 1, -128);
        assert_eq!(heights[(5 * cells + 5) as usize], -10);
    }

    #[test]
    fn edits_reject_corruption() {
        let good = sample_edits().serialize();
        let decode = |b: &[u8]| EditedColumns::deserialize(b, WorldShape::DEFAULT);

        assert_eq!(decode(&good[..3]).unwrap_err(), EditsDecodeError::Truncated);

        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(decode(&bad).unwrap_err(), EditsDecodeError::BadMagic);

        let mut bad = good.clone();
        bad[4] = 99;
        assert_eq!(
            decode(&bad).unwrap_err(),
            EditsDecodeError::UnsupportedVersion(99)
        );

        assert_eq!(
            decode(&good[..good.len() - 1]).unwrap_err(),
            EditsDecodeError::Truncated
        );

        let mut bad = good.clone();
        bad.push(0);
        assert_eq!(decode(&bad).unwrap_err(), EditsDecodeError::TrailingBytes);

        // A count so large it would overflow the length arithmetic.
        let mut bad = good.clone();
        bad[5..13].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode(&bad).unwrap_err(), EditsDecodeError::Truncated);
    }

    #[test]
    fn edits_reject_unordered_or_repeated_entries() {
        let mut edits = EditedColumns::new(WorldShape::DEFAULT);
        edits.record(1, 1, 5);
        edits.record(2, 2, 6);
        let mut bytes = edits.serialize();
        let first = EDITS_HEADER_LEN..EDITS_HEADER_LEN + EDITS_ENTRY_LEN;
        let second = EDITS_HEADER_LEN + EDITS_ENTRY_LEN..EDITS_HEADER_LEN + 2 * EDITS_ENTRY_LEN;
        let (a, b) = (
            bytes[first.clone()].to_vec(),
            bytes[second.clone()].to_vec(),
        );
        bytes[first.clone()].copy_from_slice(&b);
        bytes[second.clone()].copy_from_slice(&a);
        assert_eq!(
            EditedColumns::deserialize(&bytes, WorldShape::DEFAULT).unwrap_err(),
            EditsDecodeError::Unordered
        );
        // Repeated: the first entry twice.
        bytes[second].copy_from_slice(&a);
        bytes[first].copy_from_slice(&a);
        assert_eq!(
            EditedColumns::deserialize(&bytes, WorldShape::DEFAULT).unwrap_err(),
            EditsDecodeError::Unordered
        );
    }

    /// An overlay from a larger world has columns outside this one. Accepting
    /// them would fold each edit onto the wrong column across the seam.
    #[test]
    fn edits_from_a_larger_world_are_refused() {
        let small = WorldShape::DEFAULT;
        let big = WorldShape::new(small.size_x() * 2, small.size_z()).expect("legal size");
        let mut edits = EditedColumns::new(big);
        let x = small.size_x() + 10;
        edits.record(x, 3, 1);
        assert_eq!(
            EditedColumns::deserialize(&edits.serialize(), small).unwrap_err(),
            EditsDecodeError::NotCanonical { x, z: 3 }
        );
    }
}
