//! Benches, mesas and buttes (M15 task 2b, `docs/landforms.md` *Flash
//! flood*, ADR-0021).
//!
//! Where flash floods work dry uplands, a hard caprock resists and the
//! soft beds under it are stripped: the ground steps down in **benches**
//! (flat tops, near-vertical risers), and where gullies have cut back far
//! enough the caprock is left standing as **mesas** — flat tops a few
//! hundred blocks across behind walls 30–120 high — and, at the last
//! stage, narrow **buttes**. Rubble from the caprock piles at every wall's
//! foot as a **talus** apron at ~34°, up to half the wall's height.
//!
//! Two height operations, both pure functions of `(x, z)`, the column's
//! flash-flood weight and its base height and slope:
//!
//! 1. **Dissection.** A mask (two octaves of tiled noise, ~700 and ~230
//!    blocks) splits the upland into caprock that stands `depth` above
//!    the stripped ground, and the stripped ground, which keeps the base
//!    height. Its threshold wanders, so some uplands are thick with mesas
//!    and others a plain with a few buttes; caprock never covers more than
//!    about a third, so it stands in islands. The caprock is raised, not
//!    the floor lowered: stripping floors down dug bowls that the network
//!    filled as closed basins (+2–6 points of land drained to them — M15
//!    criterion 6 asks for fewer), while hills on ground that drained
//!    leave it draining. The mask's edge is the wall; a band below it the
//!    talus. Where a slower noise is high the wall is
//!    widened to a walkable ramp: about half the mesas have a way up
//!    (owner's decision 5).
//! 2. **Benches.** The result is terraced in steps 18–60 high (whole
//!    multiples of the badlands' 3-block beds, so a riser's colours read as
//!    beds). The riser is a few blocks wide whatever the ground's slope —
//!    the slope sets how wide each bench is — so risers are cliffs on any
//!    plateau, and benches are wide where the plateau is gentle.
//!
//! Applied before the drainage network (ADR-0021 §4): the network routes
//! round the mesas and along the stripped ground. Canyons cut by its
//! rivers, and walls banded below the top 12 blocks, come later.

use vox_core::WorldShape;

use crate::climate::smoothstep;
use crate::elevation::{Elevation, LandformNoise};

/// The dissection mask's two octaves, blocks, and the finer one's share.
const MASK_WAVELENGTHS: (i64, i64) = (700, 230);
const MASK_FINE: f32 = 0.3;
/// Where the stripped ground begins, as the mask's threshold: between
/// these, by a slow noise. Never under the first, so at most about a third
/// of the upland stands as caprock and the caprock stands in islands — the
/// stripped ground between them connects, and drains.
const THRESHOLD: (f32, f32) = (0.25, 0.6);
const THRESHOLD_WAVELENGTH: i64 = 6_000;
/// How far the caprock stands above the stripped ground at full weight,
/// blocks: walls 30–120 high, by a slow noise.
const DEPTH: (f32, f32) = (30.0, 120.0);
const DEPTH_WAVELENGTH: i64 = 4_000;
/// Share of a wall's height that is talus at its foot.
const TALUS_SHARE: f32 = 0.4;
/// The mask's width, in its own units, of the talus band and of the wall
/// (sheer, and where there is a way up).
const TALUS_BAND: f32 = 0.11;
const WALL_BAND: (f32, f32) = (0.012, 0.09);
/// Where a way up is: the ramp noise above these (a smoothstep).
const RAMP: (f32, f32) = (0.15, 0.45);
const RAMP_WAVELENGTH: i64 = 450;
/// Benches: step height range (blocks, rounded to whole beds), its noise,
/// and a riser's width (blocks). Weight from which benches are full.
const STEP: (f32, f32) = (18.0, 60.0);
const STEP_WAVELENGTH: i64 = 5_000;
const BED: f32 = 3.0;
const RISER_BLOCKS: f32 = 5.0;
const BENCH_WEIGHT: (f32, f32) = (0.3, 0.7);
/// Weight from which the dissection is full.
const DISSECT_WEIGHT: (f32, f32) = (0.1, 0.6);

/// The layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Mesas {
    mask: [LandformNoise; 2],
    threshold: LandformNoise,
    depth: LandformNoise,
    ramp: LandformNoise,
    step: LandformNoise,
}

/// Where on the layer a column stands (the readout's name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MesaPart {
    /// On caprock: a mesa's or butte's top, or a bench.
    Top,
    /// On a wall or the talus below it.
    Talus,
    /// On the stripped ground between.
    Floor,
}

