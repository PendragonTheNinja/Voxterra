//! Benches, mesas and buttes (M15 task 2b, `docs/landforms.md` *Flash
//! flood*, ADR-0026).
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
//! 2. **Benches.** The plateau under the mesas is terraced in steps 18–60
//!    high. A riser is half as wide as it is high whatever the ground's
//!    slope — a slope of soft beds under a cliff band, the slope setting
//!    how wide each bench is — and its edge wanders with a fine noise.
//!    (Risers 5 blocks wide, steps rounded to whole beds, and the mesas
//!    terraced with their plateau made walls one sheer drop, top to floor,
//!    and benches left standing cylinders: the owner's first look at
//!    version 22.)
//!
//! Applied before the drainage network (ADR-0026 §4): the network routes
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
/// A wall's profile, Monument Valley's (M15 task 6, after the owner's first
/// look at version 22: "straight up, no real shape"): a sheer caprock cliff
/// over a steep slope of the soft beds under it, over a talus apron. Each
/// one's share of the wall's height, and its width in the mask's own units
/// (~350 blocks a unit): the cliff's sheer, or a walkable ramp where there
/// is a way up; the slope ~1.5 a block; the talus ~0.7.
const CLIFF_SHARE: f32 = 0.4;
const SLOPE_SHARE: f32 = 0.35;
const TALUS_SHARE: f32 = 0.25;
const WALL_BAND: (f32, f32) = (0.012, 0.09);
const SLOPE_BAND: f32 = 0.05;
const TALUS_BAND: f32 = 0.09;
/// A ragged outline: a fine noise shifting the mask by up to this much
/// (~8 blocks), so walls have buttresses and alcoves — the two coarse
/// octaves alone drew smooth curves, buttes as cylinders.
const RAGGED: (f32, i64) = (0.025, 48);
/// Where a way up is: the ramp noise above these (a smoothstep).
const RAMP: (f32, f32) = (0.15, 0.45);
const RAMP_WAVELENGTH: i64 = 450;
/// Benches: step height range (blocks, rounded to whole beds), its noise,
/// and a riser's width (blocks). Weight from which benches are full.
const STEP: (f32, f32) = (18.0, 60.0);
/// Slow: a level is a multiple of the step, so where the step changes the
/// levels slide sideways, and faster than the plateau climbs they had
/// squeezed risers into sheer drops.
const STEP_WAVELENGTH: i64 = 20_000;
/// A riser's width across, as a share of its height, and how far its edge
/// wanders (blocks of height, through the ground's contours) and at what
/// wavelength — slow, against the plateau's slope (a wobble that changed
/// height faster than the ground squeezed risers into sheer drops).
const RISER_PER_STEP: f32 = 0.5;
const WOBBLE: (f32, i64) = (6.0, 200);
const BENCH_WEIGHT: (f32, f32) = (0.3, 0.7);
/// Weight from which the dissection is full.
const DISSECT_WEIGHT: (f32, f32) = (0.1, 0.6);

/// The layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Mesas {
    mask: [LandformNoise; 2],
    ragged: LandformNoise,
    wobble: LandformNoise,
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
            ragged: n(RAGGED.1, 0xE5_0007),
            wobble: n(WOBBLE.1, 0xE5_0008),
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
        // Ragged only near an edge (the fine noise is not asked elsewhere).
        let reach = wall + SLOPE_BAND + TALUS_BAND + RAGGED.0;
        let mask = if (mask - threshold).abs() < reach {
            mask + RAGGED.0 * e.noise(x, z, &self.ragged)
        } else {
            mask
        };
        let foot = threshold - wall;
        let slope_foot = foot - SLOPE_BAND;
        let talus = smoothstep(slope_foot - TALUS_BAND, slope_foot, mask);
        let steep = ((mask - slope_foot) / SLOPE_BAND).clamp(0.0, 1.0);
        let cliff = smoothstep(foot, threshold, mask);
        let standing = TALUS_SHARE * talus + SLOPE_SHARE * steep + CLIFF_SHARE * cliff;
        let part = if standing >= 0.98 {
            MesaPart::Top
        } else if standing > 0.02 {
            MesaPart::Talus
        } else {
            MesaPart::Floor
        };

        // 2. Benches: the plateau the mesas stand on, in flat steps with
        // risers RISER_BLOCKS wide. The plateau only: terraced after the
        // mesas were raised, a mesa's talus and wall had been cut into
        // benches too and its risers set by the plateau's gentle slope —
        // one sheer drop, top to floor.
        let mut ground = height;
        let bench = smoothstep(BENCH_WEIGHT.0, BENCH_WEIGHT.1, flood);
        if bench > 0.0 {
            // Not rounded to whole beds: where the step's noise crossed
            // from one bed count to the next, every level jumped and the
            // ground dropped a whole step in one column.
            let step = STEP.0 + (STEP.1 - STEP.0) * u(&self.step);
            // Ragged edges: the terraces follow the ground's contours, so
            // they wander with it a few blocks.
            let wobble = WOBBLE.0 * e.noise(x, z, &self.wobble);
            let level = ((ground + wobble) / step).floor() * step;
            let f = (ground + wobble - level) / step;
            // The riser's share of a step: RISER_PER_STEP of the step's
            // height across, whatever the ground's slope.
            let riser = (RISER_PER_STEP * slope().max(1e-3)).clamp(0.02, 0.6);
            let g = ((f - (1.0 - riser)) / riser).clamp(0.0, 1.0);
            let terraced = level + step * riser_profile(g);
            ground += (terraced - ground) * bench;
        }
        Some((ground + depth * standing, part))
    }
}

