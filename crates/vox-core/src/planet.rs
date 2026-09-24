//! The world's fixed shape: its size, how its horizontal axes wrap, what a Z
//! coordinate *means*, and where a player starts (M10 task 1, amendment A2).
//!
//! Distinct from [`crate::world::World`], which is the runtime store of loaded
//! chunks. Nothing here holds state or allocates — it is the definition of the
//! world, queried by the chunk generator, the LOD sampler and the save layer.
//!
//! ## The world is a torus (ADR-0012)
//!
//! Both horizontal axes wrap; Y does not. Walk in any cardinal direction and
//! you eventually arrive back where you started. There is no edge anywhere.
//!
//! ## The seam lives only where CONTENT is addressed
//!
//! This is the design decision that makes the torus safe to build, and it is
//! easy to undo by accident, so it is stated first.
//!
//! The player, the camera and every loaded chunk live in coordinates that
//! **never wrap**. Walk east past the far edge of the world and x keeps
//! counting — 204 800, 204 801, … — rather than jumping back to 0. Streaming,
//! LOD rings, rendering, physics and the raycast therefore measure distance
//! with plain subtraction and never meet a discontinuity.
//!
//! The world's *content* is periodic instead. Exactly three things map an
//! unwrapped position to its canonical one, and they are the only places the
//! seam exists:
//!
//! 1. **Terrain generation** — noise tiles with the world's period, so a chunk
//!    at x and a chunk at x + size generate identical ground.
//! 2. **The save layer** — chunk files are keyed by canonical position, so an
//!    edit made on one lap is found on the next.
//! 3. **The LOD edit overlay** — player edits are keyed canonically for the
//!    same reason.
//!
//! This works because the view never reaches halfway round the world
//! ([`WorldShape::max_view_distance_blocks`]), so the same place can never be
//! loaded twice under two different unwrapped positions at once.
//!
//! **Do not canonicalise positions anywhere else.** Doing so reintroduces the
//! seam into code that is currently free of it.
//!
//! ## Latitude loops, and is equal-area
//!
//! One lap north passes equator → north pole → equator → south pole → equator:
//! the same sequence of climates as walking a great circle through both poles
//! of a real sphere. `sin(latitude)` is a triangle wave in Z, which makes every
//! latitude band cover the same share of the world it does on Earth — 8.2%
//! polar, 39.8% tropical — rather than the 26% / 26% an evenly spaced latitude
//! would give.
//!
//! ## One block is one metre, in every direction
//!
//! Vertical scale is not compressed: [`WORLD_Y_MAX_BLOCKS`] clears Everest and
//! [`WORLD_Y_MIN_BLOCKS`] clears Challenger Deep. Landform scale is tuned to
//! walking time instead (ADR-0010).

use std::fmt;

use crate::coords::{CHUNK_SIZE, ChunkPos};

/// Every world size is a whole multiple of this many blocks, on both axes.
///
/// The hard requirement is only that sizes be whole chunks, so chunk
/// coordinates wrap exactly for the save layer. The coarser step keeps size
/// choices to a sensible menu. (ADR-0012 first justified it as stopping LOD
/// nodes straddling the seam misaligned; under the unwrapped frame, nodes just
/// sample periodic terrain, and alignment no longer matters.)
pub const WORLD_SIZE_QUANTUM_BLOCKS: i64 = 8_192;

/// The default world: 25 quanta square, 204.8 km each way.
pub const DEFAULT_WORLD_SIZE_BLOCKS: i64 = 25 * WORLD_SIZE_QUANTUM_BLOCKS;

/// Smallest legal world. Four quanta (32.8 km) keeps at least a few landforms
/// in each direction; smaller would be a single tile of noise.
pub const MIN_WORLD_SIZE_BLOCKS: i64 = 4 * WORLD_SIZE_QUANTUM_BLOCKS;

/// Largest legal world. 2 097 km each way — far beyond anything planned, and
/// well inside what `i64` block coordinates and `f64` positions represent
/// exactly. A policy ceiling, not an engine limit.
pub const MAX_WORLD_SIZE_BLOCKS: i64 = 256 * WORLD_SIZE_QUANTUM_BLOCKS;

