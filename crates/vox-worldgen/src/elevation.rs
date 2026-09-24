//! Elevation: the shape of the world's surface (M10 task 4).
//!
//! A pure function of `(seed, x, z)` — no state, no caching, no dependence on
//! chunk boundaries or generation order. That is not a stylistic preference:
//! the LOD sampler queries this at 2 km out without generating a single chunk,
//! and if the two paths ever disagree about a column's height you get M09's
//! ghost blocks at continental scale.
//!
//! ## Stages
//!
//! Composed in one place, [`Elevation::height`], from separately named stages:
//!
//! 1. [`Elevation::continent`] — where land is, at all. Lowest frequency.
//! 2. [`Elevation::relief`] — how *rough* a region is, independently of how
//!    high it is. The stage that produces plains.
//! 3. [`Elevation::orogeny`] — where mountain building happens, as LINES
//!    rather than blobs.
//! 4. [`Elevation::detail`] — local texture, scaled by relief.
//!
//! Erosion and hydrology (a later milestone) slot in as another stage over
//! this one's output. That is why the composition lives in one function
//! instead of being smeared through `surface_height`.
//!
//! ## Height is earned by extent
//!
//! The world is compressed ~100× horizontally (200 000 blocks pole to pole
//! against Earth's 20 000 km) but NOT vertically — one block is one metre. Take
//! Earth's landform proportions literally under that compression and the
//! Himalaya becomes a 5:1 wall.
//!
//! So peak height is a function of the *extent* of the uplift producing it:
//! [`Elevation::orogeny`] is low-frequency, and its value is cubed before
//! scaling. A summit near the world ceiling requires that field to sit near its
//! maximum across a region tens of thousands of blocks wide, which is rare by
//! construction rather than by a rarity roll. Small ranges stay small
//! automatically, and a larger world would grow larger mountains with no
//! constant changed.
//!
//! ## Most of the world is dull, deliberately
//!
//! [`Elevation::relief`] is raised to a power so the majority of the map sits
//! near zero roughness. Fractal noise left alone is rough *everywhere*, which
//! gives medium hills wall to wall and no true plains — and then mountains mean
//! nothing, because there is nothing flat to contrast them against. Earth is
//! mostly abyssal plain, shelf, steppe and lowland.

use vox_core::{WORLD_Y_MAX_BLOCKS, WORLD_Y_MIN_BLOCKS, WorldShape};

// --- Continental shape -----------------------------------------------------

/// Wavelengths (blocks) of the continent field. The coarse octave sets
/// continent size against a 200 000-block world; the fine one breaks up
/// coastlines so they are not ellipses.
const CONTINENT_WAVELENGTHS: [i64; 3] = [56_000, 21_000, 7_500];
const CONTINENT_WEIGHTS: [f32; 3] = [1.0, 0.30, 0.09];

/// Continent value at which land begins and at which it is fully inland. The
/// gap is the coastal blend.
/// Share of the world that is land, for every seed and every world size.
///
/// A design parameter, not an outcome. The continent field is a handful of
/// random lattice values, so left alone the land fraction is an accident of the
/// seed: measured across 200 seeds at the default size it ran from 15% to 91%,
/// with 30% of seeds over 60% land. Each world therefore calibrates its own
/// coastline at construction to hit this figure. It is also exactly the knob a
/// future "more land / more ocean" world-creation option would turn.
const TARGET_LAND_FRACTION: f32 = 0.42;

/// Continent-field samples per axis used to calibrate the coastline. Coarse on
/// purpose: the continent field is the lowest-frequency stage, and this runs
/// once per world, at construction.
const CALIBRATION_GRID: i64 = 96;

const COAST_LO: f32 = -0.10;
const COAST_HI: f32 = 0.05;

/// Detail is damped within this many blocks of sea level, to this fraction of
/// its normal amplitude.
///
/// Coastal plains are flat on Earth, and here the flatness is load-bearing:
/// full-strength detail at the shoreline makes the ground oscillate across sea
/// level, shattering every coast into a speckle of one-minute ponds and inlets.
/// The median water crossing was 500 blocks — a player in and out of a boat
/// constantly, with no real oceans to justify it.
const COASTAL_CALM_BLOCKS: f32 = 60.0;
const COASTAL_CALM_FLOOR: f32 = 0.22;

/// Gentle continental rise from coast to interior, in blocks. Lowlands, not
/// mountains — those come from orogeny.
const LOWLAND_RISE: f32 = 70.0;

/// Continental shelf depth, and the fraction of the ocean-ward blend it
/// occupies before the continental slope drops away.
const SHELF_DEPTH: f32 = 20.0;
const SHELF_FRAC: f32 = 0.34;
const SLOPE_FRAC: f32 = 0.30;

