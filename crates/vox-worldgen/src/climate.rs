//! Climate: the first stage-1 system (M13 task 1, ADR-0017).
//!
//! CLAUDE.md's worldgen is two stages: stage 1 is a low-resolution simulation
//! of the whole planet at world creation; stage 2 derives each chunk from it.
//! Climate is the first thing that needs stage 1, because rainfall is not
//! local — a column is dry because of mountains hundreds of kilometres upwind,
//! which no single chunk can see.
//!
//! [`ClimateMap::build`] computes a coarse grid over the whole torus from the
//! elevation field, once per world, deterministically from the seed and size.
//! Each column then reads it ([`ClimateMap::sea_level_temperature`],
//! [`ClimateMap::precipitation`]) and applies altitude cooling from its own
//! exact height ([`temperature_at`]).
//!
//! ## The mechanisms (mirrored from Earth; dimensions chosen for play)
//!
//! 1. **Latitude** sets sea-level temperature, fitted to Earth's annual zonal
//!    means as a function of `sin(latitude)`.
//! 2. **Altitude** cools, at an exaggerated lapse rate ([`LAPSE_PER_BLOCK`]):
//!    our peaks are ~1/7 of Earth's, and at Earth's rate none would carry
//!    snow (ADR-0010's reasoning, applied to temperature).
//! 3. **Prevailing winds** blow along each row of the grid: easterly trades
//!    in the tropics, westerlies in mid-latitudes, polar easterlies, blended
//!    at the boundaries. Moisture is taken up over the sea, carried
//!    downwind, rained out over land — so interiors are drier than coasts.
//! 4. **Rising and sinking air**: the equatorial belt is wet, the subtropical
//!    highs dry — [`vertical_motion`] — which is where Earth's deserts are.
//! 5. **Rain shadows**: air climbing a range cools, cannot hold its moisture
//!    ([`moisture_capacity`] of the air at that height), and rains it out on
//!    the windward side; it reaches the lee already dry.
//!
//! ## Deterministic on every machine
//!
//! Only `+ − × ÷`, comparisons and `floor` — which IEEE 754 makes bit-exact
//! everywhere — as in the elevation field. `f32::exp` and friends call the
//! platform's maths library, which may differ in the last bit between
//! Windows and Linux, and a last-bit difference can flip a biome boundary;
//! [`exp`] here is built from basic operations instead. The parallel build
//! computes rows and cells independently and never reduces across threads.

use rayon::prelude::*;
use vox_core::WorldShape;

use crate::elevation::Elevation;

/// Temperature drop per block of altitude, °C. Earth's is 0.0065.
///
/// Tuned to the snowline targets in M13 criterion 4 with [`SNOW_LINE_C`]:
/// permanent snow above ~940 blocks at the equator, ~470 at 45°, ~110 at
/// 60°, sea level near the poles. Land's median height is ~75 blocks (2.4 °C
/// below the coast) and 90% of land is under ~200–330.
pub const LAPSE_PER_BLOCK: f32 = 0.032;

/// Mean annual temperature at or below which snow lies all year, °C.
pub const SNOW_LINE_C: f32 = -3.0;

/// Target size of a climate cell, in blocks; the grid has between
/// [`MIN_CELLS`] and [`MAX_CELLS`] cells per axis. Climate belts scale with
/// the world (ADR-0012), but ranges do not (ADR-0010) — a rain shadow needs
/// cells small enough to see a 7 000-block range, hence the cap only on
/// count.
const CELL_TARGET: i64 = 256;
const MIN_CELLS: i64 = 128;
const MAX_CELLS: i64 = 1_024;
/// Elevation samples per cell per axis.
const CELL_SAMPLES: i64 = 3;

