//! The world's fixed shape: how far it extends, what a Z coordinate *means*,
//! and where a player starts (M10 task 1).
//!
//! Distinct from [`crate::world::World`], which is the runtime store of loaded
//! chunks. Nothing here holds state or allocates — it is the definition of the
//! world, identical in every session, queried by the chunk generator and the
//! LOD sampler alike.
//!
//! ## The world is finite
//!
//! Infinite generation is out as of M10. The world is a bounded box, square in
//! plan, large enough that no player reaches an edge in normal play. Finiteness
//! is what makes a latitude axis possible at all: "north" cannot mean anything
//! without a pole to be north of.
//!
//! ## Why square, and why this size
//!
//! Z *is* the latitude axis, so its extent sets the climate scale — the two are
//! not independent knobs. At [`WORLD_HALF_EXTENT_BLOCKS`] the pole-to-equator
//! distance is 100 000 blocks against Earth's 10 000 km, compressing climate
//! bands 100×: a temperate belt lands around 15 000 blocks wide. Making X match
//! costs nothing (bounds are constants, chunks generate on demand) and avoids a
//! world where travelling one direction is meaningfully longer than the other.
//!
//! ## One block is one metre, in every direction
//!
//! Vertical scale is NOT compressed. Peaks and depths are quoted in real units
//! and mean it — [`WORLD_Y_MAX_BLOCKS`] clears Everest, [`WORLD_Y_MIN_BLOCKS`]
//! clears Challenger Deep. Horizontal scale is compressed ~100×, so gradients
//! here are steeper than Earth's; the elevation field's earned-by-extent rule
//! (M10 task 4) is what keeps that from producing unclimbable walls.

use crate::coords::{CHUNK_SIZE, ChunkPos};

/// Half the world's horizontal extent, in blocks, on both X and Z.
///
/// The world spans `[-HALF, +HALF)` on each axis — half-open, so the total
/// width is exactly `2 × HALF` and the boundary falls on a chunk edge
/// (`100 000 / 32 = 3125` exactly).
pub const WORLD_HALF_EXTENT_BLOCKS: i64 = 100_000;

/// Lowest block coordinate in the world (exclusive floor is one below).
///
/// Clears Challenger Deep (−10 935 m) with room for seabed beneath it. A
/// multiple of [`CHUNK_SIZE`], so the floor is a chunk edge.
pub const WORLD_Y_MIN_BLOCKS: i64 = -11_264;

/// One past the highest block coordinate in the world.
///
/// Clears Everest (8 848 m). A multiple of [`CHUNK_SIZE`].
pub const WORLD_Y_MAX_BLOCKS: i64 = 9_216;

/// Y at which oceans and seas fill. The origin of the vertical scale: land
/// elevations and ocean depths are both quoted relative to this.
pub const SEA_LEVEL_BLOCKS: i64 = 0;

/// Horizontal distance in blocks from the equator to a pole.
///
/// Equal to the half-extent by construction: the poles sit exactly at the Z
/// bounds.
pub const POLE_TO_EQUATOR_BLOCKS: i64 = WORLD_HALF_EXTENT_BLOCKS;

/// Is this block column inside the world's horizontal bounds?
#[inline]
pub fn in_bounds_xz(x: i64, z: i64) -> bool {
    (-WORLD_HALF_EXTENT_BLOCKS..WORLD_HALF_EXTENT_BLOCKS).contains(&x)
        && (-WORLD_HALF_EXTENT_BLOCKS..WORLD_HALF_EXTENT_BLOCKS).contains(&z)
}

/// Is this block position inside the world, vertically included?
#[inline]
pub fn in_bounds(x: i64, y: i64, z: i64) -> bool {
    in_bounds_xz(x, z) && (WORLD_Y_MIN_BLOCKS..WORLD_Y_MAX_BLOCKS).contains(&y)
}

/// Does this chunk lie inside the world?
///
/// Every bound is a multiple of [`CHUNK_SIZE`], so a chunk is either wholly in
/// or wholly out — there are no straddling chunks to reason about.
#[inline]
pub fn contains_chunk(pos: ChunkPos) -> bool {
    let s = CHUNK_SIZE as i64;
    let (hx, ylo, yhi) = (
        WORLD_HALF_EXTENT_BLOCKS / s,
        WORLD_Y_MIN_BLOCKS / s,
        WORLD_Y_MAX_BLOCKS / s,
    );
    (-hx..hx).contains(&pos.x) && (-hx..hx).contains(&pos.z) && (ylo..yhi).contains(&pos.y)
}

/// Clamp a block column into the world's horizontal bounds.
#[inline]
pub fn clamp_xz(x: i64, z: i64) -> (i64, i64) {
    let lo = -WORLD_HALF_EXTENT_BLOCKS;
    let hi = WORLD_HALF_EXTENT_BLOCKS - 1;
    (x.clamp(lo, hi), z.clamp(lo, hi))
}

