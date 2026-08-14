//! Single-level LOD ring selection (Milestone 08, ADR-0008).
//!
//! Pure bookkeeping, mirroring [`Streamer`](crate::streaming::Streamer): given
//! the camera's position, [`LodRing`] tracks which coarse LOD *nodes* are
//! loaded and computes which to load and unload. It owns no geometry and does
//! no generation — *how* a node is filled (generate coarse from seed, mesh,
//! upload) is the caller's concern.
//!
//! ## Nodes and the node grid
//!
//! One LOD node covers a `stride`×`stride` block of chunks horizontally
//! (`stride` chunks per side). The world tiles into a fixed grid of these; a
//! [`LodNodePos`] is a node's `(x, z)` cell in that grid. The ring is
//! horizontal only — terrain is a heightfield, so distant terrain is a ring of
//! node columns around the camera, each placed at the surface's Y band by the
//! caller.
//!
//! ## The disjoint boundary (the M08 "named trap")
//!
//! Full-res chunks and LOD nodes must never draw the same terrain (criterion 4:
//! no double terrain, no z-fighting). This module enforces that at *node
//! granularity*: the camera's node and the block of nodes within
//! `full_radius_nodes` of it are **owned by full-res** and are never LOD; LOD
//! owns the ring from `full_radius_nodes + 1` out to `lod_radius_nodes`. For
//! this to be gapless, the caller must drive the full-res streamer to fill that
//! inner node block (set its chunk radius to cover `full_radius_nodes` whole
//! nodes). The partition is exact — no overlap, no gap — because both sides
//! snap to the same node grid.
//!
//! ## Hysteresis
//!
//! Outer edge: nodes load at `lod_radius_nodes`, unload only beyond
//! `unload_radius_nodes` (> load), so a camera on a node boundary doesn't
//! thrash distant nodes. The inner (full-res) edge is crisp: full-res picks up
//! a node the same frame LOD drops it, so it is always covered by exactly one.

use std::collections::HashSet;

use crate::coords::{CHUNK_SIZE, ChunkPos};

/// A node's cell in the horizontal LOD node grid. One node spans `stride`
/// chunks per side; the grid is fixed to the world (not camera-relative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LodNodePos {
    pub x: i64,
    pub z: i64,
}

impl LodNodePos {
    #[inline]
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    /// The node grid cell containing a chunk (floor division; correct for
    /// negatives).
    #[inline]
    pub fn containing(chunk: ChunkPos, stride: i64) -> Self {
        Self {
            x: chunk.x.div_euclid(stride),
            z: chunk.z.div_euclid(stride),
        }
    }

    /// Minimum chunk coordinate (x) this node covers.
    #[inline]
    pub fn min_chunk_x(self, stride: i64) -> i64 {
        self.x * stride
    }

    /// Minimum chunk coordinate (z) this node covers.
    #[inline]
    pub fn min_chunk_z(self, stride: i64) -> i64 {
        self.z * stride
    }

    /// World-space minimum block corner (x) of this node's region.
    #[inline]
    pub fn origin_block_x(self, stride: i64) -> i64 {
        self.min_chunk_x(stride) * CHUNK_SIZE as i64
    }

    /// World-space minimum block corner (z) of this node's region.
    #[inline]
    pub fn origin_block_z(self, stride: i64) -> i64 {
        self.min_chunk_z(stride) * CHUNK_SIZE as i64
    }
}

/// What the [`LodRing`] wants the caller to do this update.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LodUpdate {
    /// Nodes newly in the ring: generate coarse, mesh, upload, then confirm
    /// via [`LodRing::mark_loaded`].
    pub to_load: Vec<LodNodePos>,
    /// Nodes now out of the ring (too far, or taken over by full-res): drop and
    /// confirm via [`LodRing::mark_unloaded`].
    pub to_unload: Vec<LodNodePos>,
}

impl LodUpdate {
    pub fn is_empty(&self) -> bool {
        self.to_load.is_empty() && self.to_unload.is_empty()
    }
}

/// Chebyshev (chessboard) distance between node cells — the natural metric for
/// a square ring.
#[inline]
fn cheby(a: LodNodePos, b: LodNodePos) -> i64 {
    (a.x - b.x).abs().max((a.z - b.z).abs())
}

