//! End moraines and drumlins (M15 task 3, `docs/landforms.md` *Ice*,
//! ADR-0026): what the last ice age's ice left on the lowlands it covered.
//!
//! - **End moraines** are ridges of rubble along the ice's farthest edge:
//!   here, along the contour where the ice-age weight crosses one half,
//!   5–20 blocks high, broken into segments, in the lobed arcs the margin
//!   makes as it follows the ground.
//! - **Drumlins** are streamlined hills the ice moulded behind its edge,
//!   in swarms aligned with its flow: here 5–15 high, ~250 long and ~90
//!   wide, behind the margin (weights 0.55–0.95), elongated along Z — on
//!   the lowlands an ice sheet flows outward from its dome toward the
//!   equator.
//!
//! The margin is read from the ice-age weight *without* the biomes'
//! boundary noise: that noise frays at 24 blocks, and a moraine along it
//! would wriggle like a worm. It is bent instead by a slow noise into
//! lobes a few kilometres across, as an ice sheet's edge pushed out along
//! the lowlands: over the tectonic terrain's flat lowlands the weight
//! follows latitude, and its contour ran dead straight for kilometres.
//! Drumlins stand in staggered strips, each hill its own size and some
//! missing: on a plain grid they read as rows of identical hills. Both are height layers that only raise, on
//! low-relief ground, before the drainage network (ADR-0026 §4) — a
//! moraine across a valley dams a lake, as on Earth. Pure functions of
//! `(x, z)`, tiled with the world.

use vox_core::WorldShape;

use super::dunes::{Wave, tent};
use crate::climate::smoothstep;
use crate::elevation::{Elevation, LandformNoise};

/// A moraine's half-width in the ice-age weight (the margin is where it
/// is 0.5), its height range (blocks) by a slow noise, and the share of
/// the margin it stands on (a noise above this).
const MORAINE_HALF: f32 = 0.06;
const MORAINE_HEIGHT: (f32, f32) = (4.0, 12.0);
const MORAINE_GAPS: f32 = 0.0;
/// Drumlins: the weights between which a field lies (behind the margin),
/// the spacing along and across the ice's flow (blocks), their height
/// range, and the share of that ground that is a swarm.
const DRUMLIN_WEIGHT: (f32, f32) = (0.55, 0.95);
const DRUMLIN_ALONG: f64 = 260.0;
const DRUMLIN_ACROSS: f64 = 110.0;
const DRUMLIN_HEIGHT: (f32, f32) = (5.0, 15.0);
const DRUMLIN_SWARM: f32 = 0.0;
/// Low ground only: relief and uplift (blocks) over which neither forms —
/// in mountains the ice was in valleys, cutting troughs.
const LOWLAND_RELIEF: (f32, f32) = (0.3, 0.45);
const LOWLAND_UPLIFT: (f32, f32) = (60.0, 150.0);
/// Noise wavelengths, blocks.
const SEGMENT_WAVELENGTH: i64 = 700;
const SIZE_WAVELENGTH: i64 = 2_500;
const SWARM_WAVELENGTH: i64 = 3_000;
const WANDER: (i64, f64) = (500, 45.0);
/// The margin's lobes: the ice weight shifted by up to this, by a noise of
/// this wavelength (blocks) and a second at a third of it. Over the flat
/// lowland where the margin ran straight the weight rises ~0.1 per 150
/// blocks, so lobes there bulge a few hundred blocks either way.
const LOBES: (f32, i64) = (0.3, 4_000);
/// Drumlins: the share of a swarm's cells that hold a hill, and the
/// smallest a hill is relative to the swarm's size.
const DRUMLIN_KEPT: f64 = 0.75;
const DRUMLIN_SMALLEST: f64 = 0.45;

/// What the ice left at a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deposit {
    Moraine,
    Drumlins,
}

impl Deposit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Moraine => "end moraine",
            Self::Drumlins => "drumlins",
        }
    }
}

/// The layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Deposits {
    seed: u64,
    shape: WorldShape,
    along: Wave,
    across: Wave,
    segment: LandformNoise,
    size: LandformNoise,
    swarm: LandformNoise,
    wander: [LandformNoise; 2],
    lobes: [LandformNoise; 2],
}

