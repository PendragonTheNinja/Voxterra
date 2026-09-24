//! Multi-level LOD ring selection (Milestone 09, ADR-0008).
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
//! exactly 4x4 stride-2 nodes. Nesting is what makes the level partition exact
//! rather than approximate — a coarse node is never half-covered by a finer
//! level, which is the failure that produces double terrain and z-fighting.
//!
//! ## The partition (the "named trap", now at every boundary)
//!
//! Each level owns a square annulus measured in **chunks** around a *snapped
//! center*: the camera's chunk floored to the **coarsest** stride. Snapping is
//! essential — if each level centered on its own grid, the camera's differing
//! offset within a level-2 node vs a level-8 node would misalign their
//! boundaries and open gaps (or overlaps) between levels. One shared,
//! coarsest-aligned center makes every boundary land on a grid line of *every*
//! level, because the strides nest.
//!
//! Level `i` covers `[inner_i, outer_i)` chunks from that center, with
//! `inner_i = outer_{i-1}`. A node belongs to its level iff its whole region
//! lies inside the outer square and entirely outside the inner one — so no node
//! is ever half-owned. The constructor enforces the alignment; a violation is a
//! programming error, not a tuning mistake.
//!
//! ## Where LOD begins, and why it underlaps
//!
//! `inner_0` is where LOD starts. Because the center is snapped, it can sit up
//! to `coarsest_stride - 1` chunks from the true camera position, so an
//! `inner_0` set flush against the full-resolution radius would leave a gap on
//! the far side. LOD therefore **underlaps**: `inner_0` is chosen small enough
//! (0 is always safe) that LOD covers everything full-res does and more, and
//! the full-res chunks simply draw on top (depth-biased). Wasted coarse nodes
//! under the near field are cheap; a hole in the world is not.
//!
//! Because the grids nest and boundaries align, the levels are provably
//! disjoint and gapless: a world column beyond the full-res radius is covered
//! by exactly one LOD node.
//!
//! ## Hysteresis
//!
//! Node unloading uses a slack margin beyond each level's outer edge, so a
//! camera drifting on a boundary doesn't thrash. Handover between levels stays
//! crisp: a region dropped by one level is picked up by its neighbour in the
//! same update, so it is always covered by exactly one level.

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
    /// Outer edge of this level, in chunks from the camera (exclusive).
    pub outer_chunks: i64,
}

#[derive(Debug, Clone, Copy)]
struct LevelCfg {
    stride: i64,
    inner_chunks: i64,
    outer_chunks: i64,
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

/// Tracks the loaded LOD-node set across levels and computes ring deltas.
pub struct LodRing {
    levels: Vec<LevelCfg>,
    loaded: HashSet<LodNodeId>,
    /// Extra chunks beyond a level's outer edge before a node is unloaded
    /// (hysteresis).
    unload_margin_chunks: i64,
    /// Coarsest stride; the shared center is snapped to this grid.
    coarsest_stride: i64,
    /// World vertical band that LOD must cover, in blocks (inclusive min,
    /// exclusive max). Nodes stack vertically to span it.
    world_y: (i64, i64),
    /// Snapped center (chunks) at the last recompute.
    last_center: Option<(i64, i64)>,
}

impl LodRing {
    /// Build a ring.
    ///
    /// - `lod_inner_chunks`: where LOD begins (see "underlaps" in the module
    ///   docs — `0` is always gap-safe).
    /// - `levels`: finest to coarsest, each with its stride and outer edge.
    /// - `unload_margin_chunks`: hysteresis slack.
    ///
    /// Panics if the configuration cannot produce an exact partition: strides
    /// must be >= 1, powers of two, and each a multiple of the previous; radii
    /// must strictly increase; and every boundary radius must be a multiple of
    /// the strides it borders (so no node straddles two levels). These are
    /// programming errors — a bad value would otherwise surface as double
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
            // Both edges must land on this level's grid lines, or a node would
            // straddle the boundary and be half-owned by two levels.
            assert!(
                inner % lvl.stride == 0,
                "level {i}: inner radius {inner} must be a multiple of stride {}",
                lvl.stride
            );
            assert!(
                lvl.outer_chunks % lvl.stride == 0,
                "level {i}: outer radius {} must be a multiple of stride {}",
                lvl.outer_chunks,
                lvl.stride
            );
            cfgs.push(LevelCfg {
                stride: lvl.stride,
                inner_chunks: inner,
                outer_chunks: lvl.outer_chunks,
            });
            inner = lvl.outer_chunks;
            prev_stride = lvl.stride;
        }

        let coarsest_stride = cfgs.last().expect("levels non-empty").stride;
        // The shared center is snapped to the coarsest grid; every boundary
        // radius must therefore also be a multiple of the coarsest stride, or
        // the annuli would not align with the finer grids after snapping.
        for (i, c) in cfgs.iter().enumerate() {
            assert!(
                c.inner_chunks % coarsest_stride == 0 && c.outer_chunks % coarsest_stride == 0,
                "level {i}: radii ({}, {}) must be multiples of the coarsest stride {coarsest_stride}",
                c.inner_chunks,
                c.outer_chunks
            );
        }
        assert!(
            unload_margin_chunks % coarsest_stride == 0,
            "unload margin must be a multiple of the coarsest stride {coarsest_stride}"
        );

