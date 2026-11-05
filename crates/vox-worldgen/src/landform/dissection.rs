//! Rain-dissected hills (M15 task 4, `docs/landforms.md` *Rain*,
//! ADR-0021).
//!
//! Where rain is plentiful, many small streams cut the hills into
//! close-set valleys and soil creep rounds the ridges between — the
//! Appalachian Plateau, Borneo's uplands, Brazil's *mar de morros*. Here:
//! valleys 40–120 blocks apart, finer than the drainage network's cells,
//! closer as the rain rises; 10–40 deep; ridges convex on top and concave
//! at the foot.
//!
//! **The valleys are left, the ridges raised.** A layer that cut valleys
//! before the network would hand its 128-block cells a field of pits;
//! cut after, it would leave lakes standing over air. So this layer, like
//! the dunes, comes after the water and only raises dry ground: the
//! valley floors keep the ground the water was fitted to, and the ground
//! between them rises into rounded ridges. It fades out toward rivers so
//! they keep their valleys. The ridge crests are the zero lines of warped,
//! tiled noise — winding, branching, never a grid.

use vox_core::WorldShape;

use crate::climate::smoothstep;
use crate::elevation::{Elevation, LandformNoise};

/// The two valley spacings blended by rain (wavelengths of the noise,
/// blocks: its zero lines fall about half a wavelength apart).
const WAVELENGTHS: (i64, i64) = (220, 90);
/// Rain weight over which the finer spacing takes over.
const FINE_FROM: (f32, f32) = (0.4, 0.9);
/// Ridge height (blocks), the least and the most, and the noise's value
/// at which a ridge has fallen to the valley floor (its half-width in the
/// noise's units).
const DEPTH: (f32, f32) = (10.0, 40.0);
const SHOULDER: f32 = 0.35;
/// The warp of the column before the noise: wavelength and reach, blocks.
const WARP: (i64, f32) = (300, 60.0);
/// Relief over which hills are dissected (plains stay plains).
const HILLS: (f32, f32) = (0.08, 0.3);
/// The relief at or below which no hill is dissected.
pub const DISSECTED_FROM_RELIEF: f32 = HILLS.0;

/// The layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Dissection {
    /// Per spacing: two octaves (the second at 0.6 of the wavelength), and
    /// the warp of the column before them.
    coarse: [LandformNoise; 2],
    fine: [LandformNoise; 2],
    warp: [LandformNoise; 2],
    depth: LandformNoise,
}

impl Dissection {
    pub fn new(shape: WorldShape) -> Self {
        let n = |w, salt| LandformNoise::new(shape, w, salt);
        let pair = |w: i64, salt: u64| [n(w, salt), n(w * 3 / 5, salt + 0x10)];
        Self {
            coarse: pair(WAVELENGTHS.0, 0xD1_0001),
            fine: pair(WAVELENGTHS.1, 0xD1_0002),
            warp: [n(WARP.0, 0xD1_0004), n(WARP.0, 0xD1_0005)],
            depth: n(2_000, 0xD1_0003),
        }
    }

    /// Blocks the ridges raise a column, from the rain weight `rain` and
    /// the base relief.
    pub fn at(&self, e: &Elevation, x: i64, z: i64, rain: f32, relief: f32) -> f32 {
        let hills = smoothstep(HILLS.0, HILLS.1, relief);
        let strength = rain * hills;
        if strength <= 0.0 {
            return 0.0;
        }
        let fine = smoothstep(FINE_FROM.0, FINE_FROM.1, rain);
        // Warped, and two octaves at unrelated wavelengths: a single value
        // noise's zero lines ran straight along its lattice.
        let wx = x + (WARP.1 * e.noise(x, z, &self.warp[0])).round() as i64;
        let wz = z + (WARP.1 * e.noise(x, z, &self.warp[1])).round() as i64;
        let pair =
            |p: &[LandformNoise; 2]| 0.65 * e.noise(wx, wz, &p[0]) + 0.35 * e.noise(wx, wz, &p[1]);
        let n = pair(&self.coarse) * (1.0 - fine) + pair(&self.fine) * fine;
        // A ridge's crest on the zero line, falling to broad valley floors
        // at the ground's own level: convex at the top, concave at the foot.
        let t = 1.0 - smoothstep(0.0, SHOULDER, n.abs());
        let depth = DEPTH.0 + (DEPTH.1 - DEPTH.0) * (0.5 + 0.5 * e.noise(x, z, &self.depth));
        strength * depth * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ridges rise from valleys at the original ground: the layer only
    /// raises, reaches its depth, and leaves some ground at zero.
    #[test]
    fn ridges_rise_from_untouched_valleys() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(3, shape);
        let d = Dissection::new(shape);
        let hs: Vec<f32> = (0..4_000).map(|i| d.at(&e, i, 77, 1.0, 0.5)).collect();
        assert!(hs.iter().all(|&h| h >= 0.0));
        assert!(hs.iter().any(|&h| h > 9.0));
        assert!(
            hs.iter().filter(|&&h| h < 1.0).count() > 40,
            "valley floors"
        );
        assert_eq!(d.at(&e, 5, 5, 0.0, 0.5), 0.0);
    }

    /// Valley density rises with rain (M15 criterion 3): more ridge
    /// crests along the same line at full rain than at little.
    #[test]
    fn valleys_closer_with_more_rain() {
        let shape = WorldShape::DEFAULT;
        let e = Elevation::new(3, shape);
        let d = Dissection::new(shape);
        let crests = |rain: f32| {
            let hs: Vec<f32> = (0..20_000).map(|i| d.at(&e, i, 4_321, rain, 0.5)).collect();
            // Each rise through half the layer's height is one ridge
            // (local maxima counted the wiggles on a ridge's flank).
            let half = 0.5 * hs.iter().copied().fold(0.0, f32::max);
            hs.windows(2)
                .filter(|w| w[0] < half && w[1] >= half)
                .count()
        };
        let (dry, wet) = (crests(0.3), crests(1.0));
        assert!(
            wet as f32 > 1.5 * dry as f32,
            "{wet} crests at full rain, {dry} at little"
        );
    }
}
