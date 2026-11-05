//! Dune seas (M15 task 2, `docs/landforms.md` *Wind — dune seas*,
//! ADR-0021).
//!
//! A height layer of wind-blown sand where the wind process acts: ridges
//! 6–30 blocks high and 100–250 apart, their type set by the wind as
//! Fryberger's rule has it — one steady wind makes **transverse** ridges
//! across it (gentle windward slope, a steep lee face at sand's angle of
//! repose, flat interdunes between), a wind that swings with the seasons
//! makes **linear** ridges along it, winds from many directions where the
//! belts meet make **star** dunes, and where sand runs short at a sea's
//! edges the crests break into **barchans**, horns trailing downwind.
//!
//! ## The dune wind
//!
//! M13's winds blow along latitude rows. Earth's trades blow from the
//! north-east and south-east, so the dune wind is tilted toward the
//! equator in the trades (from ~ENE / ~ESE) and toward the poles in the
//! westerlies (from ~WSW / ~WNW) by [`TILT`] (owner's decision 3). Only the
//! dunes read it; rain, temperature and biomes are unchanged.
//!
//! ## Tiled with the world
//!
//! Every ridge is a wave whose wave vector is a whole number of cycles
//! around the torus along each axis ([`Wave`]), so the pattern repeats
//! exactly with the world's period — there is no seam to hide (ADR-0012).
//! That fixes each direction per wind belt and hemisphere quarter; the
//! direction cannot turn smoothly with latitude without bending the
//! ridges' spacing far from the origin. Belts blend where they meet (that
//! is where star dunes go), and the quarter boundaries — the equators and
//! the poles, never deserts — fade the dunes out.
//!
//! ## Pure, deterministic
//!
//! A function of `(x, z)` and the column's wind weight: basic arithmetic,
//! `floor`, and the elevation field's tiled value noise.

use vox_core::WorldShape;

use crate::climate::smoothstep;
use crate::elevation::{Elevation, LandformNoise};

/// The dune wind's tilt from the rows, radians (22.5° = π/8: from ENE in
/// the northern trades).
const TILT: f64 = std::f64::consts::FRAC_PI_8;
/// Its sine and cosine (no `sin` at run time: determinism).
const TILT_SIN: f64 = 0.382_683_43;
const TILT_COS: f64 = 0.923_879_53;
/// The two ridge spacings blended across a dune sea, blocks; ridges are
/// [`HEIGHT_PER_SPACING`] of their spacing high at full strength.
const SPACINGS: [f64; 2] = [120.0, 220.0];
const HEIGHT_PER_SPACING: f32 = 0.12;
/// Star dunes: spacing and height at full strength, blocks.
const STAR_SPACING: f64 = 420.0;
const STAR_HEIGHT: f32 = 55.0;
/// How much the belts must mix (`4w(1 − w)`, 1 where they mix equally)
/// before star dunes begin and are full.
const STAR_MEET: (f32, f32) = (0.6, 0.95);
/// Wavelength over which star dunes' sizes vary, blocks.
const STAR_SIZE_WAVELENGTH: i64 = 500;
/// A transverse ridge's windward slope's share of the spacing; the lee
/// face's slope (sand's angle of repose, ~33°: walkable at one block per
/// 1.5).
const WINDWARD_SHARE: f32 = 0.62;
const LEE_SLOPE: f32 = 0.65;
/// A linear ridge's half-width as a share of its spacing.
const LINEAR_HALF_WIDTH: f32 = 0.35;
/// Wavelengths of the noises, blocks: where a dune sea lies within the
/// wind's ground; which spacing; the crests' sinuous wander and its reach;
/// the barchans' broken crests.
const ERG_PATCH_WAVELENGTH: i64 = 9_000;
const SPACING_WAVELENGTH: i64 = 3_000;
const SINUOUS: [(i64, f64); 2] = [(450, 45.0), (160, 10.0)];
/// Star dunes stand alone, not on a lattice: their columns wander this far
/// at this wavelength before the phase, and where the size noise is under
/// the first value there is no star at all.
const STAR_WANDER: (i64, f64) = (700, 80.0);
const STAR_PRESENT: (f32, f32) = (-0.1, 0.5);
const BARCHAN_WAVELENGTH: i64 = 400;
const SWING_WAVELENGTH: i64 = 2_500;
/// The wind weight, scaled by the patch noise and the warmth, over which
/// a dune sea begins and is full; below the full value its crests break
/// into barchans. Tuned so dune seas cover ~20–30% of hot desert and ~10%
/// of cold (the catalogue; Earth's ergs are mostly in hot deserts).
const ERG: (f32, f32) = (0.55, 0.9);
const ERG_PATCH: (f32, f32) = (0.15, 1.4);
/// Warmth: cold deserts have less loose sand (frozen, vegetated, stony
/// ground); the share of a hot desert's erg a cold one keeps, and the
/// temperatures (°C) between.
const COLD_SHARE: f32 = 0.45;
const WARM_C: (f32, f32) = (10.0, 18.0);
/// Within this share of a lap of each equator and pole, dunes fade out:
/// the quarter's fixed wind direction changes there.
const QUARTER_FADE: (f64, f64) = (0.005, 0.02);
/// Seasonal swing of the dune wind (decision 1) by `|sin(latitude)|`: none
/// in the trades' core, full toward the deserts' poleward edges (~18–28°),
/// some in the westerlies' deserts.
const SWING_RAMP: (f32, f32) = (0.309, 0.469);
const WESTERLY_SWING: f32 = 0.3;