/// Distance over land in which moisture falls by e (before recycling), in
/// blocks. Landform scale: continents are tens of kilometres wide at every
/// world size (ADR-0010), so this does not scale with the world. Earth's
/// rain reaches thousands of kilometres inland (the Amazon); our continents
/// are ~60× smaller, so ~30 km here — deserts come mostly from sinking air
/// ([`vertical_motion`]) and rain shadows, as Earth's do, not from distance.
const DRYING_LENGTH: f32 = 35_000.0;
/// Sideways mixing of moisture between neighbouring rows, per cell of
/// travel (0..0.5). Winds are not single lines: without it each peak dries a
/// one-cell streak all the way downwind.
const LATERAL_MIX: f32 = 0.25;
/// Mixing steps per cell of travel (each stable on its own).
const LATERAL_MIX_STEPS: usize = 2;
/// Fraction of rain over land returned to the air (evapotranspiration).
/// Rainforests recycle over half their rain, which is how the Amazon stays
/// wet thousands of kilometres from the Atlantic.
const RECYCLING: f32 = 0.6;
/// Distance over the sea in which air takes up moisture by e, in blocks.
const OCEAN_UPTAKE_LENGTH: f32 = 3_000.0;
/// Distance over which moisture above what the air can hold is stripped
/// out, in blocks: the width of a windward slope's rain.
const OROGRAPHIC_LENGTH: f32 = 1_500.0;
/// Windward rain: the multiplier on rain per unit of moisture above what the
/// air can hold, and its ceiling — windward slopes on Earth get ~2–5× the
/// lowland's.
const OROGRAPHIC_GAIN: f32 = 3.0;
const OROGRAPHIC_MAX: f32 = 5.0;
/// Upper bound on precipitation, mm/yr (Earth's wettest places: ~12 000).
const MAX_PRECIPITATION: f32 = 9_000.0;

/// Moisture over warm open sea, as the precipitation it would give
/// (mm/yr), at [`OCEAN_REFERENCE_C`]; it falls ~4.5% per °C colder — the
/// Clausius–Clapeyron 7% damped, because annual rain is not the
/// saturation vapour pressure (mid-latitude seas give ~800 mm, polar ~50).
const OCEAN_MOISTURE: f32 = 2_700.0;
const OCEAN_REFERENCE_C: f32 = 27.0;
const MOISTURE_PER_C: f32 = 0.045;

/// Continental interiors: colder than coasts at the same latitude, by up to
/// [`CONTINENTAL_COOLING_C`] at high latitudes, approached over
/// [`CONTINENTAL_LENGTH`] blocks of land upwind. Nothing at the equator.
const CONTINENTAL_COOLING_C: f32 = 9.0;
const CONTINENTAL_LENGTH: f32 = 12_000.0;

/// Sea-level mean annual temperature, °C, from `s = sin(latitude)`: a fit to
/// Earth's annual zonal means (27 °C at the equator, 19 at 30°, 0 at 60°,
/// −25 at the poles), monotonic in |s|.
pub fn sea_level_temperature_at_latitude(s: f32) -> f32 {
    let s2 = s * s;
    let s4 = s2 * s2;
    let s8 = s4 * s4;
    27.0 - 35.21 * s2 + 20.35 * s4 - 36.95 * s8
}

/// Mean annual temperature at a column of height `height` (blocks above sea
/// level; the sea surface and seabeds read sea level) whose sea-level
/// temperature is `sea_level_c`.
pub fn temperature_at(sea_level_c: f32, height: i64) -> f32 {
    sea_level_c - LAPSE_PER_BLOCK * height.max(0) as f32
}

/// How much moisture air at `temp_c` can carry, as the precipitation it
/// would give (mm/yr). Over the open sea air carries this much.
fn moisture_capacity(temp_c: f32) -> f32 {
    OCEAN_MOISTURE * exp(MOISTURE_PER_C * (temp_c.max(-45.0) - OCEAN_REFERENCE_C))
}

/// Rain from rising and sinking air, as a multiplier, by `|sin(latitude)|`:
/// the wet equatorial belt (the ITCZ), dry subtropical highs (~20–32°),
/// the wetter mid-latitude storm track, dry polar air.
fn vertical_motion(abs_s: f32) -> f32 {
    // (sin(latitude), multiplier) at 0, 8, 15, 22, 30, 38, 45, 55, 65, 75, 90°.
    const STOPS: [(f32, f32); 11] = [
        (0.000, 1.60),
        (0.139, 1.50),
        (0.259, 0.90),
        (0.375, 0.40),
        (0.500, 0.40),
        (0.616, 0.85),
        (0.707, 1.25),
        (0.819, 1.25),
        (0.906, 0.75),
        (0.966, 0.50),
        (1.000, 0.40),
    ];
    piecewise(&STOPS, abs_s)
}