/// Abyssal plain depth in blocks.
const ABYSSAL_DEPTH: f32 = 260.0;

// --- Trenches --------------------------------------------------------------

const TRENCH_WAVELENGTH: i64 = 6_000;
/// Ridge value above which a trench forms. High, because trenches are rare.
const TRENCH_THRESHOLD: f32 = 0.94;
/// A second, independent low-frequency gate. A threshold on the ridge alone
/// cannot make trenches rare AND wide — the band where a ridge exceeds a
/// threshold is a fixed fraction of the map, so rarity has to be bought by
/// narrowing it into a crack. Gating on a separate field instead restricts
/// trenches to a few stretches of the line, leaving those stretches full width.
const TRENCH_GATE_WAVELENGTH: i64 = 12_000;
const TRENCH_GATE_LO: f32 = 0.56;
const TRENCH_GATE_HI: f32 = 0.80;
/// Extra depth below the abyssal plain, in blocks.
const TRENCH_EXTRA: f32 = 340.0;

// --- Relief ----------------------------------------------------------------

const RELIEF_WAVELENGTHS: [i64; 2] = [2_600, 900];
const RELIEF_WEIGHTS: [f32; 2] = [1.0, 0.5];

// --- Orogeny ---------------------------------------------------------------

const OROGENY_WAVELENGTHS: [i64; 2] = [7_000, 2_600];
const OROGENY_WEIGHTS: [f32; 2] = [1.0, 0.25];
/// Orogeny below this produces no uplift at all. Mountain building is a
/// threshold process — most continental crust is not being shortened — and
/// without the floor, every gentle rise in the field becomes a hill.
///
/// Kept LOW on purpose. Raising it does make mountains rarer, but it buys that
/// rarity by narrowing them: only a thin strip near each ridge crest clears the
/// bar, so the whole rise is crammed into a few thousand blocks and the flanks
/// come out at 2:1. Rarity has to be bought somewhere that does not cost
/// extent — see [`OROGENY_GATE_WAVELENGTH`].
const OROGENY_FLOOR: f32 = 0.42;

/// An independent low-frequency gate on mountain building.
///
/// This is where rarity is bought. Orogeny is a belt phenomenon — crust is
/// being shortened along a few plate boundaries and nowhere else — so gating on
/// a separate wide field restricts ranges to a handful of belts while leaving
/// each belt its full width. Rare AND broad, which a threshold on the ridge
/// field alone cannot give you.
const OROGENY_GATE_WAVELENGTH: i64 = 13_000;
const OROGENY_GATE_LO: f32 = 0.10;
const OROGENY_GATE_HI: f32 = 0.52;
/// Uplift at full orogeny, in blocks. The ceiling a summit can reach, and only
/// where the field saturates across a very wide region.
const MAX_UPLIFT: f32 = 1_150.0;

// --- Detail ----------------------------------------------------------------

const DETAIL_WAVELENGTHS: [i64; 3] = [800, 280, 90];
/// Weights fall roughly in proportion to wavelength, NOT on a fixed
/// persistence. Amplitude divided by wavelength is what the eye reads as slope,
/// so octaves that keep a constant ratio while the wavelength shrinks 3-4x per
/// step put cliff-grade gradients into the finest octave.
const DETAIL_WEIGHTS: [f32; 3] = [1.0, 0.28, 0.07];
/// Detail amplitude in blocks: a floor everywhere, plus relief-driven and
/// massif-driven terms. The floor keeps dead-flat ground from looking synthetic.
const DETAIL_FLOOR: f32 = 11.0;
const DETAIL_PER_RELIEF: f32 = 230.0;
const DETAIL_PER_MASSIF: f32 = 0.13;
/// Detail is suppressed underwater — abyssal plains are among the flattest
/// surfaces on Earth.
const DETAIL_OCEAN_SCALE: f32 = 0.42;

/// Margin kept between generated terrain and the hard world bounds, so nothing
/// generates into the last chunk layer.
const BOUND_MARGIN: i64 = 64;

/// One noise octave, tiled to the world's period.
///
/// ## Why octaves are per-world now (ADR-0012)
///
/// The world is a torus, so terrain must repeat exactly every world width:
/// generation is one of the three places the seam exists. Noise repeats only if
/// a whole number of lattice cells fits around the world, so each octave rounds
/// its target wavelength to the nearest size that does
/// ([`WorldShape::cells_around_x`]) — a few percent at the default size, and
/// correct at any legal size. The wavelength constants above are therefore
/// TARGETS, not exact cell sizes.
#[derive(Clone, Copy, Debug)]
struct Octave {
    /// Lattice cells around the world along each axis.
    nx: u32,
    nz: u32,
    /// Cells per block, `n / size`, in 32.32 fixed point. See [`lattice`].
    scale_x: u64,
    scale_z: u64,
    /// Hash salt: the TARGET wavelength, not the rounded one, so octaves stay
    /// decorrelated from one another and each hashes identically at every
    /// world size.
    salt: u64,
}