        Self {
            levels: cfgs,
            loaded: HashSet::new(),
            unload_margin_chunks,
            coarsest_stride,
            world_y: world_y_blocks,
            last_center: None,
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

    /// Distance in BLOCKS from the snapped centre at which `level` hands over
    /// to the next coarser one — where geomorph must be complete (ADR-0009).
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

    /// The shared center all annuli are measured from: the camera's chunk
    /// floored to the coarsest grid. See the module docs on why this must be
    /// shared rather than per-level.
    #[inline]
    fn snapped_center(&self, cam_chunk: ChunkPos) -> (i64, i64) {
        let s = self.coarsest_stride;
        (cam_chunk.x.div_euclid(s) * s, cam_chunk.z.div_euclid(s) * s)
    }

    /// Is `id` in its level's annulus, with `extra` chunks of outer slack
    /// (0 for load, the hysteresis margin for unload)?
    ///
    /// Tested on the node's whole REGION, not its center: a node counts only if
    /// it lies entirely inside the outer square and entirely outside the inner
    /// one. Grid alignment guarantees these are the only two possibilities, so
    /// no node is ever half-owned by two levels.
    fn in_annulus(&self, id: LodNodeId, cam_chunk: ChunkPos, extra: i64) -> bool {
        let cfg = self.levels[id.level as usize];
        let s = cfg.stride;
        let (cx, cz) = self.snapped_center(cam_chunk);
        let (ox, oz) = (id.x * s, id.z * s);
        let outer = cfg.outer_chunks + extra;
        let inner = cfg.inner_chunks;

        let within_outer =
            ox >= cx - outer && ox + s <= cx + outer && oz >= cz - outer && oz + s <= cz + outer;
        // Outside the inner square if either axis clears it entirely. With
        // inner == 0 the "inner square" is empty, so nothing is excluded —
        // otherwise the node the camera stands in would be skipped, leaving a
        // hole directly underfoot.
        let outside_inner = inner == 0
            || ox >= cx + inner
            || ox + s <= cx - inner
            || oz >= cz + inner
            || oz + s <= cz - inner;
        within_outer && outside_inner
    }

    /// Compute the load/unload delta for the camera's chunk position. A no-op
    /// when the camera hasn't moved to a new finest-level node.
    pub fn update(&mut self, camera_chunk: ChunkPos) -> LodUpdate {
        let center = self.snapped_center(camera_chunk);
        if self.last_center == Some(center) {
            return LodUpdate::default();
        }
        self.last_center = Some(center);
        let (cx, cz) = center;

        let mut to_load = Vec::new();
        for (li, cfg) in self.levels.iter().enumerate() {
            let level = li as u32;
            let s = cfg.stride;
            // Node cells spanning this level's outer square.
            let lo_x = (cx - cfg.outer_chunks).div_euclid(s);
            let hi_x = (cx + cfg.outer_chunks).div_euclid(s);
            let lo_z = (cz - cfg.outer_chunks).div_euclid(s);
            let hi_z = (cz + cfg.outer_chunks).div_euclid(s);
            for nz in lo_z..=hi_z {
                for nx in lo_x..=hi_x {
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
            .filter(|&id| !self.in_annulus(id, camera_chunk, self.unload_margin_chunks))
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
        self.last_center = None;
    }
}

/// Surface heights for columns the player has CHANGED, kept sparse so that
/// *every* LOD level can honour edits.
///
/// ## Why this exists
///
/// A coarse cell's height is the MINIMUM real surface over the cell — the
/// safety contract enforced by `lod_heightfield_never_exceeds_real_terrain`:
/// coarse may sit below real ground (harmless, it is hidden) but never above
/// it (it pokes through). Seed sampling honours that for untouched terrain,
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
    #[should_panic(expected = "must be a multiple of stride")]
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
        let base = [(2i64, 16i64), (4, 32), (8, 64)];
        let base_margin = 8i64;
        for extra in 0..=2 {
            let mut levels: Vec<(i64, i64)> = base.to_vec();
            for _ in 0..extra {
                let (s, o) = *levels.last().unwrap();
                levels.push((s * 2, o * 2));
            }
            let coarsest = levels.last().unwrap().0;
            let round_up = |v: i64| ((v + coarsest - 1) / coarsest) * coarsest;
            let mut prev = 0i64;
            let built: Vec<LodLevel> = levels
                .iter()
                .map(|&(stride, outer)| {
                    let o = round_up(outer).max(prev + coarsest);
                    prev = o;
                    LodLevel {
                        stride,
                        outer_chunks: o,
                    }
                })
                .collect();
            // Must not panic for any reachable slider value.
            let ring = LodRing::new(0, &built, round_up(base_margin), TEST_Y);
            assert_eq!(ring.level_count(), built.len());
        }
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
}