impl MesaPart {
    pub fn name(self) -> &'static str {
        match self {
            Self::Top => "mesa top",
            Self::Talus => "talus slope",
            Self::Floor => "mesa country",
        }
    }
}

impl Mesas {
    pub fn new(shape: WorldShape) -> Self {
        let n = |w, salt| LandformNoise::new(shape, w, salt);
        Self {
            mask: [
                n(MASK_WAVELENGTHS.0, 0xE5_0001),
                n(MASK_WAVELENGTHS.1, 0xE5_0002),
            ],
            threshold: n(THRESHOLD_WAVELENGTH, 0xE5_0003),
            depth: n(DEPTH_WAVELENGTH, 0xE5_0004),
            ramp: n(RAMP_WAVELENGTH, 0xE5_0005),
            step: n(STEP_WAVELENGTH, 0xE5_0006),
        }
    }

    /// The column's new height, from its flash-flood weight `flood`, its
    /// base height and its base slope (blocks per block), and where on the
    /// layer it stands. `slope` is asked only where benches are cut.
    /// `None` where the layer does nothing.
    pub fn at(
        &self,
        e: &Elevation,
        x: i64,
        z: i64,
        flood: f32,
        height: f32,
        slope: impl FnOnce() -> f32,
    ) -> Option<(f32, MesaPart)> {
        if flood <= 0.0 {
            return None;
        }
        // 1. Dissection.
        let dissect = smoothstep(DISSECT_WEIGHT.0, DISSECT_WEIGHT.1, flood);
        let mask = (1.0 - MASK_FINE) * e.noise(x, z, &self.mask[0])
            + MASK_FINE * e.noise(x, z, &self.mask[1]);
        let u = |n: &LandformNoise| 0.5 + 0.5 * e.noise(x, z, n);
        let threshold = THRESHOLD.0 + (THRESHOLD.1 - THRESHOLD.0) * u(&self.threshold);
        let depth = dissect * (DEPTH.0 + (DEPTH.1 - DEPTH.0) * u(&self.depth));
        let ramp = smoothstep(RAMP.0, RAMP.1, e.noise(x, z, &self.ramp));
        let wall = WALL_BAND.0 + (WALL_BAND.1 - WALL_BAND.0) * ramp;
        let foot = threshold - wall;
        let talus = smoothstep(foot - TALUS_BAND, foot, mask);
        let cliff = smoothstep(foot, threshold, mask);
        let standing = TALUS_SHARE * talus + (1.0 - TALUS_SHARE) * cliff;
        let mut h = height + depth * standing;
        let part = if standing >= 0.98 {
            MesaPart::Top
        } else if standing > 0.02 {
            MesaPart::Talus
        } else {
            MesaPart::Floor
        };

        // 2. Benches: flat steps with risers RISER_BLOCKS wide.
        let bench = smoothstep(BENCH_WEIGHT.0, BENCH_WEIGHT.1, flood);
        if bench > 0.0 {
            let raw = STEP.0 + (STEP.1 - STEP.0) * u(&self.step);
            let step = (raw / BED).round() * BED;
            let level = (h / step).floor() * step;
            let f = (h - level) / step;
            // The riser's share of a step, from how far the ground runs to
            // climb one step.
            let riser = (RISER_BLOCKS * slope().max(1e-3) / step).clamp(0.02, 0.6);
            let terraced = level + step * smoothstep(1.0 - riser, 1.0, f);
            h += (terraced - h) * bench;
        }
        Some((h, part))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Benches are flat and their risers steep: along a uniform slope at
    /// full weight, most columns sit on a flat and a few climb fast.
    #[test]
    fn benches_are_flat_with_steep_risers() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(5, shape);
        let m = Mesas::new(shape);
        let slope = 0.1;
        let (mut flat, mut steep, mut n) = (0u32, 0u32, 0u32);
        let mut last = None;
        for x in 0..4_000i64 {
            let base = 200.0 + slope * x as f32;
            let (h, _) = m.at(&e, x, 0, 1.0, base, || slope).unwrap();
            if let Some(prev) = last {
                let d: f32 = h - prev;
                n += 1;
                flat += u32::from(d.abs() < 0.05);
                steep += u32::from(d.abs() > 1.0);
            }
            last = Some(h);
        }
        // On a 0.1 slope: some flat (benches, and the stripped floor), and
        // risers and walls climbing more than a block a block.
        assert!(flat * 2 > n, "{flat} flat of {n}");
        assert!(steep > 10, "only {steep} steep steps");
    }

    #[test]
    fn nothing_without_flash_floods() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(5, shape);
        assert_eq!(Mesas::new(shape).at(&e, 10, 10, 0.0, 300.0, || 0.1), None);
    }
}