/// Share of the wind blowing east (the westerlies, ~30–60°); the rest blows
/// west (trades and polar easterlies). Blended across ~±5° at the boundaries,
/// where Earth's winds are light and variable.
fn eastward_share(abs_s: f32) -> f32 {
    // sin 25° = 0.423, sin 35° = 0.574, sin 55° = 0.819, sin 65° = 0.906.
    smoothstep(0.423, 0.574, abs_s) * (1.0 - smoothstep(0.819, 0.906, abs_s))
}

/// Cooling of a continental interior after `fetch` blocks of land upwind.
fn continental_cooling(abs_s: f32, fetch: f32) -> f32 {
    CONTINENTAL_COOLING_C * smoothstep(0.3, 0.9, abs_s) * (1.0 - exp(-fetch / CONTINENTAL_LENGTH))
}

/// The planet's climate on a coarse grid (stage 1).
pub struct ClimateMap {
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
    /// Per cell, row-major (`j * nx + i`): sea-level temperature (°C, with
    /// continental cooling), annual precipitation (mm/yr), mean elevation
    /// (blocks) and share of land.
    sea_temp: Vec<f32>,
    precip: Vec<f32>,
    elev: Vec<f32>,
    land: Vec<f32>,
}

/// The inputs a sweep reads.
struct Grid<'a> {
    nx: usize,
    nz: usize,
    cell: f32,
    abs_s: &'a [f32],
    elev: &'a [f32],
    land: &'a [f32],
}

