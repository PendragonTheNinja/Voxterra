//! Chunk streaming policy (Milestone 02).
//!
//! Pure bookkeeping: the [`Streamer`] tracks which chunk positions are
//! currently loaded and, given the camera's chunk, computes which chunks
//! should be loaded and which unloaded. It does NOT own chunks, generate
//! them, or touch [`World`](crate::World) — *how* a load request is
//! fulfilled (generate from seed, read from disk, do it async) is the
//! caller's concern (Milestone 02 task 3+). This separation keeps the
//! policy fully unit-testable without graphics, IO, or worldgen.
//!
//! ## Radii and hysteresis
//!
//! Two radii (in chunks), measured as Euclidean distance between chunk
//! positions:
//!
//! Streaming is CYLINDRICAL: `load_radius` is a horizontal radius. The world is
//! a heightfield, so what matters is how far away a column is, not how far
//! above it the camera is — with a sphere, flying a few hundred blocks up drops
//! the ground out of range and the terrain under you unloads.
//!
//! - `load_radius`: chunks within this HORIZONTAL distance should be loaded.
//! - `unload_radius` (> load_radius): chunks beyond this should be unloaded.
//!
//! ## Vertically, streaming FOLLOWS THE SURFACE (M10 task 2)
//!
//! Until M10 the vertical extent was a fixed band of chunk-Y, which worked only
//! because the world was 256 blocks tall — 8 chunk layers, cheap to keep loaded
//! everywhere. M10 opens the world to roughly −11 000 … +9 000 so that Everest
//! and ocean trenches both fit, which is 640 layers. Loading a full-height
//! cylinder at that scale is ~80× more chunks and the engine stops.
//!
//! The answer is not a bigger budget. Terrain is a surface, and a player
//! interacts with the part of a column near that surface — so each column loads
//! a window around its OWN terrain height rather than a band shared by the
//! whole world. Total world height then costs nothing: a trench column loads
//! chunks 300 layers down, a summit column loads chunks 250 layers up, and both
//! load the same handful of chunks.
//!
//! The caller supplies the surface as a **span** of chunk-Y per chunk column,
//! not a single height. A chunk column is 32 blocks wide and can contain a
//! cliff face spanning many chunk layers; asking for one height would load the
//! top of the cliff and leave a hole down its face.
//!
//! Chunks in the gap between the two radii are left in whatever state they
//! are already in. Without this hysteresis band, a camera sitting on a
//! chunk boundary would load and unload the same chunk on alternating
//! frames (thrashing). The gap must be at least one chunk wide.

use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::coords::{CHUNK_SIZE, ChunkPos};
use crate::planet;

/// What the [`Streamer`] wants the caller to do this update.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamUpdate {
    /// Chunks newly in range that the caller should load (generate or read
    /// from disk) and then confirm via [`Streamer::mark_loaded`].
    pub to_load: Vec<ChunkPos>,
    /// Chunks now out of range that the caller should unload (and, if
    /// modified, persist) and then confirm via [`Streamer::mark_unloaded`].
    pub to_unload: Vec<ChunkPos>,
}

impl StreamUpdate {
    pub fn is_empty(&self) -> bool {
        self.to_load.is_empty() && self.to_unload.is_empty()
    }
}

/// Tracks the loaded-chunk set and computes streaming deltas.
pub struct Streamer {
    loaded: HashSet<ChunkPos>,
    load_radius: i64,
    unload_radius: i64,
    /// Chunk layers kept loaded below and above each column's surface span.
    below_chunks: i64,
    above_chunks: i64,
    /// Camera chunk used for the last `update`; `update` is a no-op (returns
    /// empty) when the camera hasn't changed chunks and nothing else has.
    last_center: Option<ChunkPos>,
}

impl Streamer {
    /// Create a streamer that keeps `below_chunks` layers beneath and
    /// `above_chunks` layers above each column's terrain surface.
    ///
    /// Panics if `unload_radius <= load_radius` (the hysteresis band must be at
    /// least one chunk wide) or if either vertical margin is negative.
    pub fn surface_following(
        load_radius: i64,
        unload_radius: i64,
        below_chunks: i64,
        above_chunks: i64,
    ) -> Self {
        assert!(
            below_chunks >= 0 && above_chunks >= 0,
            "vertical margins must be non-negative (got {below_chunks}, {above_chunks})"
        );
        let mut s = Self::new(load_radius, unload_radius);
        s.below_chunks = below_chunks;
        s.above_chunks = above_chunks;
        s
    }