impl Deposits {
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        let n = |w, salt| LandformNoise::new(shape, w, salt);
        Self {
            seed,
            shape,
            along: Wave::new(shape, (0.0, 1.0), DRUMLIN_ALONG),
            across: Wave::new(shape, (1.0, 0.0), DRUMLIN_ACROSS),
            segment: n(SEGMENT_WAVELENGTH, 0xF1_0001),
            size: n(SIZE_WAVELENGTH, 0xF1_0002),
            swarm: n(SWARM_WAVELENGTH, 0xF1_0003),
            wander: [n(WANDER.0, 0xF1_0004), n(WANDER.0, 0xF1_0005)],
            lobes: [n(LOBES.1, 0xF1_0006), n(LOBES.1 / 3, 0xF1_0007)],
        }
    }

    /// Blocks the ice's deposits raise a column, and which, from its
    /// smooth ice-age weight `ice` and its base relief and uplift.
    pub fn at(
        &self,
        e: &Elevation,
        x: i64,
        z: i64,
        ice: f32,
        relief: f32,
        uplift: f32,
    ) -> Option<(f32, Deposit)> {
        let low = (1.0 - smoothstep(LOWLAND_RELIEF.0, LOWLAND_RELIEF.1, relief))
            * (1.0 - smoothstep(LOWLAND_UPLIFT.0, LOWLAND_UPLIFT.1, uplift));
        if low <= 0.0 {
            return None;
        }
        let size = 0.5 + 0.5 * e.noise(x, z, &self.size);
        let ice = ice
            + LOBES.0
                * (0.75 * e.noise(x, z, &self.lobes[0]) + 0.25 * e.noise(x, z, &self.lobes[1]));
        // The end moraine: a ridge across the margin, parabolic.
        let across = (ice - 0.5) / MORAINE_HALF;
        if across.abs() < 1.0 {
            let standing = smoothstep(
                MORAINE_GAPS,
                MORAINE_GAPS + 0.2,
                e.noise(x, z, &self.segment),
            );
            let height = MORAINE_HEIGHT.0 + (MORAINE_HEIGHT.1 - MORAINE_HEIGHT.0) * size;
            let h = low * standing * height * (1.0 - across * across);
            if h > 0.0 {
                return Some((h, Deposit::Moraine));
            }
        }
        // Drumlins behind it.
        let field = smoothstep(DRUMLIN_WEIGHT.0, DRUMLIN_WEIGHT.0 + 0.05, ice)
            * (1.0 - smoothstep(DRUMLIN_WEIGHT.1 - 0.05, DRUMLIN_WEIGHT.1, ice));
        let swarm = smoothstep(
            DRUMLIN_SWARM,
            DRUMLIN_SWARM + 0.2,
            e.noise(x, z, &self.swarm),
        );
        let strength = low * field * swarm;
        if strength <= 0.0 {
            return None;
        }
        let wx =
            self.shape.canonical_x(x) as f64 + WANDER.1 * f64::from(e.noise(x, z, &self.wander[0]));
        let wz =
            self.shape.canonical_z(z) as f64 + WANDER.1 * f64::from(e.noise(x, z, &self.wander[1]));
        // An ellipse in each cell: the product of two tents, the
        // along-flow one flattened so the hill is long. Each strip across
        // the flow is slid along it by its own amount, and each hill sized
        // (or left out) by its own draw — a tent is 0 at its cell's edges,
        // so neither shows as a seam.
        let strip = self.across.cycle(self.shape, wx, wz);
        let j = (strip.floor() as i64).rem_euclid(self.across.cycles());
        let slide = unit(mix(self.seed ^ 0xD8E1, j as u64));
        let along = self.along.cycle(self.shape, wx, wz) + slide;
        let i = (along.floor() as i64).rem_euclid(self.along.cycles());
        let draw = mix(self.seed ^ 0xD8E2, (j as u64) << 32 | i as u64);
        if unit(draw) >= DRUMLIN_KEPT {
            return None;
        }
        let scale = DRUMLIN_SMALLEST + (1.0 - DRUMLIN_SMALLEST) * unit(mix(draw, 1));
        let a = tent((along - along.floor()) as f32);
        let b = tent((strip - strip.floor()) as f32);
        let hill = smoothstep(0.0, 1.0, a) * smoothstep(0.35, 1.0, b);
        let height =
            (DRUMLIN_HEIGHT.0 + (DRUMLIN_HEIGHT.1 - DRUMLIN_HEIGHT.0) * size) * scale as f32;
        let h = strength * height * hill;
        (h > 0.0).then_some((h, Deposit::Drumlins))
    }
}