impl ClimateMap {
    /// Build the map for one world. Deterministic in `(elevation, shape)`;
    /// parallel over rows.
    pub fn build(elevation: &Elevation, shape: WorldShape) -> Self {
        let cells = |size: i64| (size / CELL_TARGET).clamp(MIN_CELLS, MAX_CELLS) as usize;
        let (nx, nz) = (cells(shape.size_x()), cells(shape.size_z()));
        let cell_x = shape.size_x() as f64 / nx as f64;
        let cell_z = shape.size_z() as f64 / nz as f64;

        // Elevation and land share per cell, from a small grid of samples.
        let samples: Vec<(f32, f32)> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (i, j) = (c % nx, c / nx);
                let (mut sum, mut land) = (0.0f64, 0u32);
                for b in 0..CELL_SAMPLES {
                    for a in 0..CELL_SAMPLES {
                        let fa = (a as f64 + 0.5) / CELL_SAMPLES as f64;
                        let fb = (b as f64 + 0.5) / CELL_SAMPLES as f64;
                        let x = ((i as f64 + fa) * cell_x).floor() as i64;
                        let z = ((j as f64 + fb) * cell_z).floor() as i64;
                        let h = elevation.height(x, z);
                        sum += h as f64;
                        land += u32::from(h > 0);
                    }
                }
                let n = (CELL_SAMPLES * CELL_SAMPLES) as f32;
                ((sum / n as f64) as f32, land as f32 / n)
            })
            .collect();
        let elev: Vec<f32> = samples.iter().map(|s| s.0).collect();
        let land: Vec<f32> = samples.iter().map(|s| s.1).collect();

        let abs_s: Vec<f32> = (0..nz)
            .map(|j| {
                let z = ((j as f64 + 0.5) * cell_z).floor() as i64;
                (shape.latitude_sine(z) as f32).abs()
            })
            .collect();
        let grid = Grid {
            nx,
            nz,
            cell: cell_x as f32,
            abs_s: &abs_s,
            elev: &elev,
            land: &land,
        };
        let (east, west) = rayon::join(|| sweep(&grid, true), || sweep(&grid, false));
        let mut sea_temp = Vec::with_capacity(nx * nz);
        let mut precip = Vec::with_capacity(nx * nz);
        for (j, &a) in abs_s.iter().enumerate() {
            let w = eastward_share(a);
            let t0 = sea_level_temperature_at_latitude(a);
            for i in 0..nx {
                let c = j * nx + i;
                let fetch = west.1[c] + (east.1[c] - west.1[c]) * w;
                sea_temp.push(t0 - continental_cooling(a, fetch));
                precip.push(west.0[c] + (east.0[c] - west.0[c]) * w);
            }
        }
        // Winds are not single lines: mix neighbouring rows and cells a little.
        blur(&mut precip, nx, nz);
        blur(&mut sea_temp, nx, nz);
        Self {
            nx,
            nz,
            cell_x,
            cell_z,
            sea_temp,
            precip,
            elev,
            land,
        }
    }

    /// Cells per axis.
    pub fn cells(&self) -> (usize, usize) {
        (self.nx, self.nz)
    }

    /// Blocks per cell, per axis.
    pub fn cell_blocks(&self) -> (f64, f64) {
        (self.cell_x, self.cell_z)
    }

    /// Raw per-cell values (for maps and tests): sea-level temperature,
    /// precipitation, mean elevation, land share.
    pub fn cell(&self, i: usize, j: usize) -> (f32, f32, f32, f32) {
        let c = j * self.nx + i;
        (self.sea_temp[c], self.precip[c], self.elev[c], self.land[c])
    }

    /// Sea-level mean annual temperature (°C) at a column, which may be on
    /// any lap of the world.
    pub fn sea_level_temperature(&self, x: i64, z: i64) -> f32 {
        self.bilinear(&self.sea_temp, x, z)
    }

    /// Annual precipitation (mm/yr) at a column.
    pub fn precipitation(&self, x: i64, z: i64) -> f32 {
        self.bilinear(&self.precip, x, z)
    }

    /// Bilinear between cell centres, wrapping with the torus. This is
    /// content addressing — the grid tiles the world as the noise does
    /// (CLAUDE.md: terrain generation is a canonicalising site).
    fn bilinear(&self, field: &[f32], x: i64, z: i64) -> f32 {
        let u = x as f64 / self.cell_x - 0.5;
        let v = z as f64 / self.cell_z - 0.5;
        let (fu, fv) = (u.floor(), v.floor());
        let (tu, tv) = ((u - fu) as f32, (v - fv) as f32);
        let i0 = (fu as i64).rem_euclid(self.nx as i64) as usize;
        let j0 = (fv as i64).rem_euclid(self.nz as i64) as usize;
        let (i1, j1) = ((i0 + 1) % self.nx, (j0 + 1) % self.nz);
        let at = |i: usize, j: usize| field[j * self.nx + i];
        let top = at(i0, j0) + (at(i1, j0) - at(i0, j0)) * tu;
        let bottom = at(i0, j1) + (at(i1, j1) - at(i0, j1)) * tu;
        top + (bottom - top) * tv
    }
}