    pub fn new(load_radius: i64, unload_radius: i64) -> Self {
        assert!(
            load_radius >= 1 && unload_radius > load_radius,
            "need 1 <= load_radius < unload_radius (got {load_radius}, {unload_radius})"
        );
        Self {
            loaded: HashSet::new(),
            load_radius,
            unload_radius,
            below_chunks: load_radius,
            above_chunks: load_radius,
            last_center: None,
        }
    }

    /// Chunk layers kept below and above each column's surface span.
    pub fn vertical_margins(&self) -> (i64, i64) {
        (self.below_chunks, self.above_chunks)
    }

    /// The chunk-Y range this column should keep loaded, given its surface
    /// span. Clamped to the world's vertical bounds so a seabed column near the
    /// floor does not request chunks below the world.
    ///
    /// Public because several callers must agree with the streamer about which
    /// chunks will EVER exist for a column — a first-mesh gate waiting on a
    /// neighbour outside this window waits forever, and a LOD node checking
    /// residency outside it never suppresses. Both were real M09 defects when
    /// the equivalent judgement was made independently.
    pub fn window_for(&self, surface_span: (i64, i64)) -> (i64, i64) {
        let s = CHUNK_SIZE as i64;
        let (world_lo, world_hi) = (
            planet::WORLD_Y_MIN_BLOCKS / s,
            planet::WORLD_Y_MAX_BLOCKS / s - 1,
        );
        let lo = (surface_span.0 - self.below_chunks).clamp(world_lo, world_hi);
        let hi = (surface_span.1 + self.above_chunks).clamp(world_lo, world_hi);
        (lo, hi)
    }

    pub fn load_radius(&self) -> i64 {
        self.load_radius
    }

    pub fn unload_radius(&self) -> i64 {
        self.unload_radius
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.loaded.contains(&pos)
    }

    pub fn loaded(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.loaded.iter().copied()
    }

    /// Compute what to load/unload for the camera at chunk `center`.
    ///
    /// - `to_load`: chunks within `load_radius` not already loaded, sorted
    ///   nearest-first so the caller can prioritize chunks around the camera.
    /// - `to_unload`: loaded chunks beyond `unload_radius`.
    ///
    /// This does NOT mutate the loaded set — the caller confirms completion
    /// via [`Streamer::mark_loaded`] / [`Streamer::mark_unloaded`], so an
    /// async pipeline can fulfill loads over several frames without the
    /// streamer re-requesting them. Re-requesting is prevented by treating
    /// in-flight chunks as "to be loaded"; callers that fulfill
    /// asynchronously should mark a chunk loaded when its data is ready, and
    /// should avoid duplicate work by tracking their own in-flight set.
    /// `surface_span(cx, cz)` returns the INCLUSIVE chunk-Y span of terrain
    /// surface within that chunk column — lowest and highest surface chunk. It
    /// is a span rather than a height because a 32-block-wide column can hold a
    /// cliff face crossing many layers, and a single height would load the top
    /// of the cliff and leave a hole down its face.
    ///
    /// The query is called at most once per column per update (results are
    /// memoized), so a caller backing it with real worldgen pays per column,
    /// not per chunk.
    pub fn update(
        &mut self,
        center: ChunkPos,
        surface_span: impl Fn(i64, i64) -> (i64, i64),
    ) -> StreamUpdate {
        self.last_center = Some(center);

        let load_sq = self.load_radius * self.load_radius;
        let unload_sq = self.unload_radius * self.unload_radius;

        // One query per column, not per chunk: a column contributes several
        // loaded layers, and the unload scan revisits every one of them.
        let mut windows: HashMap<(i64, i64), (i64, i64)> = HashMap::new();
        let mut window_of = |cx: i64, cz: i64, this: &Self| -> (i64, i64) {
            *windows
                .entry((cx, cz))
                .or_insert_with(|| this.window_for(surface_span(cx, cz)))
        };

        // Unload: loaded chunks beyond the unload radius, outside their
        // column's window, or outside the world.
        //
        // No vertical hysteresis is needed, and adding it would be noise: a
        // column's window depends only on its own terrain, which does not
        // change as the camera moves, so a chunk cannot oscillate in and out of
        // it the way a horizontal radius makes chunks oscillate at its edge.
        let mut to_unload: Vec<ChunkPos> = Vec::new();
        for &p in &self.loaded {
            let (lo, hi) = window_of(p.x, p.z, self);
            if horiz_dist_sq(p, center) > unload_sq
                || p.y < lo
                || p.y > hi
                || !planet::contains_chunk(p)
            {
                to_unload.push(p);
            }
        }
        to_unload.sort_by_key(|&p| (p.x, p.y, p.z)); // deterministic order

        // Load: for each column in the horizontal disc, the layers around that
        // column's own surface. Not a Y range relative to the camera — climbing
        // above it would load nothing and the terrain under you would vanish.
        let r = self.load_radius;
        let mut to_load: Vec<ChunkPos> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                let (cx, cz) = (center.x + dx, center.z + dz);
                if horiz_dist_sq(
                    ChunkPos::new(cx, 0, cz),
                    ChunkPos::new(center.x, 0, center.z),
                ) > load_sq
                {
                    continue;
                }
                let (lo, hi) = window_of(cx, cz, self);
                for ny in lo..=hi {
                    let p = ChunkPos::new(cx, ny, cz);
                    if planet::contains_chunk(p) && !self.loaded.contains(&p) {
                        to_load.push(p);
                    }
                }
            }
        }
        // Nearest-first so the caller fills in chunks around the camera
        // before distant ones.
        to_load.sort_by_key(|&p| dist_sq(p, center));

        StreamUpdate { to_load, to_unload }
    }

    /// Confirm a chunk has been loaded (data is resident). Idempotent.
    pub fn mark_loaded(&mut self, pos: ChunkPos) {
        self.loaded.insert(pos);
    }

    /// Confirm a chunk has been unloaded (data released). Idempotent.
    pub fn mark_unloaded(&mut self, pos: ChunkPos) {
        self.loaded.remove(&pos);
    }

    /// Convenience: apply an update's loads and unloads immediately, as a
    /// synchronous caller (e.g. tests, or a simple single-threaded path)
    /// would. Async callers should instead confirm individually as work
    /// completes.
    pub fn apply(&mut self, update: &StreamUpdate) {
        for &p in &update.to_load {
            self.loaded.insert(p);
        }
        for &p in &update.to_unload {
            self.loaded.remove(&p);
        }
    }
}