/// A plane wave tiled with the torus: `m` whole cycles around each axis.
#[derive(Clone, Copy, Debug)]
struct Wave {
    mx: f64,
    mz: f64,
}

impl Wave {
    /// The nearest tiled wave to direction `(dx, dz)` (a unit vector) at
    /// `spacing` blocks.
    fn new(shape: WorldShape, (dx, dz): (f64, f64), spacing: f64) -> Self {
        let (px, pz) = (shape.size_x() as f64, shape.size_z() as f64);
        Self {
            mx: (px * dx / spacing).round(),
            mz: (pz * dz / spacing).round(),
        }
    }

    /// The phase in `[0, 1)` at canonical `(x, z)` (fractional, warped):
    /// periodic in each with the world's size, since the cycle counts are
    /// whole.
    fn phase(&self, shape: WorldShape, x: f64, z: f64) -> f32 {
        let (px, pz) = (shape.size_x() as f64, shape.size_z() as f64);
        let a = self.mx * x / px;
        let b = self.mz * z / pz;
        let p = (a - a.floor()) + (b - b.floor());
        (p - p.floor()) as f32
    }
}

/// Transverse, linear and star waves for one wind direction.
#[derive(Clone, Copy, Debug)]
struct Belt {
    /// Along the wind (phase grows downwind), per spacing.
    along: [Wave; 2],
    /// Across it (crests along the wind), per spacing.
    across: [Wave; 2],
}

impl Belt {
    fn new(shape: WorldShape, (dx, dz): (f64, f64)) -> Self {
        Self {
            along: SPACINGS.map(|s| Wave::new(shape, (dx, dz), s)),
            across: SPACINGS.map(|s| Wave::new(shape, (-dz, dx), s)),
        }
    }
}

/// The dune layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Dunes {
    shape: WorldShape,
    /// Per quarter of the lap of Z (`latitude_sine` rising, falling,
    /// falling, rising): the trades' and the westerlies' dune winds.
    trades: [Belt; 4],
    westerlies: [Belt; 4],
    /// Star dunes' three crossing waves, 60° apart: peaks on a hexagonal
    /// lattice with an arm along each.
    star: [Wave; 3],
    noise: Noises,
}

