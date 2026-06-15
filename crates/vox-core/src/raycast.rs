//! Voxel raycasting (Milestone 03 task 3).
//!
//! An Amanatides–Woo DDA traversal: march a ray through the integer voxel
//! grid one cell at a time (never skipping or double-counting a cell) and
//! stop at the first solid block within reach. Pure logic — solidity is
//! supplied as a borrowed `is_solid(WorldPos) -> bool` closure, so this is
//! decoupled from `World` and fully unit-testable without graphics or IO.
//!
//! Reference: Amanatides & Woo, "A Fast Voxel Traversal Algorithm for Ray
//! Tracing" (1987).

use crate::block::BlockId;
use crate::coords::WorldPos;

/// What a ray hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayHit {
    /// The solid voxel the ray entered.
    pub block_pos: WorldPos,
    /// The empty voxel just before it along the ray — where a placed block
    /// would go (the face that was struck, stepped back one cell). `None`
    /// only in the degenerate case of starting already inside a solid block,
    /// where there is no "previous" empty cell.
    pub place_pos: Option<WorldPos>,
    /// Outward unit face normal of the struck face, e.g. `(0, 1, 0)` for a
    /// top face. Each component is -1, 0, or +1 and exactly one is nonzero.
    /// `(0,0,0)` only in the start-inside-solid case.
    pub normal: (i64, i64, i64),
}

/// Cast a ray from `origin` along `dir` (need not be normalized) up to
/// `max_distance` world units. Returns the first solid voxel hit, or `None`
/// if none is solid within reach.
///
/// `is_solid(pos)` reports whether the voxel at integer `pos` blocks the ray.
pub fn raycast_voxels(
    origin: [f64; 3],
    dir: [f64; 3],
    max_distance: f64,
    mut is_solid: impl FnMut(WorldPos) -> bool,
) -> Option<RayHit> {
    // Normalize direction; a zero-length ray can't hit anything.
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len == 0.0 || !len.is_finite() {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];

    // Current voxel (the cell containing the origin point).
    let mut voxel = [
        origin[0].floor() as i64,
        origin[1].floor() as i64,
        origin[2].floor() as i64,
    ];

    // If we start inside a solid voxel, that's the hit (no entry face).
    let start = WorldPos::new(voxel[0], voxel[1], voxel[2]);
    if is_solid(start) {
        return Some(RayHit {
            block_pos: start,
            place_pos: None,
            normal: (0, 0, 0),
        });
    }

    // Step direction per axis (+1 / -1 / 0).
    let step = [sign(d[0]), sign(d[1]), sign(d[2])];

    // tMax: distance along the ray to the next voxel boundary on each axis.
    // tDelta: distance along the ray between successive boundaries per axis.
    let mut t_max = [0.0f64; 3];
    let mut t_delta = [0.0f64; 3];
    for a in 0..3 {
        if d[a] == 0.0 {
            // Ray is parallel to this axis: it never crosses a boundary here.
            t_max[a] = f64::INFINITY;
            t_delta[a] = f64::INFINITY;
        } else {
            let inv = 1.0 / d[a].abs();
            t_delta[a] = inv;
            // Distance to the first boundary in the step direction.
            let cell_min = voxel[a] as f64;
            let next_boundary = if step[a] > 0 {
                cell_min + 1.0
            } else {
                cell_min
            };
            t_max[a] = (next_boundary - origin[a]) / d[a];
            // When stepping negative, the above already yields a positive t
            // because both numerator and denominator are negative.
        }
    }

    // March. `prev` tracks the cell we came from (the place cell) and the
    // axis we last stepped gives the face normal.
    loop {
        // Advance along whichever axis has the nearest next boundary.
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };

        // Stop if the next boundary is beyond reach.
        if t_max[axis] > max_distance {
            return None;
        }

        let prev = voxel;
        voxel[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        let pos = WorldPos::new(voxel[0], voxel[1], voxel[2]);
        if is_solid(pos) {
            // The struck face normal points back along the axis we stepped.
            let mut normal = (0, 0, 0);
            match axis {
                0 => normal.0 = -step[0],
                1 => normal.1 = -step[1],
                _ => normal.2 = -step[2],
            }
            return Some(RayHit {
                block_pos: pos,
                place_pos: Some(WorldPos::new(prev[0], prev[1], prev[2])),
                normal,
            });
        }
    }
}