/// Tracks the loaded LOD-node set and computes ring deltas.
pub struct LodRing {
    loaded: HashSet<LodNodePos>,
    /// Nodes within this Chebyshev radius of the camera node are owned by
    /// full-res and are never LOD.
    full_radius_nodes: i64,
    /// LOD nodes load out to this radius (inclusive), beyond full_radius.
    load_radius_nodes: i64,
    /// LOD nodes unload only beyond this radius (> load; outer hysteresis).
    unload_radius_nodes: i64,
    /// Node stride in chunks (a node = stride chunks per side).
    stride: i64,
    last_center: Option<LodNodePos>,
}

impl LodRing {
    /// Panics if the radii are not strictly increasing
    /// (`full < load < unload`) or `stride < 1` — the bands must each be at
    /// least one node wide.
    pub fn new(
        stride: i64,
        full_radius_nodes: i64,
        load_radius_nodes: i64,
        unload_radius_nodes: i64,
    ) -> Self {
        assert!(stride >= 1, "LOD stride must be >= 1");
        assert!(
            full_radius_nodes >= 0
                && load_radius_nodes > full_radius_nodes
                && unload_radius_nodes > load_radius_nodes,
            "require full < load < unload (each band >= 1 node wide)"
        );
        Self {
            loaded: HashSet::new(),
            full_radius_nodes,
            load_radius_nodes,
            unload_radius_nodes,
            stride,
            last_center: None,
        }
    }

    pub fn stride(&self) -> i64 {
        self.stride
    }

    pub fn loaded(&self) -> &HashSet<LodNodePos> {
        &self.loaded
    }

    /// True iff node `n` should be a LOD node for camera node `cam` at the
    /// given outer radius: in the ring `full_radius < cheby <= radius`.
    fn in_ring(&self, n: LodNodePos, cam: LodNodePos, radius: i64) -> bool {
        let d = cheby(n, cam);
        d > self.full_radius_nodes && d <= radius
    }

    /// Compute the load/unload delta for the camera's chunk position. A no-op
    /// (empty) when the camera hasn't changed node cells.
    pub fn update(&mut self, camera_chunk: ChunkPos) -> LodUpdate {
        let cam = LodNodePos::containing(camera_chunk, self.stride);
        if self.last_center == Some(cam) {
            return LodUpdate::default();
        }
        self.last_center = Some(cam);

        // Load: every node in the ring (out to load radius) not already loaded.
        let mut to_load = Vec::new();
        for dz in -self.load_radius_nodes..=self.load_radius_nodes {
            for dx in -self.load_radius_nodes..=self.load_radius_nodes {
                let n = LodNodePos::new(cam.x + dx, cam.z + dz);
                if self.in_ring(n, cam, self.load_radius_nodes) && !self.loaded.contains(&n) {
                    to_load.push(n);
                }
            }
        }

        // Unload: any loaded node no longer in the ring out to the UNLOAD
        // radius — i.e. now inside the full-res block, or beyond unload range.
        let to_unload: Vec<LodNodePos> = self
            .loaded
            .iter()
            .copied()
            .filter(|&n| !self.in_ring(n, cam, self.unload_radius_nodes))
            .collect();

        LodUpdate { to_load, to_unload }
    }

    /// Confirm a node was loaded.
    pub fn mark_loaded(&mut self, n: LodNodePos) {
        self.loaded.insert(n);
    }

    /// Confirm a node was unloaded.
    pub fn mark_unloaded(&mut self, n: LodNodePos) {
        self.loaded.remove(&n);
    }

    /// Forget all loaded nodes (e.g. when LOD is toggled off). The next
    /// `update` will re-request the full ring.
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.last_center = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: i64 = 8;

    fn ring(full: i64, load: i64, unload: i64) -> LodRing {
        LodRing::new(STRIDE, full, load, unload)
    }

    fn cp(cx: i64, cz: i64) -> ChunkPos {
        ChunkPos::new(cx, 0, cz)
    }

    /// A chunk maps to the right node cell, including negatives.
    #[test]
    fn node_containing_chunk() {
        assert_eq!(LodNodePos::containing(cp(0, 0), 8), LodNodePos::new(0, 0));
        assert_eq!(LodNodePos::containing(cp(7, 7), 8), LodNodePos::new(0, 0));
        assert_eq!(LodNodePos::containing(cp(8, 0), 8), LodNodePos::new(1, 0));
        assert_eq!(
            LodNodePos::containing(cp(-1, -1), 8),
            LodNodePos::new(-1, -1)
        );
        assert_eq!(LodNodePos::containing(cp(-8, 0), 8), LodNodePos::new(-1, 0));
    }

