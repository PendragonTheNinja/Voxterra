//! Block-light propagation (Milestone 04 task 2, ADR-0004).
//!
//! A BFS flood-fill: light-emitting voxels seed their emission level, and
//! light spreads to adjacent non-solid voxels at one level less, stopping at
//! 0 and at solid blocks. Overlapping sources take the brightest value, not
//! the sum.
//!
//! The core ([`propagate_block_light`]) is written against an abstract
//! [`LightVolume`] — a rectangular region exposing per-cell solidity and
//! emission plus get/set light — so it is independent of `Chunk`, the
//! registry, and the renderer, and is exhaustively unit-testable on tiny
//! hand-built grids. A chunk adapter (task 4) implements `LightVolume` over a
//! chunk-plus-neighbor-borders view.
//!
//! ## Cross-chunk spread
//!
//! Like meshing, lighting is computed per chunk with neighbors as read-only
//! boundary context: a chunk relights using its neighbors' border light as
//! fixed inputs (light flows *in* from bright neighbors). When a chunk's
//! border light changes, the dirty-set marks the neighbor for relight, which
//! then sees the updated border. This avoids holding several chunks mutably
//! at once while still converging to globally-correct light.

/// Maximum light level (4-bit, ADR-0004).
pub const MAX_LIGHT: u8 = 15;

/// A rectangular volume the propagator can read solidity/emission from and
/// read/write light into. Coordinates are volume-local `(x, y, z)`, each in
/// `0..dim(axis)`. Implementors decide how out-of-this-chunk borders are
/// represented (e.g. an extra ring of cells carrying neighbor light).
pub trait LightVolume {
    /// Dimensions `[x, y, z]` of the volume in cells.
    fn dims(&self) -> [usize; 3];
    /// Whether the cell blocks light (light does not enter solid cells).
    fn is_solid(&self, x: usize, y: usize, z: usize) -> bool;
    /// Light this cell emits on its own (0 for non-emitters).
    fn emission(&self, x: usize, y: usize, z: usize) -> u8;
    /// Current block-light at the cell.
    fn get_light(&self, x: usize, y: usize, z: usize) -> u8;
    /// Set block-light at the cell.
    fn set_light(&mut self, x: usize, y: usize, z: usize, level: u8);
}

/// Flood-fill block light through `vol` from scratch: clears non-emitter
/// light to 0, seeds emitters, and BFS-propagates. Cells that are solid hold
/// their emission (so a glowing solid block still reads as lit) but do not
/// propagate beyond their own cell unless they emit.
///
/// Returns the number of cells that ended up lit (>0), useful for telemetry
/// and for deciding whether a chunk needs light storage at all.
pub fn propagate_block_light(vol: &mut impl LightVolume) -> usize {
    let [dx, dy, dz] = vol.dims();

    // Seed: each cell starts at its own emission. (Clearing first guarantees
    // a from-scratch recompute, matching recompute-on-load semantics.)
    let mut queue: Vec<(usize, usize, usize, u8)> = Vec::new();
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let e = vol.emission(x, y, z);
                vol.set_light(x, y, z, e);
                if e > 1 {
                    queue.push((x, y, z, e));
                }
            }
        }
    }

    // BFS. Each dequeued cell pushes light-1 into non-solid neighbors that
    // are currently dimmer, raising them and re-queueing.
    let mut head = 0;
    while head < queue.len() {
        let (x, y, z, level) = queue[head];
        head += 1;

        // A cell may have been raised brighter after being queued; use the
        // current value so we propagate the strongest.
        let level = vol.get_light(x, y, z).max(level);
        if level <= 1 {
            continue;
        }
        let spread = level - 1;

        for (nx, ny, nz) in neighbors(x, y, z, dx, dy, dz) {
            if vol.is_solid(nx, ny, nz) {
                continue;
            }
            if vol.get_light(nx, ny, nz) < spread {
                vol.set_light(nx, ny, nz, spread);
                if spread > 1 {
                    queue.push((nx, ny, nz, spread));
                }
            }
        }
    }

    // Count lit cells.
    let mut lit = 0;
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                if vol.get_light(x, y, z) > 0 {
                    lit += 1;
                }
            }
        }
    }
    lit
}