/// Lowest block coordinate in the world. Clears Challenger Deep (−10 935 m)
/// with room for seabed beneath it. A multiple of [`CHUNK_SIZE`].
pub const WORLD_Y_MIN_BLOCKS: i64 = -11_264;

/// One past the highest block coordinate in the world. Clears Everest
/// (8 848 m). A multiple of [`CHUNK_SIZE`].
pub const WORLD_Y_MAX_BLOCKS: i64 = 9_216;

/// Y at which oceans fill. Land elevations and ocean depths are both quoted
/// relative to this.
pub const SEA_LEVEL_BLOCKS: i64 = 0;

/// Is this Y inside the world? Horizontally every position is in the world —
/// it wraps — so this is the only bound there is.
#[inline]
pub fn in_vertical_bounds(y: i64) -> bool {
    (WORLD_Y_MIN_BLOCKS..WORLD_Y_MAX_BLOCKS).contains(&y)
}

/// Does this chunk lie within the world's vertical bounds? Every bound is a
/// multiple of [`CHUNK_SIZE`], so a chunk is wholly in or wholly out.
#[inline]
pub fn chunk_in_vertical_bounds(pos: ChunkPos) -> bool {
    let s = CHUNK_SIZE as i64;
    (WORLD_Y_MIN_BLOCKS / s..WORLD_Y_MAX_BLOCKS / s).contains(&pos.y)
}

/// Why a requested world size was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldShapeError {
    /// Not a whole multiple of [`WORLD_SIZE_QUANTUM_BLOCKS`].
    NotQuantised(i64),
    TooSmall(i64),
    TooLarge(i64),
}

impl fmt::Display for WorldShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotQuantised(s) => write!(
                f,
                "world size {s} is not a multiple of {WORLD_SIZE_QUANTUM_BLOCKS} blocks"
            ),
            Self::TooSmall(s) => write!(
                f,
                "world size {s} is below the minimum of {MIN_WORLD_SIZE_BLOCKS} blocks"
            ),
            Self::TooLarge(s) => write!(
                f,
                "world size {s} is above the maximum of {MAX_WORLD_SIZE_BLOCKS} blocks"
            ),
        }
    }
}

impl std::error::Error for WorldShapeError {}

/// The size of one world — its period on each horizontal axis.
///
/// Chosen when a world is created and recorded in `world.meta`; nothing may
/// assume a fixed size. Only constructible through [`WorldShape::new`], so
/// every value in circulation is a legal size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldShape {
    size_x: i64,
    size_z: i64,
}

impl WorldShape {
    /// The default world: 204.8 km square.
    pub const DEFAULT: Self = Self {
        size_x: DEFAULT_WORLD_SIZE_BLOCKS,
        size_z: DEFAULT_WORLD_SIZE_BLOCKS,
    };

    /// A world of the given size in blocks on each axis.
    pub fn new(size_x: i64, size_z: i64) -> Result<Self, WorldShapeError> {
        for s in [size_x, size_z] {
            if s % WORLD_SIZE_QUANTUM_BLOCKS != 0 {
                return Err(WorldShapeError::NotQuantised(s));
            }
            if s < MIN_WORLD_SIZE_BLOCKS {
                return Err(WorldShapeError::TooSmall(s));
            }
            if s > MAX_WORLD_SIZE_BLOCKS {
                return Err(WorldShapeError::TooLarge(s));
            }
        }
        Ok(Self { size_x, size_z })
    }

    /// World size along X, in blocks.
    #[inline]
    pub fn size_x(&self) -> i64 {
        self.size_x
    }

    /// World size along Z, in blocks.
    #[inline]
    pub fn size_z(&self) -> i64 {
        self.size_z
    }

    /// The canonical X for a position: `[0, size_x)`. **Content addressing
    /// only** — see the module docs before calling this anywhere else.
    #[inline]
    pub fn canonical_x(&self, x: i64) -> i64 {
        x.rem_euclid(self.size_x)
    }

    /// The canonical Z for a position: `[0, size_z)`. Content addressing only.
    #[inline]
    pub fn canonical_z(&self, z: i64) -> i64 {
        z.rem_euclid(self.size_z)
    }