/// Squared Euclidean distance between two chunk positions (in chunk units).
/// Squared to avoid a sqrt; compared against squared radii.
fn dist_sq(a: ChunkPos, b: ChunkPos) -> i64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

/// Squared HORIZONTAL distance in chunk units.
///
/// Streaming is cylindrical, not spherical (M09): the world is a heightfield,
/// so what matters is how far away a column is, not how far above it you are.
/// With a sphere, flying a few hundred blocks up drops the ground out of the
/// load radius and the whole world under you unloads — terrain visibly
/// vanishing beneath a flying camera. A cylinder keeps the terrain band loaded
/// no matter the altitude.
#[inline]
fn horiz_dist_sq(a: ChunkPos, b: ChunkPos) -> i64 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

/// Squared distance in chunk units, widened to avoid overflow at extreme world
/// coordinates (`i64` squares overflow past ~3e9). Used for batch ordering,
/// where correctness at the far edges matters more than the last cycle.
#[inline]
fn dist_sq_wide(a: ChunkPos, b: ChunkPos) -> i128 {
    let dx = (a.x as i128) - (b.x as i128);
    let dy = (a.y as i128) - (b.y as i128);
    let dz = (a.z as i128) - (b.z as i128);
    dx * dx + dy * dy + dz * dz
}