/// Convenience wrapper that resolves solidity through a block accessor and a
/// "is this block id solid?" predicate (e.g. the registry).
pub fn raycast_blocks(
    origin: [f64; 3],
    dir: [f64; 3],
    max_distance: f64,
    mut block_at: impl FnMut(WorldPos) -> BlockId,
    mut solid: impl FnMut(BlockId) -> bool,
) -> Option<RayHit> {
    raycast_voxels(origin, dir, max_distance, |p| solid(block_at(p)))
}

#[inline]
fn sign(x: f64) -> i64 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Whether the unit voxel cell at integer `cell` (spanning `[cell, cell+1]`
/// on each axis) intersects the axis-aligned box `[aabb_min, aabb_max]`.
///
/// Used to reject block placement that would overlap the player (so you
/// can't entomb the camera). Standard separating-axis AABB overlap; touching
/// faces (shared boundary, zero overlap volume) do NOT count as overlap, so
/// you can place a block flush against the player without being blocked.
pub fn cell_overlaps_aabb(cell: WorldPos, aabb_min: [f64; 3], aabb_max: [f64; 3]) -> bool {
    let cell_min = [cell.x as f64, cell.y as f64, cell.z as f64];
    let cell_max = [cell_min[0] + 1.0, cell_min[1] + 1.0, cell_min[2] + 1.0];
    for a in 0..3 {
        // Strict inequalities: flush contact is not overlap.
        if cell_max[a] <= aabb_min[a] || cell_min[a] >= aabb_max[a] {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn solids(cells: &[(i64, i64, i64)]) -> impl Fn(WorldPos) -> bool + '_ {
        let set: HashSet<(i64, i64, i64)> = cells.iter().copied().collect();
        move |p| set.contains(&(p.x, p.y, p.z))
    }

    #[test]
    fn hits_straight_ahead_on_x() {
        // Solid at x=5; ray from x=0 looking +X along the row y=z=0.
        let solid = solids(&[(5, 0, 0)]);
        let hit = raycast_voxels([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos, WorldPos::new(5, 0, 0));
        assert_eq!(hit.place_pos, Some(WorldPos::new(4, 0, 0)));
        assert_eq!(hit.normal, (-1, 0, 0)); // we approached from -X
    }

    #[test]
    fn hits_from_negative_direction() {
        let solid = solids(&[(0, 0, 0)]);
        // Start at x=5.5 looking -X.
        let hit = raycast_voxels([5.5, 0.5, 0.5], [-1.0, 0.0, 0.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos, WorldPos::new(0, 0, 0));
        assert_eq!(hit.place_pos, Some(WorldPos::new(1, 0, 0)));
        assert_eq!(hit.normal, (1, 0, 0)); // approached from +X
    }

    #[test]
    fn reports_top_face_normal() {
        // Looking down onto a floor block.
        let solid = solids(&[(0, 0, 0)]);
        let hit = raycast_voxels([0.5, 5.5, 0.5], [0.0, -1.0, 0.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos, WorldPos::new(0, 0, 0));
        assert_eq!(hit.normal, (0, 1, 0)); // struck the top face
        assert_eq!(hit.place_pos, Some(WorldPos::new(0, 1, 0)));
    }

    #[test]
    fn misses_when_nothing_in_range() {
        let solid = solids(&[(50, 0, 0)]);
        // Reach only 10 units; block is 50 away.
        assert!(raycast_voxels([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, solid).is_none());
    }

    #[test]
    fn misses_empty_world() {
        let solid = |_: WorldPos| false;
        assert!(raycast_voxels([0.0, 0.0, 0.0], [1.0, 2.0, 3.0], 1000.0, solid).is_none());
    }

    #[test]
    fn diagonal_ray_hits_correct_cell_and_face() {
        // A wall of solids at x=3 (any y,z in range). A diagonal ray should
        // strike the wall and report an X-facing normal (it crosses the x=3
        // boundary to enter).
        let solid = |p: WorldPos| p.x == 3 && (0..6).contains(&p.y) && (0..6).contains(&p.z);
        let hit = raycast_voxels([0.5, 0.5, 0.5], [1.0, 0.7, 0.3], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos.x, 3);
        assert_eq!(hit.normal, (-1, 0, 0));
        // Place cell is just outside the wall on the -X side.
        assert_eq!(hit.place_pos.unwrap().x, 2);
    }

    #[test]
    fn start_inside_solid_returns_immediately() {
        let solid = solids(&[(0, 0, 0)]);
        let hit = raycast_voxels([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos, WorldPos::new(0, 0, 0));
        assert_eq!(hit.place_pos, None);
        assert_eq!(hit.normal, (0, 0, 0));
    }

    #[test]
    fn marches_without_skipping_cells() {
        // First solid along a shallow diagonal is the nearest one; ensure we
        // don't tunnel past it. Solids at x=2 and x=4; must hit x=2 first.
        let solid = |p: WorldPos| (p.x == 2 || p.x == 4) && p.y == 0 && p.z == 0;
        let hit = raycast_voxels([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos.x, 2);
    }

    #[test]
    fn negative_coordinate_region() {
        let solid = solids(&[(-10, -10, -10)]);
        let hit = raycast_voxels([-5.5, -5.5, -5.5], [-1.0, -1.0, -1.0], 100.0, solid).unwrap();
        assert_eq!(hit.block_pos, WorldPos::new(-10, -10, -10));
        // Entered along one axis; normal points back toward +.
        let (nx, ny, nz) = hit.normal;
        assert_eq!(nx.abs() + ny.abs() + nz.abs(), 1, "exactly one face axis");
    }

    #[test]
    fn place_cell_is_adjacent_to_hit_across_normal() {
        // For any hit (not start-inside), place_pos = block_pos + normal.
        let solid = solids(&[(7, 2, 3)]);
        let hit = raycast_voxels([0.5, 2.5, 3.5], [1.0, 0.0, 0.0], 100.0, solid).unwrap();
        let b = hit.block_pos;
        let n = hit.normal;
        assert_eq!(
            hit.place_pos.unwrap(),
            WorldPos::new(b.x + n.0, b.y + n.1, b.z + n.2)
        );
    }

    #[test]
    fn overlap_cell_containing_point() {
        // A small player box around (5.5, 10.5, 5.5) overlaps cell (5,10,5).
        let min = [5.2, 10.2, 5.2];
        let max = [5.8, 11.8, 5.8];
        assert!(cell_overlaps_aabb(WorldPos::new(5, 10, 5), min, max));
        assert!(cell_overlaps_aabb(WorldPos::new(5, 11, 5), min, max)); // head cell
    }

    #[test]
    fn no_overlap_adjacent_cell() {
        // Player box well inside cell (5,10,5); neighbor cell (6,10,5) is clear.
        let min = [5.3, 10.2, 5.3];
        let max = [5.7, 11.6, 5.7];
        assert!(!cell_overlaps_aabb(WorldPos::new(6, 10, 5), min, max));
        assert!(!cell_overlaps_aabb(WorldPos::new(4, 10, 5), min, max));
        assert!(!cell_overlaps_aabb(WorldPos::new(5, 9, 5), min, max)); // below feet
    }

    #[test]
    fn flush_contact_is_not_overlap() {
        // Box exactly spanning [5,6] touches cell (6,..) at the shared face
        // but does not overlap it — placement flush against the player is OK.
        let min = [5.0, 10.0, 5.0];
        let max = [6.0, 11.0, 6.0];
        assert!(!cell_overlaps_aabb(WorldPos::new(6, 10, 5), min, max));
        assert!(cell_overlaps_aabb(WorldPos::new(5, 10, 5), min, max));
    }
}