impl Octave {
    fn new(shape: WorldShape, target_wavelength: i64) -> Self {
        let nx = shape.cells_around_x(target_wavelength);
        let nz = shape.cells_around_z(target_wavelength);
        Self {
            nx,
            nz,
            scale_x: fixed_scale(nx, shape.size_x()),
            scale_z: fixed_scale(nz, shape.size_z()),
            salt: target_wavelength as u64,
        }
    }
}

/// The elevation field for one world: its seed and its size.
#[derive(Clone, Copy, Debug)]
pub struct Elevation {
    seed: u64,
    shape: WorldShape,
    /// Subtracted from the continent field so this world's land fraction lands
    /// on [`TARGET_LAND_FRACTION`]. See [`Elevation::calibrate_coastline`].
    continent_offset: f32,
    continent: [Octave; 3],
    relief: [Octave; 2],
    orogeny: [Octave; 2],
    orogeny_gate: Octave,
    trench: Octave,
    trench_gate: Octave,
    detail: [Octave; 3],
}

impl Elevation {
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        let oct = |w: i64| Octave::new(shape, w);
        let mut e = Self {
            seed,
            shape,
            continent_offset: 0.0,
            continent: CONTINENT_WAVELENGTHS.map(oct),
            relief: RELIEF_WAVELENGTHS.map(oct),
            orogeny: OROGENY_WAVELENGTHS.map(oct),
            orogeny_gate: oct(OROGENY_GATE_WAVELENGTH),
            trench: oct(TRENCH_WAVELENGTH),
            trench_gate: oct(TRENCH_GATE_WAVELENGTH),
            detail: DETAIL_WAVELENGTHS.map(oct),
        };
        e.continent_offset = e.calibrate_coastline();
        e
    }

    /// The continent-field shift that makes [`TARGET_LAND_FRACTION`] of this
    /// world land.
    ///
    /// Samples the raw continent field on a coarse grid covering exactly one
    /// lap of the world, and finds the value that the target share of samples
    /// exceed. The coastline sits where [`coastal_profile`] crosses sea level —
    /// a fixed continent value, found once by bisection — so the offset moves
    /// the field until that crossing falls at the right quantile.
    ///
    /// Deterministic from the seed and size alone, so the chunk generator and
    /// the LOD sampler, which share this struct, always agree.
    fn calibrate_coastline(&self) -> f32 {
        let (sx, sz) = (self.shape.size_x(), self.shape.size_z());
        let mut samples = Vec::with_capacity((CALIBRATION_GRID * CALIBRATION_GRID) as usize);
        for j in 0..CALIBRATION_GRID {
            for i in 0..CALIBRATION_GRID {
                samples
                    .push(self.continent_c(i * sx / CALIBRATION_GRID, j * sz / CALIBRATION_GRID));
            }
        }
        samples.sort_by(|a, b| a.total_cmp(b));
        let sea_share = 1.0 - TARGET_LAND_FRACTION;
        let at = ((samples.len() as f32 * sea_share) as usize).min(samples.len() - 1);
        samples[at] - shoreline_continent_value()
    }

    /// Surface height in blocks at world column `(x, z)`. Sea level is 0.
    ///
    /// `x`/`z` may be on any lap of the world; the result repeats exactly every
    /// world width. The column is wrapped ONCE here and every stage below works
    /// in canonical coordinates, rather than each octave wrapping for itself.
    pub fn height(&self, x: i64, z: i64) -> i64 {
        self.height_canonical(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    /// The single composition point. Every stage feeds in here and nowhere
    /// else, so a future erosion pass has one place to attach.
    fn height_canonical(&self, x: i64, z: i64) -> i64 {
        let c = self.continent_c(x, z) - self.continent_offset;
        let CoastalProfile {
            mut base,
            land,
            deep,
        } = coastal_profile(c);

        // Trenches: rare, linear, and only in genuinely deep water. Gated on
        // `deep` so one cannot appear off a beach.
        if deep > 0.0 {
            let t = self.ridge(x, z, &self.trench);
            let line = smoothstep01((t - TRENCH_THRESHOLD) / (1.0 - TRENCH_THRESHOLD));
            if line > 0.0 {
                let g = self.value_noise(x, z, &self.trench_gate);
                let gate = smoothstep01((g - TRENCH_GATE_LO) / (TRENCH_GATE_HI - TRENCH_GATE_LO));
                base -= line * gate * deep * TRENCH_EXTRA;
            }
        }

        // Mountains. Cubing is the earned-by-extent rule: `orogeny` is
        // low-frequency, so a high value implies a wide region, and cubing makes
        // the payoff for width steep.
        let massif = if land > 0.0 {
            let raw = self.orogeny_c(x, z);
            let g = self.value_noise(x, z, &self.orogeny_gate);
            let belt = smoothstep01((g - OROGENY_GATE_LO) / (OROGENY_GATE_HI - OROGENY_GATE_LO));
            let u = ((raw - OROGENY_FLOOR) / (1.0 - OROGENY_FLOOR)).clamp(0.0, 1.0) * land * belt;
            // Squared smoothstep, not a raw cube. Both make high uplift rare,
            // but a cube's slope keeps CLIMBING to the summit, so the crest
            // ends up the steepest part of the mountain — and ridge noise puts
            // a crease there already. Smoothstep flattens to zero slope at both
            // ends: gentle foothills, a rounded crest, the steep ground in
            // between where a mountain's steep ground belongs.
            let shaped = smoothstep(u);
            shaped * shaped * MAX_UPLIFT
        } else {
            0.0
        };

        // Roughness is its own field, not a function of height — that is what
        // lets a high plateau be flat and a low region be broken.
        let relief = self.relief_c(x, z);
        let calm = smoothstep01(base.abs() / COASTAL_CALM_BLOCKS);
        let amplitude = (DETAIL_FLOOR + relief * DETAIL_PER_RELIEF + massif * DETAIL_PER_MASSIF)
            * lerp(DETAIL_OCEAN_SCALE, 1.0, land)
            * lerp(COASTAL_CALM_FLOOR, 1.0, calm);

        let h = base + massif + self.detail_c(x, z) * amplitude;
        (h.round() as i64).clamp(
            WORLD_Y_MIN_BLOCKS + BOUND_MARGIN,
            WORLD_Y_MAX_BLOCKS - BOUND_MARGIN,
        )
    }

    // --- Stages. Each has a public form taking any (x, z) — for tuning and
    // --- debug views — and a `_c` form taking canonical coordinates, which is
    // --- what the composition calls so the column is wrapped only once.

    /// Stage 1 — continentalness in roughly `[-1, 1]`. Above [`COAST_HI`] is
    /// solidly inland; below [`COAST_LO`] is sea.
    pub fn continent(&self, x: i64, z: i64) -> f32 {
        self.continent_c(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    fn continent_c(&self, x: i64, z: i64) -> f32 {
        self.fbm(x, z, &self.continent, &CONTINENT_WEIGHTS)
    }

    /// Stage 2 — how rough this region is, in `[0, 1]`, independent of height.
    ///
    /// Biased hard toward 0 by squaring: without it, fractal noise makes
    /// everywhere equally lumpy and the world has no plains.
    pub fn relief(&self, x: i64, z: i64) -> f32 {
        self.relief_c(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    fn relief_c(&self, x: i64, z: i64) -> f32 {
        let t = (self.fbm(x, z, &self.relief, &RELIEF_WEIGHTS) * 0.5 + 0.5).clamp(0.0, 1.0);
        // Squared, by multiplication rather than `powf`. The bias itself is the
        // point — it is what gives the world plains instead of uniform
        // lumpiness — but `powf` is a transcendental call in the hottest
        // function in worldgen, where every nanosecond is paid a million times
        // a second by chunk generation and the LOD sampler together.
        t * t
    }

    /// Stage 3 — mountain building, in `[0, 1]`, concentrated along LINES.
    ///
    /// Ridge noise (`1 - |n|`) rather than plain fbm: real ranges are long and
    /// narrow because they follow plate boundaries, and plain fbm gives
    /// isolated round lumps that read as noise rather than geology.
    pub fn orogeny(&self, x: i64, z: i64) -> f32 {
        self.orogeny_c(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    fn orogeny_c(&self, x: i64, z: i64) -> f32 {
        let mut sum = 0.0;
        let mut norm = 0.0;
        for (oct, w) in self.orogeny.iter().zip(OROGENY_WEIGHTS.iter()) {
            sum += self.ridge(x, z, oct) * w;
            norm += w;
        }
        (sum / norm).clamp(0.0, 1.0)
    }

    /// Stage 4 — local texture in `[-1, 1]`. Amplitude is applied by the
    /// caller, from relief, so this is shape only.
    pub fn detail(&self, x: i64, z: i64) -> f32 {
        self.detail_c(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    fn detail_c(&self, x: i64, z: i64) -> f32 {
        self.fbm(x, z, &self.detail, &DETAIL_WEIGHTS)
    }

    /// Weighted sum of value-noise octaves, normalized to roughly `[-1, 1]`.
    /// Canonical coordinates.
    fn fbm(&self, x: i64, z: i64, octaves: &[Octave], weights: &[f32]) -> f32 {
        let mut sum = 0.0;
        let mut norm = 0.0;
        for (oct, w) in octaves.iter().zip(weights.iter()) {
            sum += self.value_noise(x, z, oct) * w;
            norm += w;
        }
        sum / norm
    }

    /// Ridge noise in `[0, 1]`: peaks along the zero crossings of value noise,
    /// which form connected lines rather than isolated blobs. Canonical
    /// coordinates.
    fn ridge(&self, x: i64, z: i64, oct: &Octave) -> f32 {
        1.0 - self.value_noise(x, z, oct).abs()
    }

    /// Value noise in `[-1, 1]` for one octave, at CANONICAL coordinates. Hash
    /// the four surrounding lattice corners and smoothstep-interpolate, so
    /// adjacent columns agree and chunk borders line up exactly.
    ///
    /// The lattice wraps: the corner past the last cell is cell 0. That single
    /// rule is what makes terrain continuous across the world's seam.
    fn value_noise(&self, x: i64, z: i64, oct: &Octave) -> f32 {
        let (ix, fx) = lattice(x, oct.scale_x, oct.nx);
        let (iz, fz) = lattice(z, oct.scale_z, oct.nz);
        let ix1 = if ix + 1 == oct.nx { 0 } else { ix + 1 };
        let iz1 = if iz + 1 == oct.nz { 0 } else { iz + 1 };

        let c00 = self.lattice_value(ix, iz, oct.salt);
        let c10 = self.lattice_value(ix1, iz, oct.salt);
        let c01 = self.lattice_value(ix, iz1, oct.salt);
        let c11 = self.lattice_value(ix1, iz1, oct.salt);

        let sx = smoothstep(fx);
        let sz = smoothstep(fz);
        lerp(lerp(c00, c10, sx), lerp(c01, c11, sx), sz)
    }

    /// Deterministic hashed value in `[-1, 1]` for a lattice point.
    ///
    /// The octave's salt is mixed into the hash so two octaves whose lattices
    /// coincide at some point do not return the same value there, which would
    /// leave a visible grid of correlated spots.
    fn lattice_value(&self, lx: u32, lz: u32, salt: u64) -> f32 {
        let h = mix64(
            self.seed
                ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (lx as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ (lz as u64).wrapping_mul(0xA076_1D64_78BD_642F),
        );
        // Top 24 bits are plenty for a noise lattice, and dividing an integer
        // beats a u64 -> f64 conversion — this runs four times per octave.
        ((h >> 40) as f32) * (2.0 / 16_777_215.0) - 1.0
    }
}

/// The continental base elevation for a continent value, before trenches,
/// mountains and detail: rise on land; shelf, then slope, then abyssal plain at
/// sea. Continuous through the coast, so there is no step.
///
/// Factored out because the coastline calibration must use exactly the same
/// curve the terrain does — a copy would drift.
struct CoastalProfile {
    base: f32,
    /// 0 at sea, 1 fully inland.
    land: f32,
    /// 0 on the shelf, 1 over the abyssal plain.
    deep: f32,
}

fn coastal_profile(c: f32) -> CoastalProfile {
    let land = smoothstep01((c - COAST_LO) / (COAST_HI - COAST_LO));
    let sea = 1.0 - land;
    let shelf = smoothstep01(sea / SHELF_FRAC);
    let deep = smoothstep01((sea - SHELF_FRAC) / SLOPE_FRAC);
    CoastalProfile {
        base: land * LOWLAND_RISE - shelf * SHELF_DEPTH - deep * (ABYSSAL_DEPTH - SHELF_DEPTH),
        land,
        deep,
    }
}

/// The continent value at which the coastal profile crosses sea level: where
/// the shoreline falls, before detail. Found by bisection — the profile rises
/// monotonically with the continent value.
fn shoreline_continent_value() -> f32 {
    let (mut lo, mut hi) = (COAST_LO - 1.0, COAST_HI + 1.0);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if coastal_profile(mid).base < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// `n / period` in 32.32 fixed point: cells per block, scaled by 2^32.
fn fixed_scale(n: u32, period: i64) -> u64 {
    (((n as u128) << 32) / period as u128) as u64
}

/// A canonical coordinate's lattice cell and fractional position within it.
///
/// ## Why fixed point
///
/// This runs twice per octave per column — tens of millions of times a second
/// across chunk generation and the LOD sampler — and its cost is the cost of
/// the elevation field.
///
/// Before the torus, octave wavelengths were compile-time constants, and the
/// compiler turns division by a constant into a cheap multiply-and-shift. Tiled
/// octaves divide by a per-world value instead. Doing that in floating point
/// costs a chain of int→float→int conversions, and measured ~55% slower for the
/// whole field. In 32.32 fixed point it is one integer multiply: the high 32
/// bits are the cell, the low 32 the position within it.
///
/// `scale` truncates by under 2⁻³² cells per block, which shifts cell
/// boundaries by at most a few ten-thousandths of a cell across the largest
/// legal world — deterministic, continuous, and identical for the chunk
/// generator and the LOD sampler, which is what matters.
#[inline]
fn lattice(c: i64, scale: u64, n: u32) -> (u32, f32) {
    debug_assert!(c >= 0, "lattice() takes canonical coordinates");
    // c < 2^22 and scale <= 2^32, so the product fits comfortably in a u64.
    let u = c as u64 * scale;
    let cell = (u >> 32) as u32;
    // Top 24 bits of the fraction: exactly representable in an f32.
    let frac = (((u >> 8) & 0x00FF_FFFF) as i32) as f32 * (1.0 / 16_777_216.0);
    // Clamp guards the (theoretical) case of rounding up to exactly n.
    (cell.min(n - 1), frac)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smoothstep over a value already expressed as a 0..1 parameter, clamping
/// outside the range.
fn smoothstep01(t: f32) -> f32 {
    smoothstep(t.clamp(0.0, 1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// SplitMix64 finalizer. One round over a pre-combined key rather than one
/// round per input: the inputs are already decorrelated by odd-constant
/// multiplication before mixing, and this is the hottest function in worldgen.
/// Deterministic and platform-independent.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u64 = 0x0007_E22A_C0DE;

    /// Sample the whole world on a coarse, prime-ish lattice — a round step
    /// would alias against the field wavelengths and measure the wrong thing.
    fn survey() -> Vec<i64> {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        let half = WorldShape::DEFAULT.size_x() / 2;
        let step = 997usize;
        let mut hs = Vec::new();
        for z in (-half..half).step_by(step) {
            for x in (-half..half).step_by(step) {
                hs.push(e.height(x, z));
            }
        }
        hs
    }

    fn fraction(hs: &[i64], f: impl Fn(i64) -> bool) -> f64 {
        hs.iter().filter(|&&h| f(h)).count() as f64 / hs.len() as f64
    }

    /// **Terrain repeats exactly every world width** (ADR-0012).
    ///
    /// Generation is one of the three places the seam exists: the player's
    /// coordinates never wrap, so the same place is generated at a different
    /// unwrapped position on every lap, and it must come out identical.
    #[test]
    fn terrain_tiles_with_the_world() {
        let w = WorldShape::DEFAULT;
        let e = Elevation::new(SEED, w);
        let (px, pz) = (w.size_x(), w.size_z());
        for i in 0..400i64 {
            let (x, z) = (i * 7_919 % px, i * 104_729 % pz);
            let h = e.height(x, z);
            for (dx, dz) in [(px, 0), (0, pz), (-3 * px, 2 * pz), (5 * px, -7 * pz)] {
                assert_eq!(e.height(x + dx, z + dz), h, "({x}, {z}) differs a lap away");
            }
        }
    }

    /// **The seam is invisible.** Terrain must be as continuous across the
    /// world's wrap line as anywhere else.
    ///
    /// Tiling alone is not enough: noise can repeat exactly and still jump at
    /// the seam if the lattice's last cell does not interpolate into its first.
    /// That would show as a cliff running the full length of the world. The
    /// bound compares the seam against ordinary one-block steps elsewhere, so
    /// it holds at any tuning.
    #[test]
    fn terrain_is_continuous_across_the_seam() {
        let w = WorldShape::DEFAULT;
        let e = Elevation::new(SEED, w);
        let (px, pz) = (w.size_x(), w.size_z());
        let (mut seam, mut interior) = (0i64, 0i64);
        for i in 0..2_000i64 {
            let t = i * 97;
            // Across the X seam, and across the Z seam.
            seam = seam.max((e.height(px - 1, t) - e.height(px, t)).abs());
            seam = seam.max((e.height(t, pz - 1) - e.height(t, pz)).abs());
            // The same one-block step well away from any seam.
            interior = interior.max((e.height(px / 2, t) - e.height(px / 2 + 1, t)).abs());
            interior = interior.max((e.height(t, pz / 2) - e.height(t, pz / 2 + 1)).abs());
        }
        assert!(
            seam <= (interior * 2).max(8),
            "a {seam}-block step at the seam, against {interior} anywhere else"
        );
    }

    /// **Land fraction is a property of the world, not an accident of the seed.**
    ///
    /// Before calibration, 200 seeds at the default size ran from 15% to 91%
    /// land — the target of 42% had been tuned on a single seed. Every seed and
    /// every legal size must now land on target, and must tile and stay inside
    /// the world while doing it.
    #[test]
    fn every_world_hits_its_land_target() {
        for size in [
            vox_core::MIN_WORLD_SIZE_BLOCKS,
            10 * vox_core::WORLD_SIZE_QUANTUM_BLOCKS,
            vox_core::DEFAULT_WORLD_SIZE_BLOCKS,
        ] {
            let w = WorldShape::new(size, size).unwrap();
            for seed in 0..12u64 {
                let e = Elevation::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), w);
                let (mut land, n) = (0, 1_600i64);
                for i in 0..n {
                    let (x, z) = (i * 7_919 % size, i * 104_729 % size);
                    let h = e.height(x, z);
                    assert!(h > vox_core::WORLD_Y_MIN_BLOCKS && h < vox_core::WORLD_Y_MAX_BLOCKS);
                    assert_eq!(e.height(x + size, z - size), h);
                    if h > 0 {
                        land += 1;
                    }
                }
                let f = land as f32 / n as f32;
                assert!(
                    (f - TARGET_LAND_FRACTION).abs() < 0.05,
                    "size {size}, seed {seed}: land fraction {f:.2}, target {TARGET_LAND_FRACTION}"
                );
            }
        }
    }

    #[test]
    fn is_deterministic() {
        let a = Elevation::new(SEED, WorldShape::DEFAULT);
        let b = Elevation::new(SEED, WorldShape::DEFAULT);
        for (x, z) in [(0, 0), (1_234, -5_678), (-99_999, 99_999), (37, 41)] {
            assert_eq!(a.height(x, z), b.height(x, z));
        }
    }

    /// Terrain must never generate outside the world, including well past the
    /// horizontal bounds — the LOD sampler queries beyond the edge.
    #[test]
    fn never_generates_outside_the_world() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        for h in survey() {
            assert!(h > vox_core::WORLD_Y_MIN_BLOCKS && h < vox_core::WORLD_Y_MAX_BLOCKS);
        }
        for (x, z) in [
            (500_000, 0),
            (0, -500_000),
            (i32::MAX as i64, i32::MIN as i64),
        ] {
            let h = e.height(x, z);
            assert!(h > vox_core::WORLD_Y_MIN_BLOCKS && h < vox_core::WORLD_Y_MAX_BLOCKS);
        }
    }

    /// A world of all land or all ocean would pass most other tests here.
    #[test]
    fn there_is_both_land_and_ocean() {
        let hs = survey();
        let land = fraction(&hs, |h| h > 0);
        assert!(
            (0.10..0.55).contains(&land),
            "land fraction {land:.3} is not a plausible world (Earth is ~0.29)"
        );
    }

    /// The single largest band of the surface is abyssal plain, as on Earth.
    #[test]
    fn the_ocean_floor_dominates() {
        let hs = survey();
        let abyssal = fraction(&hs, |h| (h as f32) < -ABYSSAL_DEPTH * 0.5);
        assert!(
            abyssal > 0.4,
            "only {abyssal:.3} of the world is deep ocean floor"
        );
    }

    /// THE relief requirement. Fractal noise is rough everywhere by default,
    /// which gives medium hills wall to wall and no plains — and then mountains
    /// mean nothing, because nothing dull is left to contrast them against.
    #[test]
    fn most_land_is_flat() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        let half = WorldShape::DEFAULT.size_x() / 2;
        let mut flat = 0usize;
        let mut total = 0usize;
        for z in (-half..half).step_by(1_009) {
            for x in (-half..half).step_by(1_009) {
                let h = e.height(x, z);
                if h <= 0 {
                    continue; // land only; the ocean floor would flatter us
                }
                total += 1;
                if (e.height(x + 16, z) - h).abs() <= 2 {
                    flat += 1;
                }
            }
        }
        let f = flat as f64 / total as f64;
        assert!(
            f > 0.5,
            "only {f:.3} of land is near-flat; there are no plains"
        );
    }

    /// Mountains are exceptional, and the highest ground is exceptional even
    /// among mountains.
    #[test]
    fn high_ground_is_rare() {
        let hs = survey();
        let land: Vec<i64> = hs.iter().copied().filter(|&h| h > 0).collect();
        // Fractions of the uplift ceiling, not absolute block heights. The
        // world's vertical scale is a design choice that has already changed
        // once and may change again; what must hold at ANY scale is the shape
        // of the distribution — high ground rare, the highest rarer still.
        let above = |f: f32| {
            land.iter().filter(|&&h| h as f32 > MAX_UPLIFT * f).count() as f64 / land.len() as f64
        };
        assert!(
            above(0.25) < 0.25,
            "{:.3} of land is above 1/4 height",
            above(0.25)
        );
        assert!(
            above(0.50) < 0.12,
            "{:.3} of land is above 1/2 height",
            above(0.50)
        );
        assert!(
            above(0.80) < 0.05,
            "{:.3} of land is above 4/5 height",
            above(0.80)
        );
    }

    /// Trenches are rare and linear, not a general property of the sea floor.
    #[test]
    fn trenches_are_rare() {
        let hs = survey();
        let floor = -(ABYSSAL_DEPTH + TRENCH_EXTRA * 0.5);
        let deep = fraction(&hs, |h| (h as f32) < floor);
        assert!(deep > 0.0, "no trenches were generated at all");
        assert!(
            deep < 0.02,
            "{deep:.4} of the world is below {floor:.0}; trenches are not rare"
        );
    }

    /// **Height is earned by extent.** A summit cannot be a spike: the ground
    /// for tens of thousands of blocks around it must be elevated too, because
    /// the uplift driving it is low-frequency by construction.
    ///
    /// This is the property that keeps a 100x-compressed world from producing
    /// unclimbable walls, so it is asserted rather than assumed.
    #[test]
    fn a_summit_implies_a_massif_around_it() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        let half = WorldShape::DEFAULT.size_x() / 2;
        let mut best = (i64::MIN, 0, 0);
        for z in (-half..half).step_by(1_009) {
            for x in (-half..half).step_by(1_009) {
                let h = e.height(x, z);
                if h > best.0 {
                    best = (h, x, z);
                }
            }
        }
        let (peak, px, pz) = best;
        assert!(
            peak as f32 > MAX_UPLIFT * 0.5,
            "no serious summit found to test (max {peak})"
        );
        // Ring radius is a QUARTER OF THE OROGENY WAVELENGTH, not a fixed
        // distance. That wavelength is what sets how wide a massif can be, so a
        // fixed radius asks a different question every time the field is
        // rescaled — and did exactly that once, failing a perfectly good
        // mountain for not being wider than any real one.
        let r = OROGENY_WAVELENGTHS[0] / 4;
        let d = (r as f64 * std::f64::consts::FRAC_1_SQRT_2) as i64;
        let mut sum = 0i64;
        let mut n = 0i64;
        for (dx, dz) in [
            (r, 0),
            (-r, 0),
            (0, r),
            (0, -r),
            (d, d),
            (-d, -d),
            (d, -d),
            (-d, d),
        ] {
            sum += e.height(px + dx, pz + dz);
            n += 1;
        }
        let ring = sum / n;
        assert!(
            ring > peak / 8,
            "a {peak}-block summit sits above ground averaging {ring} {r} blocks out — \
             it is a spike, not a massif"
        );
    }

    /// No discontinuities: the coast, the continental slope and the trench
    /// walls are the steepest features here, and none may become a sheer step.
    #[test]
    fn there_are_no_cliff_discontinuities() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        let half = WorldShape::DEFAULT.size_x() / 2;
        let mut worst = 0i64;
        for z in (-half..half).step_by(1_009) {
            for x in (-half..half).step_by(1_009) {
                let d = (e.height(x + 16, z) - e.height(x, z)).abs();
                worst = worst.max(d);
            }
        }
        assert!(
            worst < 600,
            "a {worst}-block step over 16 blocks — something is discontinuous"
        );
    }

    /// **Travelling must keep producing new terrain.**
    ///
    /// The property the first tuning pass got wrong, and the reason it had to
    /// be redone: heights were dramatic (peaks near 8 000) but every wavelength
    /// was tens of kilometres, so a 200 km world held only a handful of
    /// features and a player flew for minutes across near-constant ground. A
    /// world can be both spectacular and boring, and no other test here would
    /// have caught it.
    ///
    /// Measures what a player experiences: the elevation SPREAD across a 10 km
    /// walk, at the median land start. Spread rather than endpoint difference,
    /// because a walk that climbs a ridge and comes back down has encountered
    /// terrain even though it ends where it began.
    ///
    /// The bound discriminates: at the wavelengths this replaced, the median
    /// 10 km walk spanned 83 blocks; it now spans 134.
    #[test]
    fn terrain_changes_as_you_travel() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        let half = WorldShape::DEFAULT.size_x() / 2;
        const SPAN: i64 = 10_000;
        let mut spreads = Vec::new();
        for z in (-half..half).step_by(4_001) {
            for x in (-half..half).step_by(4_001) {
                if e.height(x, z) <= 0 {
                    continue; // land only — the abyssal plain is flat by design
                }
                let (mut lo, mut hi) = (i64::MAX, i64::MIN);
                for k in 0..21 {
                    let h = e.height(x + k * SPAN / 20, z);
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
                spreads.push(hi - lo);
            }
        }
        spreads.sort_unstable();
        let median = spreads[spreads.len() / 2];
        let bound = (MAX_UPLIFT * 0.05) as i64;
        assert!(
            median > bound,
            "the median 10 km walk spans only {median} blocks (need > {bound}) — \
             the world is monotonous"
        );
    }
}