/// The layer's noises, built once.
#[derive(Clone, Copy, Debug)]
struct Noises {
    patch: LandformNoise,
    sinuous: [[LandformNoise; 2]; 2],
    spacing: LandformNoise,
    barchan: LandformNoise,
    swing: LandformNoise,
    star_wander: [LandformNoise; 2],
    star_size: LandformNoise,
}

impl Noises {
    fn new(shape: WorldShape) -> Self {
        let n = |wavelength, salt| LandformNoise::new(shape, wavelength, salt);
        Self {
            patch: n(ERG_PATCH_WAVELENGTH, 0xD0_0001),
            sinuous: std::array::from_fn(|k| {
                let salt = 0xD0_0010 + 2 * k as u64;
                [n(SINUOUS[k].0, salt), n(SINUOUS[k].0, salt + 1)]
            }),
            spacing: n(SPACING_WAVELENGTH, 0xD0_0004),
            barchan: n(BARCHAN_WAVELENGTH, 0xD0_0005),
            swing: n(SWING_WAVELENGTH, 0xD0_0007),
            star_wander: [n(STAR_WANDER.0, 0xD0_0020), n(STAR_WANDER.0, 0xD0_0021)],
            star_size: n(STAR_SIZE_WAVELENGTH, 0xD0_0006),
        }
    }
}

/// What the layer adds at a column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dune {
    /// Blocks of sand above the ground.
    pub height: f32,
    /// The dune sea's strength here, `[0, 1]` (0 outside one).
    pub erg: f32,
    /// The dunes' type here, by the wind (the readout's name).
    pub kind: DuneKind,
}

/// Fryberger's dune types, by the wind that makes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DuneKind {
    /// One steady wind: ridges across it.
    #[default]
    Transverse,
    /// A wind that swings with the seasons: ridges along it.
    Linear,
    /// Winds from many directions, where the belts meet.
    Star,
    /// Sand running short at a sea's edge: crescents.
    Barchan,
}

impl DuneKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Transverse => "transverse dunes",
            Self::Linear => "linear dunes",
            Self::Star => "star dunes",
            Self::Barchan => "barchans",
        }
    }
}

impl Dunes {
    pub fn new(shape: WorldShape) -> Self {
        let (s, c) = (TILT_SIN, TILT_COS);
        debug_assert!((TILT.sin() - s).abs() < 1e-7 && (TILT.cos() - c).abs() < 1e-7);
        // Toward the equator along Z, per quarter: in the first quarter
        // latitude rises with Z, so the equator is toward −Z.
        let equatorward = |q: usize| if q.is_multiple_of(2) { -1.0 } else { 1.0 };
        let trades = std::array::from_fn(|q| Belt::new(shape, (-c, equatorward(q) * s)));
        let westerlies = std::array::from_fn(|q| Belt::new(shape, (c, -equatorward(q) * s)));
        let h = 0.866_025_403_784_438_6; // sin 60°
        Self {
            shape,
            trades,
            westerlies,
            star: [
                Wave::new(shape, (1.0, 0.0), STAR_SPACING),
                Wave::new(shape, (0.5, h), STAR_SPACING),
                Wave::new(shape, (-0.5, h), STAR_SPACING),
            ],
            noise: Noises::new(shape),
        }
    }