/// Carry moisture along every row at once in one wind direction, twice
/// round the torus so the start does not matter, mixing it sideways between
/// rows at each step. Returns per cell the precipitation (mm/yr) and the
/// fetch — blocks of land upwind since the last sea.
fn sweep(g: &Grid, east: bool) -> (Vec<f32>, Vec<f32>) {
    let (nx, nz, cell) = (g.nx, g.nz, g.cell);
    // Per-row constants.
    let t0: Vec<f32> = g
        .abs_s
        .iter()
        .map(|&a| sea_level_temperature_at_latitude(a))
        .collect();
    let motion: Vec<f32> = g.abs_s.iter().map(|&a| vertical_motion(a)).collect();
    // Air loses moisture over land at no less than the baseline rate even
    // where sinking air suppresses rain: subtropical air crossing a
    // continent arrives dry on its far side (the Sahara is downwind of the
    // trade-wind coast, not just under the subtropical high).
    let drying: Vec<f32> = motion
        .iter()
        .map(|&m| 1.0 - exp(-cell * m.max(1.0) * (1.0 - RECYCLING) / DRYING_LENGTH))
        .collect();
    let ocean_uptake = 1.0 - exp(-cell / OCEAN_UPTAKE_LENGTH);
    let orographic = 1.0 - exp(-cell / OROGRAPHIC_LENGTH);

    let mut q: Vec<f32> = t0.iter().map(|&t| moisture_capacity(t)).collect();
    let mut fetch = vec![0.0f32; nz];
    let mut mixed = vec![0.0f32; nz];
    let (mut precip, mut fetches) = (vec![0.0; nx * nz], vec![0.0; nx * nz]);
    for lap in 0..2 {
        for k in 0..nx {
            let i = if east { k } else { nx - 1 - k };
            for j in 0..nz {
                let c = j * nx + i;
                let a = g.abs_s[j];
                let sea = g.land[c] < 0.5;
                if sea {
                    fetch[j] = 0.0;
                } else {
                    fetch[j] += cell;
                }
                let sea_level = t0[j] - continental_cooling(a, fetch[j]);
                if sea {
                    q[j] += (moisture_capacity(sea_level) - q[j]) * ocean_uptake;
                }
                // Air lifted over high ground cools and cannot hold its
                // moisture: it rains harder on the windward side, and what it
                // cannot hold is stripped out, so it reaches the lee dry.
                // Judged by the latitude's temperature and the altitude
                // alone: continental cooling is winter cold, and most rain
                // falls in the warm season — counted, it rained out flat
                // interiors.
                let cap = moisture_capacity(temperature_at(t0[j], g.elev[c].max(0.0) as i64));
                let windward =
                    (1.0 + OROGRAPHIC_GAIN * (q[j] / cap - 1.0).max(0.0)).min(OROGRAPHIC_MAX);
                let rain = q[j] * motion[j] * windward;
                q[j] -= q[j] * drying[j] * windward;
                if q[j] > cap {
                    q[j] -= (q[j] - cap) * orographic;
                }
                if lap == 1 {
                    precip[c] = rain.min(MAX_PRECIPITATION);
                    fetches[c] = fetch[j];
                }
            }
            for _ in 0..LATERAL_MIX_STEPS {
                for j in 0..nz {
                    let (up, down) = (q[(j + nz - 1) % nz], q[(j + 1) % nz]);
                    mixed[j] = q[j] + LATERAL_MIX * (up + down - 2.0 * q[j]);
                }
                std::mem::swap(&mut q, &mut mixed);
            }
        }
    }
    (precip, fetches)
}

/// Three passes of a [1 4 6 4 1] blur on each axis, wrapping.
fn blur(field: &mut [f32], nx: usize, nz: usize) {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut tmp = vec![0.0f32; field.len()];
    for _ in 0..3 {
        for j in 0..nz {
            for i in 0..nx {
                tmp[j * nx + i] = (0..5)
                    .map(|k| K[k] * field[j * nx + (i + nx + k - 2) % nx])
                    .sum();
            }
        }
        for j in 0..nz {
            for i in 0..nx {
                field[j * nx + i] = (0..5)
                    .map(|k| K[k] * tmp[((j + nz + k - 2) % nz) * nx + i])
                    .sum();
            }
        }
    }
}

/// `e^x`, from basic arithmetic only, so it is bit-identical on every
/// machine (see the module docs). Relative error under 1e-6 on [−80, 80].
pub fn exp(x: f32) -> f32 {
    let x = x.clamp(-80.0, 80.0);
    const LOG2_E: f32 = std::f32::consts::LOG2_E;
    // ln 2 split so `n * LN_2_HI` is exact for every n used (Cody–Waite).
    const LN_2_HI: f32 = 0.693_145_75;
    const LN_2_LO: f32 = 1.428_606_8e-6;
    let n = (x * LOG2_E + 0.5).floor();
    let r = (x - n * LN_2_HI) - n * LN_2_LO; // |r| <= ln2 / 2
    // Taylor to r^6: error < 2e-7 for |r| <= 0.35.
    let p = 1.0
        + r * (1.0
            + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0 + r / 720.0)))));
    // Scale by 2^n in two steps, so neither factor leaves f32's normal range.
    let half = (n as i32) / 2;
    let pow2 = |k: i32| f32::from_bits(((k + 127) as u32) << 23);
    p * pow2(half) * pow2(n as i32 - half)
}