    /// The canonical position of a chunk. Y is untouched — it does not wrap.
    /// Content addressing only.
    #[inline]
    pub fn canonical_chunk(&self, pos: ChunkPos) -> ChunkPos {
        let s = CHUNK_SIZE as i64;
        ChunkPos::new(
            pos.x.rem_euclid(self.size_x / s),
            pos.y,
            pos.z.rem_euclid(self.size_z / s),
        )
    }

    /// Shortest signed separation from `from` to `to` along X, in
    /// `(-size_x / 2, size_x / 2]`.
    ///
    /// For comparing positions that may be on DIFFERENT laps — a saved home
    /// marker against the player's current unwrapped position, say. Positions
    /// in the loaded world share a lap and should be subtracted directly.
    #[inline]
    pub fn delta_x(&self, from: i64, to: i64) -> i64 {
        shortest(to - from, self.size_x)
    }

    /// Shortest signed separation from `from` to `to` along Z. See
    /// [`Self::delta_x`].
    #[inline]
    pub fn delta_z(&self, from: i64, to: i64) -> i64 {
        shortest(to - from, self.size_z)
    }

    /// Distance from the equator to a pole, in blocks: a quarter of the Z
    /// period, since one lap of Z passes both poles.
    #[inline]
    pub fn pole_to_equator_blocks(&self) -> i64 {
        self.size_z / 4
    }

    /// `sin(latitude)`, in `[-1, 1]`: 0 at the equators, +1 at the north pole,
    /// −1 at the south pole.
    ///
    /// The form climate should consume. Being linear in Z between the poles is
    /// exactly what makes the mapping equal-area, and insolation is naturally
    /// expressed in it.
    pub fn latitude_sine(&self, z: i64) -> f64 {
        let u = self.canonical_z(z) as f64 / self.size_z as f64;
        // Triangle wave over one lap: 0 → +1 → 0 → −1 → 0.
        if u < 0.25 {
            4.0 * u
        } else if u < 0.75 {
            2.0 - 4.0 * u
        } else {
            4.0 * u - 4.0
        }
    }

    /// Latitude in degrees: `-90.0` at the south pole, `0.0` at the equators,
    /// `+90.0` at the north pole. For display and real-world reasoning; prefer
    /// [`Self::latitude_sine`] inside generation.
    pub fn latitude_degrees(&self, z: i64) -> f64 {
        self.latitude_sine(z).clamp(-1.0, 1.0).asin().to_degrees()
    }

    /// The farthest the view may ever reach, in blocks: strictly less than half
    /// the smaller period (ADR-0012 rule 3).
    ///
    /// Two reasons, both load-bearing. Past half the world the same terrain is
    /// visible twice, around the world in both directions. And the unwrapped
    /// coordinate scheme above relies on no place ever being loaded under two
    /// positions at once.
    #[inline]
    pub fn max_view_distance_blocks(&self) -> i64 {
        self.size_x.min(self.size_z) / 2 - CHUNK_SIZE as i64
    }

    /// How many noise cells of roughly `target_wavelength` blocks fit around
    /// the world along X. At least three — see `MIN_CELLS_AROUND`.
    ///
    /// Noise tiles with the world only if a whole number of cells fits around
    /// it, so each octave rounds its wavelength to the nearest size that does.
    /// At the default size that is a change of a few percent; it works
    /// identically at any legal size (ADR-0012 rule 1).
    #[inline]
    pub fn cells_around_x(&self, target_wavelength: i64) -> u32 {
        cells_around(self.size_x, target_wavelength)
    }

    /// As [`Self::cells_around_x`], along Z.
    #[inline]
    pub fn cells_around_z(&self, target_wavelength: i64) -> u32 {
        cells_around(self.size_z, target_wavelength)
    }
}