    /// The dune sand at a column whose wind weight is `wind`, at
    /// `|sin(latitude)|` `abs_s` with eastward-wind share `westerly`
    /// (`climate::eastward_share`).
    /// `temp_c` is the column's mean annual temperature.
    #[allow(clippy::too_many_arguments)] // one column's climate, each its own unit
    pub fn at(
        &self,
        e: &Elevation,
        x: i64,
        z: i64,
        wind: f32,
        abs_s: f32,
        westerly: f32,
        temp_c: f32,
    ) -> Dune {
        if wind <= 0.0 {
            return Dune::default();
        }
        let patch = 0.5 + 0.5 * e.noise(x, z, &self.noise.patch);
        let warmth = COLD_SHARE + (1.0 - COLD_SHARE) * smoothstep(WARM_C.0, WARM_C.1, temp_c);
        let erg = smoothstep(
            ERG.0,
            ERG.1,
            wind * (ERG_PATCH.0 + (ERG_PATCH.1 - ERG_PATCH.0) * patch) * warmth,
        );
        let u = self.shape.canonical_z(z) as f64 / self.shape.size_z() as f64;
        let quarter = ((u * 4.0).floor() as usize).min(3);
        let to_boundary = (u * 4.0 - (u * 4.0).round()).abs() / 4.0;
        let fade = smoothstep(
            QUARTER_FADE.0 as f32,
            QUARTER_FADE.1 as f32,
            to_boundary as f32,
        );
        let strength = erg * fade;
        if strength <= 0.0 {
            return Dune {
                height: 0.0,
                erg,
                kind: DuneKind::default(),
            };
        }

        // Sinuous crests: wander the column before the phase.
        let (mut sx, mut sz) = (0.0, 0.0);
        for (k, &(_, reach)) in SINUOUS.iter().enumerate() {
            sx += reach * f64::from(e.noise(x, z, &self.noise.sinuous[k][0]));
            sz += reach * f64::from(e.noise(x, z, &self.noise.sinuous[k][1]));
        }
        let wx = self.shape.canonical_x(x) as f64 + sx;
        let wz = self.shape.canonical_z(z) as f64 + sz;

        // Which spacing: blended across the sea, so ridges merge and split
        // at Y-junctions where the two meet, as real dune fields' do.
        let n = e.noise(x, z, &self.noise.spacing);
        let big = smoothstep(-0.3, 0.3, n);
        // Barchans at the edges: crests broken, the thin ends lagging
        // downwind as horns.
        // The crest is cut into segments one spacing long by the wave
        // across the wind, each segment's middle surviving longest — so the
        // pieces are crescents — and their sizes vary by a long noise
        // (short noise showed its lattice as squares).
        let barchan = 1.0 - smoothstep(0.35, 1.0, strength);
        let gap = -1.0 + 1.6 * barchan;
        let sizes = if gap + 0.35 <= 0.0 {
            1.0 // whole crests: no segment is cut
        } else {
            0.6 + 0.4 * e.noise(x, z, &self.noise.barchan)
        };

        // One dune type per place, by how much the wind swings: added
        // together, transverse and linear ridges make a lattice. The switch
        // wanders, so the boundary between types is not a latitude line.
        let swing = smoothstep(SWING_RAMP.0, SWING_RAMP.1, abs_s) * (1.0 - westerly)
            + WESTERLY_SWING * westerly;
        let swing = smoothstep(0.4, 0.6, swing + 0.25 * e.noise(x, z, &self.noise.swing));
        let belt = |b: &Belt| {
            let mut h = 0.0;
            for (k, weight) in [(0, 1.0 - big), (1, big)] {
                if weight <= 0.0 {
                    continue;
                }
                let spacing = SPACINGS[k] as f32;
                let ridge = HEIGHT_PER_SPACING * spacing;
                let across = b.across[k].phase(self.shape, wx, wz);
                let whole = smoothstep(gap, gap + 0.35, tent(across) * sizes);
                let phase = b.along[k].phase(self.shape, wx, wz);
                let phase = (phase - 0.2 * (1.0 - whole)).rem_euclid(1.0);
                let lee = (ridge / (LEE_SLOPE * spacing)).min(1.0 - WINDWARD_SHARE);
                let t = transverse(phase, lee) * whole;
                let l = linear(across);
                h += weight * ridge * (t + (l - t) * swing);
            }
            h
        };
        let trades = belt(&self.trades[quarter]);
        let westerlies = belt(&self.westerlies[quarter]);
        let steady = trades + (westerlies - trades) * westerly;
        // Where the belts meet the wind turns right round: star dunes.
        let meet = smoothstep(STAR_MEET.0, STAR_MEET.1, 4.0 * westerly * (1.0 - westerly));
        let star = if meet > 0.0 {
            let reach = STAR_WANDER.1;
            let tx = wx + reach * f64::from(e.noise(x, z, &self.noise.star_wander[0]));
            let tz = wz + reach * f64::from(e.noise(x, z, &self.noise.star_wander[1]));
            // Tents rounded, so the stars are mounds with arms, not facets.
            let [a, b, c] = self
                .star
                .map(|w| smoothstep(0.0, 1.0, tent(w.phase(self.shape, tx, tz))));
            // Each star its own size, as Earth's are, and many lattice
            // points none: the peak is the product's, the arms run along
            // the three waves' crests.
            let size = smoothstep(
                STAR_PRESENT.0,
                STAR_PRESENT.1,
                e.noise(x, z, &self.noise.star_size),
            );
            STAR_HEIGHT * size * (a * b * c).sqrt()
        } else {
            0.0
        };
        // Height rises fast at a sea's edge and saturates in its core: the
        // barchans' breaking does the thinning.
        let height = strength.sqrt() * (steady + (star - steady) * meet);
        let kind = if meet >= 0.5 {
            DuneKind::Star
        } else if barchan >= 0.5 {
            DuneKind::Barchan
        } else if swing >= 0.5 {
            DuneKind::Linear
        } else {
            DuneKind::Transverse
        };
        Dune { height, erg, kind }
    }
}

