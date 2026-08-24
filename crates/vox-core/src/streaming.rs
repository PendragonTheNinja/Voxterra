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
//! Streaming is CYLINDRICAL: `load_radius` is a horizontal radius, and a
//! vertical band of chunk-Y is always kept loaded within it. The world is a
//! heightfield, so what matters is how far away a column is, not how far above
//! it the camera is — with a sphere, flying a few hundred blocks up drops the
//! ground out of range and the terrain under you unloads.
//!
//! - `load_radius`: chunks within this HORIZONTAL distance should be loaded.
//! - `unload_radius` (> load_radius): chunks beyond this should be unloaded.
//!
//! Chunks in the gap between the two radii are left in whatever state they
//! are already in. Without this hysteresis band, a camera sitting on a
//! chunk boundary would load and unload the same chunk on alternating
//! frames (thrashing). The gap must be at least one chunk wide.

use std::collections::{BinaryHeap, HashSet};

use crate::coords::ChunkPos;

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
    /// Inclusive chunk-Y band always kept loaded within the horizontal radius.
    /// Streaming is cylindrical; see `horiz_dist_sq`.
    y_band: (i64, i64),
    /// Camera chunk used for the last `update`; `update` is a no-op (returns
    /// empty) when the camera hasn't changed chunks and nothing else has.
    last_center: Option<ChunkPos>,
}

impl Streamer {
    /// Create a streamer. Panics if `unload_radius <= load_radius` (the
    /// hysteresis band must be at least one chunk wide).
    /// Cylindrical streamer with an explicit vertical band (chunk Y, inclusive).
    pub fn with_y_band(load_radius: i64, unload_radius: i64, y_band: (i64, i64)) -> Self {
        let mut s = Self::new(load_radius, unload_radius);
        assert!(y_band.0 <= y_band.1, "y_band must be non-empty");
        s.y_band = y_band;
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
            // Default band spans the load radius vertically, matching the old
            // spherical behaviour closely enough for callers that don't care.
            y_band: (-load_radius, load_radius),
            last_center: None,
        }
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
    pub fn update(&mut self, center: ChunkPos) -> StreamUpdate {
        self.last_center = Some(center);

        let load_sq = self.load_radius * self.load_radius;
        let unload_sq = self.unload_radius * self.unload_radius;

        // Unload: loaded chunks beyond the unload radius.
        let mut to_unload: Vec<ChunkPos> = self
            .loaded
            .iter()
            .copied()
            .filter(|&p| {
                horiz_dist_sq(p, center) > unload_sq || p.y < self.y_band.0 || p.y > self.y_band.1
            })
            .collect();
        to_unload.sort_by_key(|&p| (p.x, p.y, p.z)); // deterministic order

        // Load: in-range chunks not already loaded. Iterate the horizontal
        // disc around the camera, crossed with the world's vertical band —
        // NOT a Y range relative to the camera, or climbing above the band
        // would load nothing and the terrain under you would disappear.
        let r = self.load_radius;
        let (y_lo, y_hi) = self.y_band;
        let mut to_load: Vec<ChunkPos> = Vec::new();
        for ny in y_lo..=y_hi {
            for dz in -r..=r {
                for dx in -r..=r {
                    let p = ChunkPos::new(center.x + dx, ny, center.z + dz);
                    if horiz_dist_sq(p, center) <= load_sq && !self.loaded.contains(&p) {
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

    #[test]
    #[should_panic]
    fn rejects_bad_radii() {
        Streamer::new(4, 4); // unload must exceed load
    }

    /// The load set is a CYLINDER: a horizontal disc crossed with the world's
    /// vertical band.
    #[test]
    fn initial_update_loads_cylinder_around_origin() {
        let mut s = Streamer::with_y_band(3, 5, (-2, 2));
        let update = s.update(cp(0, 0, 0));
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
        let mut s = Streamer::with_y_band(3, 5, (-2, 2));
        let first = s.update(cp(0, 0, 0));
        s.apply(&first);
        let ground = cp(0, 0, 0);
        assert!(s.is_loaded(ground));
        // Climb far above the band.
        let update = s.update(cp(0, 40, 0));
        assert!(
            !update.to_unload.contains(&ground),
            "ground unloaded when the camera climbed"
        );
        assert!(s.is_loaded(ground));
    }

    #[test]
    fn moving_loads_leading_unloads_trailing() {
        let mut s = Streamer::with_y_band(2, 4, (-1, 1));
        let first = s.update(cp(0, 0, 0));
        s.apply(&first);
        let before = s.loaded_count();
        let update = s.update(cp(6, 0, 0));
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
        let mut s = Streamer::with_y_band(2, 4, (-1, 1));
        for step in 0..40 {
            let u = s.update(cp(step * 3, 0, step));
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
        let mut s = Streamer::with_y_band(3, 5, (-1, 1));
        let update = s.update(cp(10, 0, 10));
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
        let mut s = Streamer::with_y_band(2, 4, (-500, -498));
        let center = cp(-1_000_000, -500, 1_000_000);
        let update = s.update(center);
        s.apply(&update);
        assert!(s.is_loaded(center));
        assert!(s.is_loaded(cp(-1_000_000 + 1, -500, 1_000_000)));
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