impl Default for WorldShape {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn shortest(d: i64, period: i64) -> i64 {
    let d = d.rem_euclid(period);
    if d > period / 2 { d - period } else { d }
}

/// The fewest noise cells an octave may have around the world.
///
/// Not 1: a lattice with one cell around the world has one value, repeated
/// everywhere, so the octave stops varying at all. The continent octave's target
/// wavelength (56 km) is larger than the smallest legal world, and at one cell
/// that world came out as a single uninterrupted ocean. Three is the smallest
/// count that still produces highs and lows on every axis.
const MIN_CELLS_AROUND: i64 = 3;

fn cells_around(period: i64, target_wavelength: i64) -> u32 {
    assert!(target_wavelength > 0, "noise wavelength must be positive");
    let n = (period as f64 / target_wavelength as f64).round() as i64;
    n.clamp(MIN_CELLS_AROUND, u32::MAX as i64) as u32
}

/// Find the first position satisfying `accept`, searching outward from a start.
///
/// The predicate is the point. M10 passes "is land above sea level" from the
/// world's origin, so a new player opens on the equator on habitable ground.
/// M12 passes "is land AND is temperate forest" for biome-selected spawns — a
/// different argument, not a different function.
///
/// Deterministic from its inputs alone: candidates are visited in a fixed order
/// (square rings of increasing radius, each walked in a fixed direction), so
/// the same seed and predicate always yield the same spawn. The search stops at
/// half the world, beyond which it would only revisit places already tried.
///
/// The result is the UNWRAPPED position the search found, near the start — not
/// the canonical one. Spawn is a position in the player's own frame, where
/// coordinates never wrap; canonicalising here would be exactly the misuse the
/// module docs warn against. Concretely: land found 20 km west of an origin
/// start is reported at x = −20 000, not x = 184 800, keeping the player near
/// the origin where positions are small. Display and saving canonicalise for
/// themselves.
///
/// `stride` is the spacing between candidates — spawn needs a continent, not a
/// block. Returns `None` if nothing satisfies `accept`.
pub fn find_spawn(
    shape: WorldShape,
    start_x: i64,
    start_z: i64,
    stride: i64,
    max_rings: i64,
    mut accept: impl FnMut(i64, i64) -> bool,
) -> Option<(i64, i64)> {
    assert!(stride > 0, "spawn search stride must be positive");
    if accept(start_x, start_z) {
        return Some((start_x, start_z));
    }
    let reach = shape.size_x.min(shape.size_z) / 2;
    for ring in 1..=max_rings {
        let d = ring * stride;
        if d > reach {
            break;
        }
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
            if accept(x, z) {
                return Some((x, z));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: WorldShape = WorldShape::DEFAULT;

    #[test]
    fn the_default_world_is_legal_and_quantised() {
        assert_eq!(W.size_x(), 204_800);
        assert_eq!(W.size_z(), 204_800);
        assert_eq!(WorldShape::new(204_800, 204_800), Ok(W));
        assert_eq!(WorldShape::default(), W);
    }

    #[test]
    fn illegal_sizes_are_rejected() {
        assert_eq!(
            WorldShape::new(200_000, 204_800),
            Err(WorldShapeError::NotQuantised(200_000))
        );
        assert_eq!(
            WorldShape::new(WORLD_SIZE_QUANTUM_BLOCKS, MIN_WORLD_SIZE_BLOCKS),
            Err(WorldShapeError::TooSmall(WORLD_SIZE_QUANTUM_BLOCKS))
        );
        let big = MAX_WORLD_SIZE_BLOCKS + WORLD_SIZE_QUANTUM_BLOCKS;
        assert_eq!(
            WorldShape::new(MIN_WORLD_SIZE_BLOCKS, big),
            Err(WorldShapeError::TooLarge(big))
        );
        // Non-square worlds are legal, even though the default is square.
        assert!(WorldShape::new(MIN_WORLD_SIZE_BLOCKS, MAX_WORLD_SIZE_BLOCKS).is_ok());
    }

    /// Every bound must land on a chunk edge, and so must every world size,
    /// or chunk coordinates would not wrap exactly.
    #[test]
    fn every_bound_and_size_is_chunk_aligned() {
        let s = CHUNK_SIZE as i64;
        assert_eq!(WORLD_SIZE_QUANTUM_BLOCKS % s, 0);
        assert_eq!(WORLD_Y_MIN_BLOCKS % s, 0);
        assert_eq!(WORLD_Y_MAX_BLOCKS % s, 0);
    }

    #[test]
    fn vertical_bounds_clear_everest_and_the_trench() {
        assert!(in_vertical_bounds(8_848), "Everest must fit");
        assert!(in_vertical_bounds(-10_935), "Challenger Deep must fit");
        assert!(in_vertical_bounds(WORLD_Y_MIN_BLOCKS));
        assert!(in_vertical_bounds(WORLD_Y_MAX_BLOCKS - 1));
        assert!(!in_vertical_bounds(WORLD_Y_MAX_BLOCKS));
        assert!(!in_vertical_bounds(WORLD_Y_MIN_BLOCKS - 1));
        let s = CHUNK_SIZE as i64;
        assert!(chunk_in_vertical_bounds(ChunkPos::new(
            0,
            WORLD_Y_MIN_BLOCKS / s,
            0
        )));
        assert!(!chunk_in_vertical_bounds(ChunkPos::new(
            0,
            WORLD_Y_MAX_BLOCKS / s,
            0
        )));
        // Horizontally, everything is in the world.
        assert!(chunk_in_vertical_bounds(ChunkPos::new(
            i64::MAX / 64,
            0,
            i64::MIN / 64
        )));
    }

    #[test]
    fn canonicalisation_wraps_negatives_and_laps() {
        let p = W.size_x();
        for (x, want) in [
            (0, 0),
            (p - 1, p - 1),
            (p, 0),
            (-1, p - 1),
            (3 * p + 17, 17),
            (-2 * p - 5, p - 5),
        ] {
            assert_eq!(W.canonical_x(x), want, "canonical_x({x})");
            assert_eq!(W.canonical_z(x), want, "canonical_z({x})");
        }
        // Idempotent.
        assert_eq!(
            W.canonical_x(W.canonical_x(-12_345)),
            W.canonical_x(-12_345)
        );
    }

    /// Canonicalising a chunk must agree with canonicalising the blocks inside
    /// it — the save layer relies on this to find a chunk from either side.
    #[test]
    fn chunk_and_block_canonicalisation_agree() {
        let s = CHUNK_SIZE as i64;
        for x in [
            -3 * W.size_x(),
            -1,
            0,
            31,
            32,
            W.size_x() - 1,
            W.size_x(),
            5 * W.size_x() + 700,
        ] {
            let chunk = ChunkPos::new(x.div_euclid(s), 7, (-x).div_euclid(s));
            let c = W.canonical_chunk(chunk);
            assert_eq!(c.x, W.canonical_x(x).div_euclid(s));
            assert_eq!(c.z, W.canonical_z(-x).div_euclid(s));
            assert_eq!(c.y, 7, "Y must never wrap");
        }
    }

    #[test]
    fn delta_takes_the_short_way_round() {
        let p = W.size_x();
        assert_eq!(W.delta_x(0, 10), 10);
        assert_eq!(W.delta_x(10, 0), -10);
        // Across the seam: from just west of it to just east is a short step.
        assert_eq!(W.delta_x(p - 10, 5), 15);
        assert_eq!(W.delta_x(5, p - 10), -15);
        // Different laps of the same place are no distance apart.
        assert_eq!(W.delta_x(123, 123 + 4 * p), 0);
        // Never longer than half the world.
        for (a, b) in [(0, p / 2), (0, p / 2 + 1), (7, 7 + p / 2 - 1)] {
            assert!(W.delta_x(a, b).abs() <= p / 2);
            assert_eq!(W.delta_z(a, b), W.delta_x(a, b));
        }
    }

    #[test]
    fn latitude_loops_through_both_poles() {
        let p = W.size_z();
        assert_eq!(W.latitude_degrees(0), 0.0);
        assert!(
            (W.latitude_degrees(p / 4) - 90.0).abs() < 1e-9,
            "north pole"
        );
        assert!(W.latitude_degrees(p / 2).abs() < 1e-9, "the far equator");
        assert!(
            (W.latitude_degrees(3 * p / 4) + 90.0).abs() < 1e-9,
            "south pole"
        );
        // Periodic, including negative and multi-lap coordinates.
        for z in [1, 12_345, p / 3, p - 1] {
            let lat = W.latitude_degrees(z);
            assert!((W.latitude_degrees(z + p) - lat).abs() < 1e-9);
            assert!((W.latitude_degrees(z - 3 * p) - lat).abs() < 1e-9);
        }
        assert_eq!(W.pole_to_equator_blocks(), p / 4);
    }

    /// The equal-area property, measured: the share of the world in each band
    /// matches Earth's, not the 26% / 26% an even spacing would give.
    #[test]
    fn latitude_is_equal_area() {
        let p = W.size_z();
        let n = 100_000;
        let (mut polar, mut tropical) = (0, 0);
        for i in 0..n {
            let lat = W.latitude_degrees(i * p / n).abs();
            if lat > 66.56 {
                polar += 1;
            }
            if lat < 23.44 {
                tropical += 1;
            }
        }
        let polar = polar as f64 / n as f64;
        let tropical = tropical as f64 / n as f64;
        assert!(
            (polar - 0.082).abs() < 0.003,
            "polar share {polar:.3}, Earth's is 0.082"
        );
        assert!(
            (tropical - 0.398).abs() < 0.003,
            "tropical share {tropical:.3}, Earth's is 0.398"
        );
    }

    #[test]
    fn the_view_never_reaches_halfway_round() {
        for size in [
            MIN_WORLD_SIZE_BLOCKS,
            DEFAULT_WORLD_SIZE_BLOCKS,
            MAX_WORLD_SIZE_BLOCKS,
        ] {
            let w = WorldShape::new(size, size).unwrap();
            assert!(w.max_view_distance_blocks() < size / 2);
            assert!(w.max_view_distance_blocks() > 0);
        }
    }

    /// Noise tiles only if a whole number of cells fits around the world. The
    /// rounded wavelength must stay close to the target at the default size,
    /// and degrade gracefully in a tiny world.
    #[test]
    fn noise_cells_fit_a_whole_number_of_times() {
        for target in [90, 800, 7_000, 34_000, 56_000] {
            let n = W.cells_around_x(target);
            assert!(n >= 1);
            let actual = W.size_x() as f64 / n as f64;
            assert!(
                (actual / target as f64 - 1.0).abs() < 0.10,
                "wavelength {target} became {actual:.0}"
            );
        }
        // An octave far larger than the world still varies: never fewer than
        // three cells, or it collapses to one repeated value.
        let tiny = WorldShape::new(MIN_WORLD_SIZE_BLOCKS, MIN_WORLD_SIZE_BLOCKS).unwrap();
        assert_eq!(tiny.cells_around_x(10_000_000), 3);
    }

    #[test]
    fn spawn_takes_the_start_when_it_already_qualifies() {
        assert_eq!(find_spawn(W, 0, 0, 64, 100, |_, _| true), Some((0, 0)));
    }

    /// The case that motivates the search: the world's origin falls in ocean.
    #[test]
    fn spawn_searches_outward_when_the_start_is_water() {
        let spawn = find_spawn(W, 0, 0, 64, 100, |x, _| x >= 500).expect("land exists");
        // The nearest ring whose east edge reaches 500 is ring 8, at 512.
        assert_eq!(spawn.0, 512);
    }

    /// Land just across the seam is found, and reported where the search found
    /// it — near the start, in the player's unwrapped frame — rather than
    /// canonicalised to the far side of the world.
    #[test]
    fn spawn_across_the_seam_stays_near_the_start() {
        let p = W.size_x();
        // "Land" is the canonical strip just west of the seam: x in
        // [p - 600, p - 400), which is x in [-600, -400) one lap back.
        let spawn = find_spawn(W, 0, 0, 64, 100, |x, _| {
            (p - 600..p - 400).contains(&W.canonical_x(x))
        })
        .expect("land exists across the seam");
        assert!(
            (-600..-400).contains(&spawn.0),
            "expected the nearby unwrapped position, got {spawn:?}"
        );
    }

    #[test]
    fn spawn_is_deterministic() {
        let p = |x: i64, z: i64| (x * 31 + z * 17).rem_euclid(97) == 0;
        let a = find_spawn(W, 0, 0, 16, 200, p);
        assert_eq!(a, find_spawn(W, 0, 0, 16, 200, p));
        assert!(a.is_some());
    }

    /// With nothing acceptable, the search ends at half the world rather than
    /// circling it forever.
    #[test]
    fn spawn_gives_up_at_half_the_world() {
        let mut calls = 0u64;
        let r = find_spawn(W, 0, 0, 4_096, 1_000_000, |_, _| {
            calls += 1;
            false
        });
        assert_eq!(r, None);
        let rings = W.size_x() / 2 / 4_096;
        assert!(calls <= 1 + 8 * (rings * (rings + 1) / 2) as u64);
    }
}
