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
//! - `load_radius`: chunks within this of the camera should be loaded.
//! - `unload_radius` (> load_radius): chunks beyond this should be unloaded.
//!
//! Chunks in the gap between the two radii are left in whatever state they
//! are already in. Without this hysteresis band, a camera sitting on a
//! chunk boundary would load and unload the same chunk on alternating
//! frames (thrashing). The gap must be at least one chunk wide.

use std::collections::HashSet;

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
    /// Camera chunk used for the last `update`; `update` is a no-op (returns
    /// empty) when the camera hasn't changed chunks and nothing else has.
    last_center: Option<ChunkPos>,
}

impl Streamer {
    /// Create a streamer. Panics if `unload_radius <= load_radius` (the
    /// hysteresis band must be at least one chunk wide).
    pub fn new(load_radius: i64, unload_radius: i64) -> Self {
        assert!(
            load_radius >= 1 && unload_radius > load_radius,
            "need 1 <= load_radius < unload_radius (got {load_radius}, {unload_radius})"
        );
        Self {
            loaded: HashSet::new(),
            load_radius,
            unload_radius,
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
            .filter(|&p| dist_sq(p, center) > unload_sq)
            .collect();
        to_unload.sort_by_key(|&p| (p.x, p.y, p.z)); // deterministic order

        // Load: in-range chunks not already loaded. Iterate the cube that
        // bounds the load sphere, keep those inside the sphere.
        let r = self.load_radius;
        let mut to_load: Vec<ChunkPos> = Vec::new();
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let p = ChunkPos::new(center.x + dx, center.y + dy, center.z + dz);
                    if dist_sq(p, center) <= load_sq && !self.loaded.contains(&p) {
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

    #[test]
    fn initial_update_loads_sphere_around_origin() {
        let mut s = Streamer::new(2, 4);
        let update = s.update(cp(0, 0, 0));
        assert!(update.to_unload.is_empty());

        // Every returned chunk is within the load radius, none beyond it.
        let r2 = 2 * 2;
        for &p in &update.to_load {
            assert!(dist_sq(p, cp(0, 0, 0)) <= r2);
        }
        // The center and the six face neighbors are definitely in range.
        for c in [
            cp(0, 0, 0),
            cp(1, 0, 0),
            cp(-1, 0, 0),
            cp(0, 1, 0),
            cp(0, -1, 0),
            cp(0, 0, 1),
            cp(0, 0, -1),
            cp(2, 0, 0),
        ] {
            assert!(update.to_load.contains(&c), "missing {c:?}");
        }
        // A corner of the bounding cube (dist² = 12 > 4) must be excluded.
        assert!(!update.to_load.contains(&cp(2, 2, 2)));
    }

    #[test]
    fn to_load_is_nearest_first() {
        let mut s = Streamer::new(3, 5);
        let update = s.update(cp(10, 10, 10));
        let mut prev = -1;
        for &p in &update.to_load {
            let d = dist_sq(p, cp(10, 10, 10));
            assert!(d >= prev, "to_load not sorted nearest-first");
            prev = d;
        }
        // First entry is the camera's own chunk (distance 0).
        assert_eq!(update.to_load.first(), Some(&cp(10, 10, 10)));
    }

    #[test]
    fn apply_then_no_reload() {
        let mut s = Streamer::new(2, 4);
        let first = s.update(cp(0, 0, 0));
        s.apply(&first);
        let loaded_after_first = s.loaded_count();
        assert!(loaded_after_first > 0);

        // Same center again: nothing new to load, nothing to unload.
        let second = s.update(cp(0, 0, 0));
        assert!(second.to_load.is_empty(), "reloaded already-loaded chunks");
        assert!(second.to_unload.is_empty());
        assert_eq!(s.loaded_count(), loaded_after_first);
    }

    #[test]
    fn moving_loads_leading_unloads_trailing() {
        let mut s = Streamer::new(2, 4);
        let u = s.update(cp(0, 0, 0));
        s.apply(&u);
        let start = s.loaded_count();

        // Move far along +X so the old sphere is entirely beyond unload.
        let update = s.update(cp(20, 0, 0));
        assert!(!update.to_load.is_empty(), "should load new region");
        assert!(!update.to_unload.is_empty(), "should unload old region");
        // Everything unloaded is from the old neighborhood (near origin),
        // everything loaded is near the new center.
        for &p in &update.to_unload {
            assert!(dist_sq(p, cp(20, 0, 0)) > 4 * 4);
        }
        for &p in &update.to_load {
            assert!(dist_sq(p, cp(20, 0, 0)) <= 2 * 2);
        }
        s.apply(&update);
        // Loaded set size returns to the steady-state sphere size.
        assert_eq!(s.loaded_count(), start);
    }

    /// The hysteresis guarantee: a chunk in the gap band (load < d <= unload)
    /// is neither loaded nor unloaded — so nudging the camera back and forth
    /// across a boundary does not thrash it.
    #[test]
    fn hysteresis_prevents_thrash() {
        let mut s = Streamer::new(3, 5);
        let u = s.update(cp(0, 0, 0));
        s.apply(&u);

        // A chunk at distance 4 from origin: outside load (3) but inside
        // unload (5). Load it by approaching, then step back.
        let edge = cp(4, 0, 0);
        // Approach so `edge` enters load range, then apply.
        let u = s.update(cp(2, 0, 0));
        s.apply(&u);
        assert!(s.is_loaded(edge), "edge chunk should have loaded when near");

        // Step back to origin: edge is now at distance 4 — in the gap band.
        let update = s.update(cp(0, 0, 0));
        assert!(
            !update.to_unload.contains(&edge),
            "edge chunk in hysteresis band must NOT be unloaded"
        );
        assert!(s.is_loaded(edge), "edge chunk should remain loaded");

        // Only when we retreat far enough that edge exceeds unload radius
        // does it actually unload.
        let update = s.update(cp(-2, 0, 0)); // edge now at distance 6 > 5
        assert!(update.to_unload.contains(&edge));
    }

    #[test]
    fn loaded_count_bounded_regardless_of_travel() {
        let mut s = Streamer::new(3, 5);
        let mut max_loaded = 0;
        // Walk a long way; loaded set must stay bounded (no leak).
        for step in 0..200 {
            let c = cp(step, 0, 0);
            let u = s.update(c);
            s.apply(&u);
            max_loaded = max_loaded.max(s.loaded_count());
        }
        // Steady-state sphere of radius 3 is ~123 chunks; assert it never
        // balloons (a leak would grow without bound as we travel).
        assert!(max_loaded < 200, "loaded set grew unbounded: {max_loaded}");
        assert_eq!(s.loaded_count(), max_loaded, "should be at steady state");
    }

    #[test]
    fn works_in_deep_negative_coordinates() {
        let mut s = Streamer::new(2, 4);
        let center = cp(-1_000_000, -500, 1_000_000);
        let update = s.update(center);
        s.apply(&update);
        assert!(s.is_loaded(center));
        // Neighbor across a sign boundary loads correctly.
        assert!(s.is_loaded(cp(-1_000_000 + 1, -500, 1_000_000)));
    }
}