/// A riser's rise across it, `g` in 0..1 to 0..1: a slope of the soft
/// beds (55% of the rise over 70% of the way), then a cliff band of the
/// hard bed at its top — the profile of a mesa's wall, in small.
fn riser_profile(g: f32) -> f32 {
    if g < 0.7 {
        0.55 * g / 0.7
    } else {
        0.55 + 0.45 * (g - 0.7) / 0.3
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

    /// A mesa's wall is a sheer caprock cliff over a steep slope and a
    /// talus apron — Monument Valley's profile — not one sheer drop to the
    /// floor. (The benches had terraced the whole layer, wall and talus
    /// into one riser: "straight up, no real shape", the owner's first
    /// look at version 22.) Across every mesa edge on a gentle plateau at
    /// full weight, no single column carries more than 60% of the wall,
    /// and the wall has a slope at its foot.
    #[test]
    fn mesa_walls_stand_on_slopes() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(5, shape);
        let m = Mesas::new(shape);
        let slope = 0.02;
        let (mut edges, mut sheer) = (0u32, Vec::new());
        for z in (0..20_000i64).step_by(97) {
            let profile: Vec<(f32, MesaPart)> = (0..3_000i64)
                .map(|x| {
                    m.at(&e, x, z, 1.0, 200.0 + slope * x as f32, || slope)
                        .unwrap()
                })
                .collect();
            // Each run from a top down to the floor (either way along x).
            let mut x = 1;
            while x < profile.len() {
                let (top, floor) = (MesaPart::Top, MesaPart::Floor);
                if profile[x - 1].1 == top && profile[x].1 != top {
                    let end = (x..profile.len()).find(|&k| profile[k].1 == floor);
                    if let Some(end) = end {
                        let drop = profile[x - 1].0 - profile[end].0;
                        if drop > 20.0 {
                            edges += 1;
                            let steepest = (x..=end)
                                .map(|k| profile[k - 1].0 - profile[k].0)
                                .fold(0.0f32, f32::max);
                            if steepest > 0.6 * drop {
                                sheer.push((x, z, drop, steepest));
                            }
                        }
                        x = end;
                    }
                }
                x += 1;
            }
        }
        assert!(edges > 30, "only {edges} mesa edges");
        assert!(
            sheer.len() * 10 < edges as usize,
            "{} of {edges} mesa edges one sheer drop, e.g. {:?}",
            sheer.len(),
            &sheer[..sheer.len().min(4)]
        );
    }

    /// A bench's riser is a cliff band over a slope, not one sheer drop:
    /// with 18–60-block steps and risers 5 blocks wide, a whole step stood
    /// as one sheer wall, and a terrace left standing on a lowered floor
    /// as a cylinder (the owner's first look at version 22). Up a plateau
    /// of 0.1–0.2 at full weight, no column climbs more than a third of
    /// its step.
    #[test]
    fn bench_risers_stand_on_slopes() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(5, shape);
        let m = Mesas::new(shape);
        let (mut risers, mut sheer) = (0u32, 0u32);
        for (k, slope) in [0.1f32, 0.15, 0.2].iter().enumerate() {
            let mut last: Option<f32> = None;
            for x in 0..60_000i64 {
                let z = 9_000 + 211 * k as i64;
                // Up and down between 100 and a few hundred blocks, as
                // plateaus stand.
                let run = 4_000.0;
                let t = (x as f32 / run).fract();
                let climb = if t < 0.5 { t } else { 1.0 - t } * run;
                // The plateau alone: a column of the stripped floor.
                let (h, part) = m
                    .at(&e, x, z, 1.0, 100.0 + slope * climb, || *slope)
                    .unwrap();
                if part != MesaPart::Floor {
                    last = None;
                    continue;
                }
                if let Some(prev) = last {
                    let d = (h - prev).abs();
                    if d > 2.0 {
                        risers += 1;
                        sheer += u32::from(d > 15.0);
                    }
                }
                last = Some(h);
            }
        }
        assert!(risers > 20, "only {risers} riser steps");
        assert_eq!(
            sheer, 0,
            "{sheer} of {risers} riser steps climb more than 15 at once"
        );
    }

    #[test]
    fn nothing_without_flash_floods() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(5, shape);
        assert_eq!(Mesas::new(shape).at(&e, 10, 10, 0.0, 300.0, || 0.1), None);
    }
}