/// A 64-bit mix of a seed and a value (splitmix64's finaliser).
fn mix(seed: u64, v: u64) -> u64 {
    let mut h = seed ^ v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// A hash's top bits as a value in `[0, 1)`.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> (Elevation, Deposits) {
        let shape = WorldShape::DEFAULT;
        (
            Elevation::new(0x7e22ac0de, shape),
            Deposits::new(0x7e22ac0de, shape),
        )
    }

    /// An end moraine bends in lobes, as the ice's edge did: it is not the
    /// line of latitude the ice weight follows over flat lowland. Found on
    /// the tectonic terrain (version 18), where the lowlands are flat
    /// enough that the margin ran dead straight for kilometres.
    #[test]
    fn moraines_bend_in_lobes() {
        let (e, d) = layer();
        // A straight margin at z0: the ice weight rising 0.1 per 150
        // blocks poleward, as over the flat lowland where it was found.
        let z0 = 40_000i64;
        let ice = |z: i64| 0.5 + (z - z0) as f32 / 1_500.0;
        // Each line's ridge centre: the mean of its moraine columns.
        let mut crests = Vec::new();
        for x in (0..12_000i64).step_by(64) {
            let zs: Vec<i64> = (z0 - 1_500..z0 + 1_500)
                .step_by(4)
                .filter(|&z| {
                    d.at(&e, x, z, ice(z), 0.0, 0.0)
                        .is_some_and(|(_, k)| k == Deposit::Moraine)
                })
                .collect();
            if !zs.is_empty() {
                crests.push(zs.iter().sum::<i64>() / zs.len() as i64);
            }
        }
        assert!(
            crests.len() > 100,
            "moraine on {} of 188 lines",
            crests.len()
        );
        let lo = *crests.iter().min().unwrap();
        let hi = *crests.iter().max().unwrap();
        assert!(
            hi - lo >= 300,
            "the moraine's crest stays within {} blocks of a straight line over 12 km",
            hi - lo
        );
    }

    /// Drumlins stand staggered in a swarm, not on a grid: the hill one
    /// spacing across the flow is no echo of this one. (On the tectonic
    /// terrain's broad lowlands the grid read as rows of identical hills.)
    #[test]
    fn drumlins_are_not_a_grid() {
        let (e, d) = layer();
        let shape = WorldShape::DEFAULT;
        let across = shape.size_x() as f64 / (shape.size_x() as f64 / DRUMLIN_ACROSS).round();
        let lag = across.round() as i64;
        let h = |x: i64, z: i64| {
            d.at(&e, x, z, 0.75, 0.0, 0.0)
                .filter(|&(_, k)| k == Deposit::Drumlins)
                .map_or(0.0, |(h, _)| f64::from(h))
        };
        let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for z in (60_000..66_000i64).step_by(13) {
            for x in (10_000..16_000i64).step_by(17) {
                let (a, b) = (h(x, z), h(x + lag, z));
                if a == 0.0 && b == 0.0 {
                    continue;
                }
                n += 1.0;
                (sa, sb) = (sa + a, sb + b);
                (saa, sbb, sab) = (saa + a * a, sbb + b * b, sab + a * b);
            }
        }
        assert!(n > 10_000.0, "only {n} drumlin samples");
        let cov = sab / n - (sa / n) * (sb / n);
        let r = cov / ((saa / n - (sa / n).powi(2)) * (sbb / n - (sb / n).powi(2))).sqrt();
        assert!(
            r < 0.3,
            "drumlins one spacing apart correlate at {r:.2}: a grid"
        );
    }
}