/// Select the `n` positions nearest `camera`, returned nearest-first.
///
/// **Why this exists (M09 task 1):** the streaming queues (relight, mesh, gen,
/// LOD) are hash sets, so draining them takes an arbitrary order. Under a deep
/// backlog that means work right next to the player can sit behind thousands of
/// distant entries — the visible symptom being freshly streamed chunks that
/// stay dark or unmeshed for seconds. Draining nearest-first makes the player's
/// immediate surroundings converge first, no matter how far behind the tail is.
///
/// **Cost:** `O(m log n)` over the `m` candidates with a bounded max-heap,
/// rather than `O(m log m)` for a full sort — the queues hold thousands of
/// entries while a batch is 8–64, so this matters at 144 Hz. Allocation is
/// bounded by `n`.
///
/// Ties are broken by position, so the result is deterministic and does not
/// depend on the iteration order of the caller's set (batches stay stable
/// frame to frame instead of shuffling).
pub fn nearest_first(
    positions: impl Iterator<Item = ChunkPos>,
    camera: ChunkPos,
    n: usize,
) -> Vec<ChunkPos> {
    if n == 0 {
        return Vec::new();
    }
    // Max-heap of the best `n` so far, keyed by (distance, position) so the
    // worst kept entry is on top and ties are ordered deterministically.
    let mut heap: BinaryHeap<(i128, i64, i64, i64)> = BinaryHeap::with_capacity(n + 1);
    for p in positions {
        let key = (dist_sq_wide(p, camera), p.x, p.y, p.z);
        if heap.len() < n {
            heap.push(key);
        } else if let Some(&worst) = heap.peek()
            && key < worst
        {
            heap.pop();
            heap.push(key);
        }
    }
    let mut out: Vec<(i128, i64, i64, i64)> = heap.into_vec();
    out.sort_unstable();
    out.into_iter()
        .map(|(_, x, y, z)| ChunkPos::new(x, y, z))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(x: i64, y: i64, z: i64) -> ChunkPos {
        ChunkPos::new(x, y, z)
    }

    /// Perfectly flat terrain at chunk-Y 0. The degenerate case, and the one
    /// that reproduces the pre-M10 fixed band.
    fn flat(_cx: i64, _cz: i64) -> (i64, i64) {
        (0, 0)
    }

    /// A long ramp: the surface climbs one chunk layer every four columns of X.
    /// Deep enough to prove the window tracks terrain rather than the camera.
    fn ramp(cx: i64, _cz: i64) -> (i64, i64) {
        let y = cx.div_euclid(4);
        (y, y)
    }

    #[test]
    #[should_panic]
    fn rejects_bad_radii() {
        Streamer::new(4, 4); // unload must exceed load
    }

    /// Flat terrain reduces to the old fixed band: a horizontal disc crossed
    /// with a constant vertical window.
    #[test]
    fn flat_terrain_loads_a_cylinder_around_origin() {
        let mut s = Streamer::surface_following(3, 5, 2, 2);
        let update = s.update(cp(0, 0, 0), flat);
        for &p in &update.to_load {
            let horiz = p.x * p.x + p.z * p.z;
            assert!(horiz <= 9, "loaded {p:?} outside horizontal radius");
            assert!((-2..=2).contains(&p.y), "loaded {p:?} outside the band");
        }
        assert!(update.to_load.contains(&cp(0, 0, 0)));
        assert!(update.to_unload.is_empty());
    }

    /// THE bug this replaced a sphere to fix: climbing far above the terrain
    /// must NOT unload it. With a sphere the ground left the load radius and
    /// the world visibly vanished beneath a flying camera.
    #[test]
    fn flying_high_keeps_the_ground_loaded() {
        let mut s = Streamer::surface_following(3, 5, 2, 2);
        let first = s.update(cp(0, 0, 0), flat);
        s.apply(&first);
        let ground = cp(0, 0, 0);
        assert!(s.is_loaded(ground));
        // Climb far above the surface window.
        let update = s.update(cp(0, 40, 0), flat);
        assert!(
            !update.to_unload.contains(&ground),
            "ground unloaded when the camera climbed"
        );
        assert!(s.is_loaded(ground));
    }

    #[test]
    fn moving_loads_leading_unloads_trailing() {
        let mut s = Streamer::surface_following(2, 4, 1, 1);
        let first = s.update(cp(0, 0, 0), flat);
        s.apply(&first);
        let before = s.loaded_count();
        let update = s.update(cp(6, 0, 0), flat);
        assert!(!update.to_load.is_empty(), "moving should load new chunks");
        assert!(
            !update.to_unload.is_empty(),
            "moving should unload old ones"
        );
        s.apply(&update);
        // Bounded: the set does not grow without limit as the camera travels.
        assert!(s.loaded_count() <= before * 2);
    }

    #[test]
    fn loaded_count_bounded_regardless_of_travel() {
        let mut s = Streamer::surface_following(2, 4, 1, 1);
        for step in 0..40 {
            let u = s.update(cp(step * 3, 0, step), flat);
            s.apply(&u);
        }
        // A cylinder of radius 4 (unload) x 3 layers is the hard ceiling.
        assert!(
            s.loaded_count() <= 9 * 9 * 3,
            "unbounded growth: {}",
            s.loaded_count()
        );
    }

    #[test]
    fn to_load_is_nearest_first() {
        let mut s = Streamer::surface_following(3, 5, 1, 1);
        let update = s.update(cp(10, 0, 10), flat);
        let mut prev = -1;
        for &p in &update.to_load {
            let d = dist_sq(p, cp(10, 0, 10));
            assert!(d >= prev, "to_load not sorted nearest-first");
            prev = d;
        }
        assert_eq!(update.to_load.first(), Some(&cp(10, 0, 10)));
    }

    #[test]
    fn works_in_deep_negative_coordinates() {
        let mut s = Streamer::surface_following(2, 4, 1, 1);
        // Deep in the negative quadrant, but inside the world (M10 bounds).
        let center = cp(-3_000, -300, -3_000);
        let deep = |_: i64, _: i64| (-300, -300);
        let update = s.update(center, deep);
        s.apply(&update);
        assert!(s.is_loaded(center));
        assert!(s.is_loaded(cp(-2_999, -300, -3_000)));
    }

    // ---- M10 task 2: surface-following vertical extent ----

    /// THE property. Every resident chunk sits within the margins of its OWN
    /// column's surface — not a band shared by the world. This is what makes a
    /// 640-layer world affordable: a trench column and a summit column each
    /// load the same handful of chunks, 600 layers apart.
    #[test]
    fn every_resident_chunk_tracks_its_own_column_surface() {
        let (below, above) = (2, 3);
        let mut s = Streamer::surface_following(6, 8, below, above);
        // Travel along the ramp so columns enter and leave from every side.
        for step in 0..30 {
            let cx = step * 2;
            let u = s.update(cp(cx, ramp(cx, 0).0, 0), ramp);
            s.apply(&u);
        }
        for p in s.loaded() {
            let (lo, hi) = ramp(p.x, p.z);
            assert!(
                p.y >= lo - below && p.y <= hi + above,
                "chunk {p:?} is outside its column's window {:?}",
                (lo - below, hi + above)
            );
        }
    }

    /// A 640-layer world must not cost 640 layers. With a surface window the
    /// resident set is bounded by the disc area times the window height,
    /// independent of how tall the world is or how far the terrain climbs.
    #[test]
    fn resident_count_is_independent_of_world_height() {
        let (r, below, above): (i64, i64, i64) = (4, 2, 2);
        let mut s = Streamer::surface_following(r, r + 2, below, above);
        // A surface that swings across hundreds of chunk layers.
        let wild = |cx: i64, _cz: i64| {
            let y = (cx * 37).rem_euclid(600) - 300;
            (y, y)
        };
        for step in 0..40 {
            let u = s.update(cp(step, wild(step, 0).0, 0), wild);
            s.apply(&u);
        }
        let disc = ((2 * (r + 2) + 1) * (2 * (r + 2) + 1)) as usize;
        let window = (below + above + 1) as usize;
        assert!(
            s.loaded_count() <= disc * window,
            "resident set {} exceeds the disc x window ceiling {}",
            s.loaded_count(),
            disc * window
        );
    }

    /// A chunk column 32 blocks wide can hold a cliff face crossing many
    /// layers. The query returns a SPAN for exactly this reason — asking for a
    /// single height would load the cliff top and leave a hole down its face.
    #[test]
    fn a_cliff_column_loads_its_whole_face() {
        let mut s = Streamer::surface_following(2, 4, 1, 1);
        // One column is a 10-layer cliff; its neighbours are flat.
        let cliff = |cx: i64, _cz: i64| if cx == 1 { (0, 9) } else { (0, 0) };
        let u = s.update(cp(0, 0, 0), cliff);
        s.apply(&u);
        for y in 0..=9 {
            assert!(
                s.is_loaded(cp(1, y, 0)),
                "layer {y} of the cliff face was never loaded"
            );
        }
    }

    /// The window clamps to the world's vertical bounds, so a seabed column
    /// near the floor does not request chunks below the world.
    #[test]
    fn the_window_clamps_to_the_world_floor_and_ceiling() {
        let mut s = Streamer::surface_following(2, 4, 8, 8);
        let sc = CHUNK_SIZE as i64;
        let floor = planet::WORLD_Y_MIN_BLOCKS / sc;
        let ceiling = planet::WORLD_Y_MAX_BLOCKS / sc - 1;
        for surface in [floor, ceiling] {
            let mut s2 = Streamer::surface_following(2, 4, 8, 8);
            let u = s2.update(cp(0, surface, 0), |_, _| (surface, surface));
            for p in &u.to_load {
                assert!(
                    (floor..=ceiling).contains(&p.y),
                    "requested {p:?} outside the world's vertical bounds"
                );
            }
        }
        let u = s.update(cp(0, floor, 0), |_, _| (floor, floor));
        assert!(!u.to_load.is_empty(), "the floor column loaded nothing");
    }

    /// Chunks outside the world are never requested, and any that somehow
    /// became resident are released.
    #[test]
    fn chunks_outside_the_world_are_never_requested() {
        let sc = CHUNK_SIZE as i64;
        let edge = planet::WORLD_HALF_EXTENT_BLOCKS / sc - 1;
        let mut s = Streamer::surface_following(4, 6, 2, 2);
        let u = s.update(cp(edge, 0, edge), flat);
        for p in &u.to_load {
            assert!(
                planet::contains_chunk(*p),
                "requested {p:?} outside the world"
            );
        }
        s.apply(&u);
        assert!(s.loaded().all(planet::contains_chunk));
    }

    /// A column's window depends only on its own terrain, never on the camera,
    /// so a camera sitting still must produce no vertical churn at all — and a
    /// camera oscillating across a chunk boundary must not thrash either.
    #[test]
    fn a_settled_camera_produces_no_further_work() {
        let mut s = Streamer::surface_following(3, 5, 2, 2);
        let first = s.update(cp(0, 0, 0), ramp);
        s.apply(&first);
        let second = s.update(cp(0, 0, 0), ramp);
        assert!(
            second.is_empty(),
            "settled camera still churning: {second:?}"
        );

        // Oscillate across a boundary; the hysteresis band must absorb it.
        for _ in 0..8 {
            let a = s.update(cp(1, 0, 0), ramp);
            s.apply(&a);
            let b = s.update(cp(0, 0, 0), ramp);
            s.apply(&b);
            assert!(
                b.to_unload.is_empty(),
                "oscillating camera unloaded chunks: {:?}",
                b.to_unload
            );
        }
    }

    // ---- M09 task 1: nearest-first batch selection ----

    #[test]
    fn nearest_first_picks_closest_and_sorts_them() {
        let cam = cp(0, 0, 0);
        let set = [cp(10, 0, 0), cp(1, 0, 0), cp(5, 0, 0), cp(2, 0, 0)];
        let got = nearest_first(set.iter().copied(), cam, 3);
        assert_eq!(got, vec![cp(1, 0, 0), cp(2, 0, 0), cp(5, 0, 0)]);
    }

    #[test]
    fn nearest_first_handles_fewer_than_requested() {
        let cam = cp(0, 0, 0);
        let set = [cp(3, 0, 0), cp(1, 0, 0)];
        let got = nearest_first(set.iter().copied(), cam, 10);
        assert_eq!(got, vec![cp(1, 0, 0), cp(3, 0, 0)]);
        assert!(nearest_first(std::iter::empty(), cam, 5).is_empty());
        assert!(nearest_first(set.iter().copied(), cam, 0).is_empty());
    }

    #[test]
    fn nearest_first_uses_3d_distance_and_is_deterministic() {
        let cam = cp(0, 0, 0);
        // Equidistant on different axes: order must be stable across runs
        // (ties broken by position), so batches don't shuffle frame to frame.
        let set = [cp(0, 0, 2), cp(2, 0, 0), cp(0, 2, 0), cp(1, 1, 1)];
        let a = nearest_first(set.iter().copied(), cam, 4);
        let b = nearest_first(set.iter().rev().copied(), cam, 4);
        assert_eq!(a, b, "selection must not depend on input order");
        // (1,1,1) is d²=3, nearer than the d²=4 trio.
        assert_eq!(a[0], cp(1, 1, 1));
    }

    #[test]
    fn nearest_first_survives_huge_coordinates() {
        // Distances must not overflow at extreme world positions.
        let cam = cp(2_000_000_000, 0, -2_000_000_000);
        let set = [
            cp(-2_000_000_000, 0, 2_000_000_000),
            cp(2_000_000_001, 0, -2_000_000_000),
        ];
        let got = nearest_first(set.iter().copied(), cam, 1);
        assert_eq!(got, vec![cp(2_000_000_001, 0, -2_000_000_000)]);
    }
}