/// Latitude as a signed fraction: `-1.0` at the south pole, `0.0` at the
/// equator, `+1.0` at the north pole.
///
/// The form climate wants (M11) — no trigonometry, no degrees, and it composes
/// directly into interpolation. Clamped, so out-of-bounds Z saturates at a pole
/// instead of running past it.
#[inline]
pub fn latitude_fraction(z: i64) -> f64 {
    (z as f64 / POLE_TO_EQUATOR_BLOCKS as f64).clamp(-1.0, 1.0)
}

/// Latitude in degrees: `-90.0` south, `0.0` at the equator, `+90.0` north.
///
/// For display and for anything expressed in real-world terms. Prefer
/// [`latitude_fraction`] inside generation.
#[inline]
pub fn latitude_degrees(z: i64) -> f64 {
    latitude_fraction(z) * 90.0
}

/// Find the first position satisfying `accept`, searching outward from a start.
///
/// The predicate is the point. M10 passes "is land above sea level" from the
/// world centre, so a new player opens at the equator on habitable ground. M11
/// passes "is land AND is temperate forest" for biome-selected spawns — a
/// different argument, not a different function.
///
/// Deterministic from its inputs alone: candidates are visited in a fixed
/// order (square rings of increasing radius, each walked in a fixed direction),
/// so the same seed and predicate always yield the same spawn. Out-of-bounds
/// candidates are skipped rather than clamped, which would test the same edge
/// column repeatedly.
///
/// `stride` is the spacing between candidates — searching every block across a
/// 200 000-block world would be absurd, and spawn does not need block
/// precision. Returns `None` if nothing satisfies `accept` within `max_rings`.
pub fn find_spawn(
    start_x: i64,
    start_z: i64,
    stride: i64,
    max_rings: i64,
    mut accept: impl FnMut(i64, i64) -> bool,
) -> Option<(i64, i64)> {
    assert!(stride > 0, "spawn search stride must be positive");
    if in_bounds_xz(start_x, start_z) && accept(start_x, start_z) {
        return Some((start_x, start_z));
    }
    for ring in 1..=max_rings {
        let d = ring * stride;
        // North edge west→east, then east edge north→south, then south edge
        // east→west, then west edge south→north. Corners belong to the
        // horizontal edges, so each is visited exactly once.
        let xs: Vec<i64> = (start_x - d..=start_x + d)
            .step_by(stride as usize)
            .collect();
        let zs: Vec<i64> = (start_z - d + stride..start_z + d)
            .step_by(stride as usize)
            .collect();
        let mut candidates = Vec::with_capacity(2 * (xs.len() + zs.len()));
        candidates.extend(xs.iter().map(|&x| (x, start_z - d)));
        candidates.extend(zs.iter().map(|&z| (start_x + d, z)));
        candidates.extend(xs.iter().rev().map(|&x| (x, start_z + d)));
        candidates.extend(zs.iter().rev().map(|&z| (start_x - d, z)));
        for (x, z) in candidates {
            if in_bounds_xz(x, z) && accept(x, z) {
                return Some((x, z));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_exact_at_every_edge() {
        let h = WORLD_HALF_EXTENT_BLOCKS;
        assert!(in_bounds_xz(0, 0));
        assert!(in_bounds_xz(-h, -h), "the low corner is INSIDE (half-open)");
        assert!(in_bounds_xz(h - 1, h - 1));
        assert!(!in_bounds_xz(h, 0), "the high edge is OUTSIDE (half-open)");
        assert!(!in_bounds_xz(0, h));
        assert!(!in_bounds_xz(-h - 1, 0));
        assert!(!in_bounds_xz(0, -h - 1));
    }

    #[test]
    fn vertical_bounds_clear_everest_and_the_trench() {
        assert!(in_bounds(0, 8_848, 0), "Everest must fit");
        assert!(in_bounds(0, -10_935, 0), "Challenger Deep must fit");
        assert!(in_bounds(0, WORLD_Y_MIN_BLOCKS, 0));
        assert!(in_bounds(0, WORLD_Y_MAX_BLOCKS - 1, 0));
        assert!(!in_bounds(0, WORLD_Y_MAX_BLOCKS, 0));
        assert!(!in_bounds(0, WORLD_Y_MIN_BLOCKS - 1, 0));
    }

    /// Every bound must land on a chunk edge, or chunks straddle the boundary
    /// and every containment question grows a special case.
    #[test]
    fn every_bound_is_chunk_aligned() {
        let s = CHUNK_SIZE as i64;
        assert_eq!(WORLD_HALF_EXTENT_BLOCKS % s, 0);
        assert_eq!(WORLD_Y_MIN_BLOCKS % s, 0);
        assert_eq!(WORLD_Y_MAX_BLOCKS % s, 0);
    }

    #[test]
    fn chunk_containment_matches_block_containment() {
        let s = CHUNK_SIZE as i64;
        let hx = WORLD_HALF_EXTENT_BLOCKS / s;
        assert!(contains_chunk(ChunkPos::new(0, 0, 0)));
        assert!(contains_chunk(ChunkPos::new(-hx, 0, -hx)));
        assert!(contains_chunk(ChunkPos::new(hx - 1, 0, hx - 1)));
        assert!(!contains_chunk(ChunkPos::new(hx, 0, 0)));
        assert!(!contains_chunk(ChunkPos::new(0, 0, -hx - 1)));
        assert!(!contains_chunk(ChunkPos::new(0, WORLD_Y_MAX_BLOCKS / s, 0)));
        assert!(contains_chunk(ChunkPos::new(0, WORLD_Y_MIN_BLOCKS / s, 0)));
    }

    #[test]
    fn latitude_runs_pole_to_pole_through_the_equator() {
        assert_eq!(latitude_degrees(0), 0.0);
        assert_eq!(latitude_degrees(POLE_TO_EQUATOR_BLOCKS), 90.0);
        assert_eq!(latitude_degrees(-POLE_TO_EQUATOR_BLOCKS), -90.0);
        assert!((latitude_degrees(POLE_TO_EQUATOR_BLOCKS / 2) - 45.0).abs() < 1e-9);
        // Symmetric about the equator.
        for z in [1, 7_919, 50_000, 99_999] {
            assert!((latitude_fraction(z) + latitude_fraction(-z)).abs() < 1e-12);
        }
    }

    /// Past a pole, latitude saturates rather than continuing to climb —
    /// otherwise a clamped position and an unclamped one disagree about climate.
    #[test]
    fn latitude_saturates_beyond_the_poles() {
        assert_eq!(latitude_fraction(POLE_TO_EQUATOR_BLOCKS * 3), 1.0);
        assert_eq!(latitude_fraction(-POLE_TO_EQUATOR_BLOCKS * 3), -1.0);
    }

    #[test]
    fn clamping_lands_inside() {
        let h = WORLD_HALF_EXTENT_BLOCKS;
        assert_eq!(clamp_xz(0, 0), (0, 0));
        assert_eq!(clamp_xz(h * 2, -h * 2), (h - 1, -h));
        let (x, z) = clamp_xz(i64::MAX, i64::MIN);
        assert!(in_bounds_xz(x, z));
    }

    #[test]
    fn spawn_takes_the_start_when_it_already_qualifies() {
        assert_eq!(find_spawn(0, 0, 64, 100, |_, _| true), Some((0, 0)));
    }

    /// The case that motivates the search: the world centre falls in ocean.
    #[test]
    fn spawn_searches_outward_when_the_centre_is_water() {
        // "Land" is everything at least 500 blocks east of the origin.
        let spawn = find_spawn(0, 0, 64, 100, |x, _| x >= 500).expect("land exists");
        assert!(spawn.0 >= 500);
        // And it is the NEAREST such ring, not an arbitrary one: ring 8 is the
        // first whose east edge reaches 512.
        assert_eq!(spawn.0, 512);
    }

    /// Same inputs, same answer — a player must not get a different world
    /// opening on a second launch.
    #[test]
    fn spawn_is_deterministic() {
        let p = |x: i64, z: i64| (x * 31 + z * 17).rem_euclid(97) == 0;
        let a = find_spawn(0, 0, 16, 200, p);
        let b = find_spawn(0, 0, 16, 200, p);
        assert_eq!(a, b);
        assert!(a.is_some());
    }

    /// Candidates outside the world are skipped, never clamped — clamping
    /// would re-test the same edge column on every ring and could return a
    /// position the predicate never actually accepted in place.
    #[test]
    fn spawn_never_returns_an_out_of_bounds_position() {
        let h = WORLD_HALF_EXTENT_BLOCKS;
        let mut tested_outside = false;
        let spawn = find_spawn(h - 1, h - 1, 64, 50, |x, z| {
            if !in_bounds_xz(x, z) {
                tested_outside = true;
            }
            x < h - 1_000
        });
        let (sx, sz) = spawn.expect("land exists inward of the corner");
        assert!(in_bounds_xz(sx, sz));
        assert!(
            !tested_outside,
            "the predicate saw an out-of-bounds candidate"
        );
    }

    #[test]
    fn spawn_gives_up_rather_than_looping_forever() {
        assert_eq!(find_spawn(0, 0, 64, 10, |_, _| false), None);
    }

    /// A spawn at the world centre is equatorial, which is the point of putting
    /// it there: the first ground a player sees is warm and habitable.
    #[test]
    fn the_world_centre_is_on_the_equator() {
        assert_eq!(latitude_degrees(0), 0.0);
        assert!(in_bounds_xz(0, 0));
    }
}