    /// The loaded ring excludes the inner full-res block and reaches the load
    /// radius — a square annulus.
    #[test]
    fn ring_is_an_annulus_excluding_full_res() {
        let mut r = ring(1, 3, 4);
        let up = r.update(cp(0, 0));
        for &n in &up.to_load {
            let d = cheby(n, LodNodePos::new(0, 0));
            assert!(d > 1 && d <= 3, "node {n:?} at cheby {d} outside annulus");
        }
        // Count: (2*3+1)^2 - (2*1+1)^2 = 49 - 9 = 40 nodes.
        assert_eq!(up.to_load.len(), 40);
        // No LOD node ever coincides with the full-res block.
        assert!(
            !up.to_load
                .iter()
                .any(|&n| cheby(n, LodNodePos::new(0, 0)) <= 1)
        );
    }

    /// Re-updating without changing node is a no-op (no thrash while still).
    #[test]
    fn stationary_update_is_noop() {
        let mut r = ring(1, 3, 4);
        let first = r.update(cp(0, 0));
        for n in &first.to_load {
            r.mark_loaded(*n);
        }
        // Same node (moved within it, not across): nothing to do.
        assert!(r.update(cp(1, 1)).is_empty());
    }

    /// Moving one node loads only the new leading strip and unloads only the
    /// trailing one — not the whole set.
    #[test]
    fn moving_one_node_streams_only_the_delta() {
        let mut r = ring(1, 3, 4);
        let up = r.update(cp(0, 0));
        for n in &up.to_load {
            r.mark_loaded(*n);
        }
        // Move one node in +x (chunk 8 → node 1).
        let up = r.update(cp(8, 0));
        // Delta is bounded — a strip, far smaller than the 40-node ring.
        assert!(!up.to_load.is_empty());
        assert!(
            up.to_load.len() <= 16,
            "delta too big: {}",
            up.to_load.len()
        );
        // Nothing loaded twice.
        for n in &up.to_load {
            assert!(!r.loaded().contains(n));
        }
    }

    /// Outer hysteresis: a node in the load..unload gap is not unloaded when the
    /// camera drifts, so it doesn't flip-flop.
    #[test]
    fn outer_hysteresis_prevents_thrash() {
        let mut r = ring(1, 3, 5);
        for n in r.update(cp(0, 0)).to_load {
            r.mark_loaded(n);
        }
        // Move so the far edge nodes fall into the gap (cheby 4, between load 3
        // and unload 5): nodes in the keep zone (1 < cheby <= 5) must NOT be
        // unloaded. (Unloads are legitimate only inside full-res or beyond
        // unload range.)
        let up = r.update(cp(8, 0)); // camera node (1,0)
        for n in &up.to_unload {
            let d = cheby(*n, LodNodePos::new(1, 0));
            assert!(
                d <= 1 || d > 5,
                "node at cheby {d} is in the keep zone but was unloaded (thrash)"
            );
        }
    }

    /// Inner boundary: a node the camera approaches gets handed to full-res
    /// (unloaded from LOD) once it enters the full-res block.
    #[test]
    fn node_handed_to_full_res_on_approach() {
        let mut r = ring(1, 3, 4);
        for n in r.update(cp(0, 0)).to_load {
            r.mark_loaded(n);
        }
        // Node (2,0) starts as LOD (cheby 2 from camera node 0). Move camera to
        // node (1,0): now (2,0) is cheby 1 → inside full-res → must unload.
        assert!(r.loaded().contains(&LodNodePos::new(2, 0)));
        let up = r.update(cp(8, 0));
        assert!(
            up.to_unload.contains(&LodNodePos::new(2, 0)),
            "node entering full-res block should unload from LOD"
        );
    }

    #[test]
    fn origin_blocks_are_node_aligned() {
        let n = LodNodePos::new(-1, 2);
        assert_eq!(n.origin_block_x(8), -(8 * CHUNK_SIZE as i64));
        assert_eq!(n.origin_block_z(8), 2 * 8 * CHUNK_SIZE as i64);
    }

    #[test]
    #[should_panic]
    fn rejects_non_increasing_radii() {
        let _ = LodRing::new(8, 3, 3, 5); // load == full
    }
}
