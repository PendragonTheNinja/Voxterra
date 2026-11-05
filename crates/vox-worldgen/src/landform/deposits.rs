//! End moraines and drumlins (M15 task 3, `docs/landforms.md` *Ice*,
//! ADR-0021): what the last ice age's ice left on the lowlands it covered.
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
//! would wriggle like a worm. Both are height layers that only raise, on
//! low-relief ground, before the drainage network (ADR-0021 §4) — a
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
    shape: WorldShape,
    along: Wave,
    across: Wave,
    segment: LandformNoise,
    size: LandformNoise,
    swarm: LandformNoise,
    wander: [LandformNoise; 2],
}

impl Deposits {
    pub fn new(shape: WorldShape) -> Self {
        let n = |w, salt| LandformNoise::new(shape, w, salt);
        Self {
            shape,
            along: Wave::new(shape, (0.0, 1.0), DRUMLIN_ALONG),
            across: Wave::new(shape, (1.0, 0.0), DRUMLIN_ACROSS),
            segment: n(SEGMENT_WAVELENGTH, 0xF1_0001),
            size: n(SIZE_WAVELENGTH, 0xF1_0002),
            swarm: n(SWARM_WAVELENGTH, 0xF1_0003),
            wander: [n(WANDER.0, 0xF1_0004), n(WANDER.0, 0xF1_0005)],
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
        // An ellipse on each lattice cell: the product of two tents, the
        // along-flow one flattened so the hill is long.
        let a = tent(self.along.phase(self.shape, wx, wz));
        let b = tent(self.across.phase(self.shape, wx, wz));
        let hill = smoothstep(0.0, 1.0, a) * smoothstep(0.35, 1.0, b);
        let height = DRUMLIN_HEIGHT.0 + (DRUMLIN_HEIGHT.1 - DRUMLIN_HEIGHT.0) * size;
        let h = strength * height * hill;
        (h > 0.0).then_some((h, Deposit::Drumlins))
    }
}