/// The in-bounds 6-neighbors of a cell.
fn neighbors(
    x: usize,
    y: usize,
    z: usize,
    dx: usize,
    dy: usize,
    dz: usize,
) -> impl Iterator<Item = (usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::with_capacity(6);
    if x + 1 < dx {
        out.push((x + 1, y, z));
    }
    if x > 0 {
        out.push((x - 1, y, z));
    }
    if y + 1 < dy {
        out.push((x, y + 1, z));
    }
    if y > 0 {
        out.push((x, y - 1, z));
    }
    if z + 1 < dz {
        out.push((x, y, z + 1));
    }
    if z > 0 {
        out.push((x, y, z - 1));
    }
    out.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple dense test volume: per-cell solid flag + emission + light.
    struct Grid {
        dims: [usize; 3],
        solid: Vec<bool>,
        emission: Vec<u8>,
        light: Vec<u8>,
    }
    impl Grid {
        fn new(dx: usize, dy: usize, dz: usize) -> Self {
            let n = dx * dy * dz;
            Self {
                dims: [dx, dy, dz],
                solid: vec![false; n],
                emission: vec![0; n],
                light: vec![0; n],
            }
        }
        fn idx(&self, x: usize, y: usize, z: usize) -> usize {
            x + y * self.dims[0] + z * self.dims[0] * self.dims[1]
        }
        fn set_solid(&mut self, x: usize, y: usize, z: usize) {
            let i = self.idx(x, y, z);
            self.solid[i] = true;
        }
        fn set_emission(&mut self, x: usize, y: usize, z: usize, e: u8) {
            let i = self.idx(x, y, z);
            self.emission[i] = e;
        }
    }
    impl LightVolume for Grid {
        fn dims(&self) -> [usize; 3] {
            self.dims
        }
        fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
            self.solid[self.idx(x, y, z)]
        }
        fn emission(&self, x: usize, y: usize, z: usize) -> u8 {
            self.emission[self.idx(x, y, z)]
        }
        fn get_light(&self, x: usize, y: usize, z: usize) -> u8 {
            self.light[self.idx(x, y, z)]
        }
        fn set_light(&mut self, x: usize, y: usize, z: usize, level: u8) {
            let i = self.idx(x, y, z);
            self.light[i] = level;
        }
    }

    #[test]
    fn single_source_radial_falloff() {
        // Source at center of an open 11³ grid emits 15; light should fall
        // off by Manhattan distance.
        let mut g = Grid::new(11, 11, 11);
        g.set_emission(5, 5, 5, 15);
        propagate_block_light(&mut g);

        assert_eq!(g.get_light(5, 5, 5), 15);
        assert_eq!(g.get_light(6, 5, 5), 14); // 1 step
        assert_eq!(g.get_light(5, 5, 7), 13); // 2 steps
        // A cell 5 steps away in a straight line.
        assert_eq!(g.get_light(10, 5, 5), 10);
        // Manhattan distance 15 would be level 0; distance 10 → 5.
        assert_eq!(g.get_light(10, 10, 5), 15 - 10);
    }

    #[test]
    fn corridor_linear_falloff() {
        // A 1-wide corridor: walls solid, a 1×1×N tube of air. Light falls
        // off linearly along the only open path.
        let mut g = Grid::new(20, 3, 3);
        // Make everything solid, then carve the row y=1,z=1.
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..20 {
                    if !(y == 1 && z == 1) {
                        g.set_solid(x, y, z);
                    }
                }
            }
        }
        g.set_emission(0, 1, 1, 15);
        propagate_block_light(&mut g);

        for x in 0..16 {
            assert_eq!(g.get_light(x, 1, 1), (15 - x as u8), "at x={x}");
        }
        assert_eq!(g.get_light(15, 1, 1), 0);
    }

    #[test]
    fn light_stops_at_solid_wall() {
        // Source on one side of a solid wall; the far side stays dark.
        let mut g = Grid::new(5, 3, 3);
        // Wall at x=2 spanning the whole y,z cross-section.
        for z in 0..3 {
            for y in 0..3 {
                g.set_solid(2, y, z);
            }
        }
        g.set_emission(0, 1, 1, 15);
        propagate_block_light(&mut g);

        assert!(g.get_light(1, 1, 1) > 0, "near side lit");
        assert_eq!(g.get_light(2, 1, 1), 0, "wall itself unlit (no emission)");
        assert_eq!(g.get_light(3, 1, 1), 0, "far side dark");
        assert_eq!(g.get_light(4, 1, 1), 0);
    }

    #[test]
    fn overlapping_sources_take_max_not_sum() {
        // Two emitters; a cell between them takes the brighter contribution.
        let mut g = Grid::new(11, 3, 3);
        g.set_emission(0, 1, 1, 15);
        g.set_emission(10, 1, 1, 15);
        propagate_block_light(&mut g);

        // Midpoint x=5 is 5 from each: each contributes 10, max = 10 (not 20).
        assert_eq!(g.get_light(5, 1, 1), 10);
        assert!(g.get_light(5, 1, 1) <= MAX_LIGHT);
    }

    #[test]
    fn emitter_can_be_solid() {
        // A glowing solid block (like a lamp): it holds its emission and
        // lights its non-solid neighbors.
        let mut g = Grid::new(5, 3, 3);
        g.set_solid(2, 1, 1);
        g.set_emission(2, 1, 1, 14);
        propagate_block_light(&mut g);

        assert_eq!(g.get_light(2, 1, 1), 14, "solid emitter holds emission");
        assert_eq!(g.get_light(1, 1, 1), 13, "neighbor lit one less");
        assert_eq!(g.get_light(3, 1, 1), 13);
    }

    #[test]
    fn empty_volume_stays_dark() {
        let mut g = Grid::new(8, 8, 8);
        let lit = propagate_block_light(&mut g);
        assert_eq!(lit, 0);
        assert_eq!(g.get_light(4, 4, 4), 0);
    }

    #[test]
    fn recompute_clears_stale_light() {
        // Pre-seed bogus light, then propagate with no emitters: must clear.
        let mut g = Grid::new(5, 5, 5);
        g.set_light(2, 2, 2, 9);
        let lit = propagate_block_light(&mut g);
        assert_eq!(lit, 0, "stale light must be cleared on recompute");
        assert_eq!(g.get_light(2, 2, 2), 0);
    }

    #[test]
    fn level_one_does_not_propagate() {
        // A source emitting 1 lights only itself (1-1 = 0 to neighbors).
        let mut g = Grid::new(5, 5, 5);
        g.set_emission(2, 2, 2, 1);
        propagate_block_light(&mut g);
        assert_eq!(g.get_light(2, 2, 2), 1);
        assert_eq!(g.get_light(3, 2, 2), 0);
    }

    #[test]
    fn light_goes_around_obstacles() {
        // A partial wall with a gap: light should bend around through the gap.
        let mut g = Grid::new(7, 7, 3);
        // Wall at x=3 for all y except a gap at y=0.
        for z in 0..3 {
            for y in 1..7 {
                g.set_solid(3, y, z);
            }
        }
        g.set_emission(0, 3, 1, 15);
        propagate_block_light(&mut g);

        // Directly behind the wall but not at the gap row: only reachable by
        // routing through the gap, so dimmer than the straight-line distance.
        let behind = g.get_light(4, 3, 1);
        assert!(behind > 0, "light should reach behind via the gap");
        // The blocked cell on the wall is dark.
        assert_eq!(g.get_light(3, 3, 1), 0);
    }
}