fn smoothstep(lo: f32, hi: f32, v: f32) -> f32 {
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear between `(x, y)` stops sorted by x, clamped at the ends.
fn piecewise(stops: &[(f32, f32)], x: f32) -> f32 {
    if x <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    stops[stops.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_matches_the_library_closely() {
        let mut x = -80.0f32;
        while x <= 80.0 {
            let (a, b) = (exp(x), x.exp());
            assert!(((a - b) / b).abs() < 2e-6, "exp({x}) = {a}, want {b}");
            x += 0.173;
        }
        assert_eq!(exp(0.0), 1.0);
    }

    #[test]
    fn sea_level_temperature_falls_from_equator_to_pole() {
        let mut last = f32::MAX;
        for k in 0..=100 {
            let t = sea_level_temperature_at_latitude(k as f32 / 100.0);
            assert!(t < last, "not falling at sin(lat) = {}", k as f32 / 100.0);
            last = t;
        }
        assert!((sea_level_temperature_at_latitude(0.0) - 27.0).abs() < 0.01);
        assert!(
            (sea_level_temperature_at_latitude(0.5) - 19.3).abs() < 0.5,
            "30°"
        );
        assert!(sea_level_temperature_at_latitude(0.866).abs() < 1.5, "60°");
        assert!(sea_level_temperature_at_latitude(1.0) < -20.0, "pole");
    }

    /// A small world, cheap enough to build several times in a dev build.
    fn small(seed: u64) -> (ClimateMap, WorldShape) {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        (
            ClimateMap::build(&Elevation::new(seed, shape), shape),
            shape,
        )
    }

    /// `sin(latitude)` at row `j`'s centre.
    fn row_sine(m: &ClimateMap, shape: WorldShape, j: usize) -> f32 {
        let z = ((j as f64 + 0.5) * m.cell_z).floor() as i64;
        shape.latitude_sine(z) as f32
    }

    /// Stage 1 is deterministic: the same world builds the same map, to the
    /// bit (parallel rows never reduce across threads).
    #[test]
    fn the_map_is_deterministic() {
        let (a, _) = small(7);
        let (b, _) = small(7);
        assert!(
            a.precip
                .iter()
                .zip(&b.precip)
                .all(|(x, y)| x.to_bits() == y.to_bits())
        );
        assert!(
            a.sea_temp
                .iter()
                .zip(&b.sea_temp)
                .all(|(x, y)| x.to_bits() == y.to_bits())
        );
    }

    /// The map tiles with the torus: the same column on any lap reads the
    /// same climate, including across the seam between the last cell and
    /// the first.
    #[test]
    fn the_map_wraps_with_the_world() {
        let (m, shape) = small(3);
        let (sx, sz) = (shape.size_x(), shape.size_z());
        for (x, z) in [(0, 0), (37, sz - 5), (sx - 3, 1_000), (sx / 2, sz / 3)] {
            for (dx, dz) in [(sx, 0), (0, sz), (-sx, -2 * sz)] {
                assert_eq!(m.precipitation(x, z), m.precipitation(x + dx, z + dz));
                assert_eq!(
                    m.sea_level_temperature(x, z),
                    m.sea_level_temperature(x + dx, z + dz)
                );
            }
        }
        // Continuous across the seam: neighbours either side differ little.
        for z in (0..sz).step_by(997) {
            let (a, b) = (m.precipitation(sx - 1, z), m.precipitation(sx, z));
            assert!(
                (a - b).abs() < 0.01 * a.max(b) + 1.0,
                "seam at z = {z}: {a} vs {b}"
            );
        }
    }

    /// Criterion 4's rain shadow: where a ridge rises across the wind,
    /// its windward slope is wetter than its lee, in both wind belts.
    #[test]
    fn mountains_cast_rain_shadows() {
        let (mut windward, mut lee, mut found) = (0.0f64, 0.0f64, 0);
        for seed in [1, 2, 3, 4, 5] {
            let (m, shape) = small(seed);
            let (nx, nz) = m.cells();
            for j in 0..nz {
                let w = eastward_share(row_sine(&m, shape, j).abs());
                let d: isize = if w > 0.9 {
                    1
                } else if w < 0.1 {
                    -1
                } else {
                    continue;
                };
                let at = |i: usize, k: isize| (i as isize + k * d).rem_euclid(nx as isize) as usize;
                for i in 0..nx {
                    let (_, _, ridge, land) = m.cell(i, j);
                    let (_, p_up, up, land_up) = m.cell(at(i, -1), j);
                    let (_, _, foot, _) = m.cell(at(i, -4), j);
                    let (_, p_lee, low, land_lee) = m.cell(at(i, 5), j);
                    let rising = ridge >= 350.0 && foot <= ridge - 250.0 && up > foot;
                    let falling = low <= ridge - 250.0;
                    if rising && falling && land > 0.9 && land_up > 0.9 && land_lee > 0.9 {
                        windward += f64::from(p_up);
                        lee += f64::from(p_lee);
                        found += 1;
                    }
                }
            }
        }
        assert!(found >= 20, "only {found} ridges across the wind");
        let ratio = windward / lee;
        assert!(
            ratio > 1.8,
            "windward / lee rain {ratio:.2} over {found} ridges"
        );
    }

    /// Criterion 4's belts: land rain peaks at the equator, dips in the
    /// subtropics (where Earth's deserts are), recovers in mid-latitudes and
    /// is least at the poles — averaged over several worlds.
    #[test]
    fn rain_follows_the_belts() {
        let mut bands = [(0.0f64, 0u32); 9];
        for seed in [1, 2, 3, 4] {
            let (m, shape) = small(seed);
            let (nx, nz) = m.cells();
            for j in 0..nz {
                let s = row_sine(&m, shape, j).abs();
                // Ten-degree bands by sine: sin 10°, 20°, … 80°.
                let b = [0.174, 0.342, 0.5, 0.643, 0.766, 0.866, 0.940, 0.985]
                    .iter()
                    .filter(|&&e| s >= e)
                    .count();
                for i in 0..nx {
                    let (_, p, _, land) = m.cell(i, j);
                    if land > 0.5 {
                        bands[b].0 += f64::from(p);
                        bands[b].1 += 1;
                    }
                }
            }
        }
        let mean: Vec<f64> = bands
            .iter()
            .map(|&(s, n)| s / f64::from(n.max(1)))
            .collect();
        let (equator, subtropics, mid, polar) = (mean[0], mean[2], mean[4], mean[8]);
        assert!(subtropics < 0.5 * equator, "{mean:.0?}");
        assert!(subtropics < mid, "{mean:.0?}");
        assert!(polar < subtropics && polar < 0.15 * equator, "{mean:.0?}");
    }

    #[test]
    fn interiors_are_colder_than_coasts_away_from_the_equator() {
        assert_eq!(continental_cooling(0.0, 50_000.0), 0.0);
        let (coast, inland) = (
            continental_cooling(0.8, 0.0),
            continental_cooling(0.8, 30_000.0),
        );
        assert_eq!(coast, 0.0);
        assert!(inland > 5.0 && inland <= CONTINENTAL_COOLING_C);
    }

    /// Criterion 4's snowline targets follow from the lapse rate at sea-level
    /// temperatures: ~940 at the equator, ~470 at 45°, ~110 at 60°.
    #[test]
    fn snowlines_fall_where_the_spec_puts_them() {
        let snowline =
            |s: f32| (sea_level_temperature_at_latitude(s) - SNOW_LINE_C) / LAPSE_PER_BLOCK;
        assert!(
            (900.0..1_000.0).contains(&snowline(0.0)),
            "{}",
            snowline(0.0)
        );
        assert!(
            (430.0..600.0).contains(&snowline(0.707)),
            "{}",
            snowline(0.707)
        );
        assert!(snowline(0.866) < 160.0, "{}", snowline(0.866));
        assert!(snowline(1.0) < 0.0);
        assert!(temperature_at(10.0, 500) < temperature_at(10.0, 100));
        assert_eq!(temperature_at(10.0, -300), 10.0, "seabeds read sea level");
    }
}