/// A transverse ridge's profile by phase downwind: a straight windward
/// slope over [`WINDWARD_SHARE`], the lee face over `lee`, then flat
/// interdune. 1 at the crest.
fn transverse(phase: f32, lee: f32) -> f32 {
    if phase < WINDWARD_SHARE {
        phase / WINDWARD_SHARE
    } else if phase < WINDWARD_SHARE + lee {
        1.0 - (phase - WINDWARD_SHARE) / lee
    } else {
        0.0
    }
}

/// A linear ridge's profile across it: symmetric, sharp-crested.
fn linear(phase: f32) -> f32 {
    (1.0 - (phase - 0.5).abs() / LINEAR_HALF_WIDTH).max(0.0)
}

/// A tent over one cycle: 0 at the ends, 1 in the middle.
fn tent(phase: f32) -> f32 {
    1.0 - (2.0 * phase - 1.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_tile_with_the_world_and_keep_their_spacing() {
        for size in [32_768, 204_800, 409_600] {
            let shape = WorldShape::new(size, size).unwrap();
            let w = Wave::new(shape, (-TILT_COS, TILT_SIN), 120.0);
            let p = shape.size_x() as f64;
            for (x, z) in [(0.0, 0.0), (12_345.5, 777.25), (p - 0.5, p - 3.0)] {
                let a = w.phase(shape, x, z);
                let b = w.phase(shape, x - p, z + p);
                let d = (a - b).abs();
                assert!(!(1e-4..=1.0 - 1e-4).contains(&d), "{size}: {a} vs {b}");
            }
            let k = ((w.mx / p).powi(2) + (w.mz / p).powi(2)).sqrt();
            assert!((1.0 / k - 120.0).abs() < 1.0, "{size}: spacing {}", 1.0 / k);
        }
    }

    #[test]
    fn transverse_ridges_are_gentle_upwind_and_steep_downwind() {
        let spacing = 220.0;
        let ridge = HEIGHT_PER_SPACING * spacing;
        let lee = ridge / (LEE_SLOPE * spacing);
        let windward = ridge / (WINDWARD_SHARE * spacing);
        assert!(windward < 0.3, "windward slope {windward}");
        assert_eq!(transverse(0.0, lee), 0.0);
        assert_eq!(transverse(WINDWARD_SHARE, lee), 1.0);
        assert_eq!(transverse(0.99, lee), 0.0, "an interdune flat");
        assert!(ridge >= 6.0 && HEIGHT_PER_SPACING * SPACINGS[1] as f32 <= 30.0);
    }
}
