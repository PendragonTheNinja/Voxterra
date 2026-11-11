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
//! ## Where things are is the tectonic framework's (milestone 16, ADR-0025)
//!
//! Until generator version 9 the continents, the mountain belts and the
//! trenches were each placed by a noise field of their own, and nothing
//! was where it was for a reason. Now [`crate::tectonics`] places them:
//! land is continental crust, ranges stand along collisions, arcs and old
//! welds, a trench lies off every subduction zone, and the sea floor
//! rises to a ridge where plates part. Every *shape* here — the coastal
//! profile, the ridged ranges and their heights, relief, detail — is as
//! ADR-0010 tuned it; what changed is the question "where?".
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

use std::sync::Arc;

use vox_core::{WORLD_Y_MAX_BLOCKS, WORLD_Y_MIN_BLOCKS, WorldShape};

use crate::tectonics::{
    BOUNDARY_REACH, Boundary, COAST_REACH, Convergence, PLATE_BLOCKS, Tectonics,
};

// --- Continental shape -----------------------------------------------------

/// Blocks of distance from the edge of continental crust per unit of the
/// continent field, on the land side and on the sea side. The first is
/// the slope the field had at a coast when it was noise (measured on
/// version 8), so the rise from the shore to the lowlands is as wide as
/// it was. The second is twice that: the noise field ran nearly level
/// along many coasts and gave them broad shallows, and a field that is a
/// distance cannot, so the shelf and slope are stretched instead — as
/// far as they can be while the sea floor still reaches the abyss within
/// the framework's reach.
const COAST_BLOCKS_PER_UNIT: (f32, f32) = (23_000.0, 46_000.0);
/// Wavelengths (blocks) of the noise that breaks the crust's edge into a
/// coast: bays and headlands, and islands and inlets within a walk of the
/// shore. The tectonic framework gives the outline at ten kilometres and
/// up; these give it below that, so that a coast is not a polygon.
const COAST_WAVELENGTHS: [i64; 3] = [6_100, 2_300, 850];
const COAST_WEIGHTS: [f32; 3] = [1.0, 0.8, 0.5];
/// How far that noise can move the field, in its own units — about 2 300
/// blocks of coastline either way at the very most, a third of that
/// typically.
const COAST_NOISE: f32 = 0.10;
/// The share of it left far from any coast. The framework's distance
/// stops counting 3 500 blocks from the crust's edge; out there the field
/// is a plateau, and noise at full strength could carry it back across
/// the shoreline — lakes of sea in the heart of a continent.
const COAST_NOISE_FAR: f32 = 0.3;

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

/// A trench lies on the plate going under, this far from the boundary's
/// line (which is the edge of the overriding crust: a coast or an arc)
/// and this wide either side of its axis. Far enough out to be in deep
/// water, near enough to end within the framework's reach.
const TRENCH_OFFSET: f32 = 2_200.0;
const TRENCH_HALF_WIDTH: f32 = 800.0;
/// Extra depth below the abyssal plain, in blocks.
const TRENCH_EXTRA: f32 = 340.0;
/// A trench, a ridge and a rift all vary along their length: this is the
/// wavelength of that, and the least share of the full shape anywhere
/// along it. Trenches have deeps and sills; rifts have lakes and dry
/// floors.
const ALONG_STRIKE_WAVELENGTH: i64 = 12_000;
const ALONG_STRIKE_FLOOR: f32 = 0.4;

// --- Ocean ridges and rifts (milestone 16) ---------------------------------

/// Where two plates part under the sea, new floor is made hot and stands
/// high, and sinks as it cools and moves away: a broad rise, steepest at
/// the crest, falling as the square root of distance — the real curve.
const RIDGE_RISE: f32 = 100.0;
/// Along the crest runs the rift the plates are parting at: a valley this
/// deep, this wide either side.
const RIDGE_RIFT_DEPTH: f32 = 60.0;
const RIDGE_RIFT_HALF_WIDTH: f32 = 300.0;
/// Where a continent is being pulled apart, a strip of it drops between
/// faults: a flat floor this wide either side of the line, walls out to
/// the second distance, and the most and least the floor drops along its
/// length. The deepest stretches go below sea level and hold long lakes.
const RIFT_FLOOR_HALF_WIDTH: f32 = 380.0;
const RIFT_WALL_HALF_WIDTH: f32 = 800.0;
const RIFT_DEPTH: (f32, f32) = (30.0, 110.0);
/// How far a rift's valley may wander from the boundary's line, blocks.
const RIFT_WANDER: f32 = 900.0;
/// The shoulders either side of a rift are pushed up as the floor goes
/// down, and fall away over the rest of the framework's reach.
const RIFT_SHOULDER: f32 = 60.0;

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
/// extent: the tectonic framework's belts (`tectonics::Boundary::belt`).
const OROGENY_FLOOR: f32 = 0.42;

/// Every world has mountains (M13, the owner's choice, 2026-11-01), and
/// about as many as the terrain had when ADR-0010 tuned it (5.0–7.6% of
/// land at the default size, version 8): at least this share of its land
/// under ranges with [`RANGE_UPLIFT`] blocks of uplift or more, or its
/// alpine biomes — and the rain shadows behind them — would be missing.
///
/// Mountains stand where the tectonic framework's belts are
/// (`tectonics::Boundary::belt`), and a world's belts are what its plates
/// happened to do. A world short of ranges has its belts strengthened
/// until it has this much ([`Elevation::calibrate_ranges`]): its old welds
/// stand higher, as old ranges do where a continent is being squeezed
/// again. A world with more keeps them.
const MIN_RANGE_SHARE: f32 = 0.05;
const RANGE_UPLIFT: f32 = 400.0;
/// The most a world's belts may be strengthened by.
const MAX_BELT_GAIN: f32 = 3.0;
/// A strong belt has a spine: along its crest the ground is raised whether
/// or not the ridged field is high there, by this share of the rest of
/// the way to full uplift, times the square of the belt's strength. So a
/// collision or an arc is one continuous range with passes in it — a
/// cordillera — while an old weld, whose belt is weak, is left as the
/// ridged field picks it out: separate worn-down massifs.
const BELT_SPINE: f32 = 0.5;
/// Uplift at full orogeny, in blocks. The ceiling a summit can reach, and only
/// where the field saturates across a very wide region.
pub(crate) const MAX_UPLIFT: f32 = 1_150.0;

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
///
/// `Clone`, cheaply: the tectonic framework is shared behind an `Arc`.
#[derive(Clone, Debug)]
pub struct Elevation {
    seed: u64,
    shape: WorldShape,
    /// Where the continents, belts, trenches, ridges and rifts are
    /// (milestone 16, ADR-0025).
    tectonics: Arc<Tectonics>,
    /// Subtracted from the continent field so this world's land fraction lands
    /// on [`TARGET_LAND_FRACTION`]. See [`Elevation::calibrate_coastline`].
    continent_offset: f32,
    /// Strengthens the mountain belts of a world that would otherwise have
    /// too few ranges; 1 in most worlds. See [`MIN_RANGE_SHARE`].
    belt_gain: f32,
    coast: [Octave; 3],
    relief: [Octave; 2],
    orogeny: [Octave; 2],
    along_strike: Octave,
    detail: [Octave; 3],
    /// Not part of the height: noise that lets biome boundaries wander
    /// (M13), kept here so it tiles with the world like every other field.
    boundary: [Octave; 3],
    /// Small-scale patches of a biome's surface (M13): sand through a
    /// steppe, gravel through tundra, outcrops on a meadow.
    patch: [Octave; 2],
    /// The meander warp (M14), an octave apart: rivers bend at a
    /// wavelength set by their width.
    meander: [Octave; 6],
}

/// What a column's surface is made of, as the elevation field knows it — the
/// height and the two stages climate classification reads (M13). Every field
/// is a pure function of `(x, z)`, so the chunk generator, the LOD sampler
/// and any readout agree on them exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    /// Blocks above sea level; see [`Elevation::height`].
    pub height: i64,
    /// [`Elevation::relief`]: how rough the region is, `[0, 1]`.
    pub relief: f32,
    /// Blocks of the height raised by mountain building (0 off the ranges,
    /// up to `MAX_UPLIFT`).
    pub uplift: f32,
}

/// Wavelengths of the biome-boundary noise, blocks: edges wander over a
/// kilometre or so, and the finest octave frays them over tens of blocks —
/// an ecotone, tongues and islands of each biome in the other, not a line
/// (M13 criterion 6).
const BOUNDARY_WAVELENGTHS: [i64; 3] = [1_400, 350, 24];
const BOUNDARY_WEIGHTS: [f32; 3] = [1.0, 0.4, 0.35];

/// Wavelengths of the meander warp, blocks, each twice the last: a river
/// picks its own between them (`Elevation::meander_warp`).
pub const MEANDER_WAVELENGTHS: [i64; 6] = [32, 64, 128, 256, 512, 1_024];

/// Wavelengths of the surface-patch noise, blocks.
const PATCH_WAVELENGTHS: [i64; 2] = [40, 11];
const PATCH_WEIGHTS: [f32; 2] = [1.0, 0.45];

impl Elevation {
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        let oct = |w: i64| Octave::new(shape, w);
        let mut e = Self {
            seed,
            shape,
            tectonics: Arc::new(Tectonics::new(seed, shape)),
            continent_offset: 0.0,
            belt_gain: 1.0,
            coast: COAST_WAVELENGTHS.map(oct),
            relief: RELIEF_WAVELENGTHS.map(oct),
            orogeny: OROGENY_WAVELENGTHS.map(oct),
            along_strike: oct(ALONG_STRIKE_WAVELENGTH),
            detail: DETAIL_WAVELENGTHS.map(oct),
            boundary: BOUNDARY_WAVELENGTHS.map(oct),
            patch: PATCH_WAVELENGTHS.map(oct),
            meander: MEANDER_WAVELENGTHS.map(oct),
        };
        e.continent_offset = e.calibrate_coastline();
        e.belt_gain = e.calibrate_ranges();
        e
    }

    /// The world's tectonic framework: what places everything here.
    pub fn tectonics(&self) -> &Tectonics {
        &self.tectonics
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
    /// The framework makes about that share of the world continental crust
    /// to begin with (`tectonics::CONTINENTAL_SHARE`), so this moves the
    /// shore a few hundred blocks, not a continent's width.
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

    /// The belt gain that gives this world at least [`MIN_RANGE_SHARE`] of
    /// its land under ranges — 1 if it already has that. Bisection on a
    /// coarse grid, like the coastline; deterministic from the seed and
    /// size.
    fn calibrate_ranges(&self) -> f32 {
        let (sx, sz) = (self.shape.size_x(), self.shape.size_z());
        // What uplift is made from, at every land sample: the ridged
        // field's share, and the framework's belt.
        let mut land = Vec::new();
        for j in 0..CALIBRATION_GRID {
            for i in 0..CALIBRATION_GRID {
                let (x, z) = (i * sx / CALIBRATION_GRID, j * sz / CALIBRATION_GRID);
                let (c, belt) = self.tectonics.column_canonical(x, z, |column| {
                    (self.continent_from(column, x, z).0, column.belt() as f32)
                });
                let profile = coastal_profile(c - self.continent_offset);
                if profile.base > 0.0 {
                    land.push((self.ridged_c(x, z), profile.land, belt));
                }
            }
        }
        let share = |gain: f32| {
            let ranges = land
                .iter()
                .filter(|&&(ridged, land, belt)| {
                    let belt = (gain * belt).min(1.0);
                    uplift(spined(ridged, belt) * land * belt) >= RANGE_UPLIFT
                })
                .count();
            ranges as f32 / land.len().max(1) as f32
        };
        if share(1.0) >= MIN_RANGE_SHARE {
            return 1.0;
        }
        let (mut lo, mut hi) = (1.0f32, MAX_BELT_GAIN);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if share(mid) >= MIN_RANGE_SHARE {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    }

    /// Surface height in blocks at world column `(x, z)`. Sea level is 0.
    ///
    /// `x`/`z` may be on any lap of the world; the result repeats exactly every
    /// world width. The column is wrapped ONCE here and every stage below works
    /// in canonical coordinates, rather than each octave wrapping for itself.
    pub fn height(&self, x: i64, z: i64) -> i64 {
        self.surface(x, z).height
    }

    /// The height with the relief and uplift it was built from, at no extra
    /// cost: the composition computes them anyway.
    pub fn surface(&self, x: i64, z: i64) -> Surface {
        self.surface_canonical(self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    /// How large this world's continents are, against the default world's:
    /// the size of its plates over the default size's. 1 at the default
    /// size and above (a plate is about [`PLATE_BLOCKS`] across wherever
    /// there is room for more than three); ~0.34 in the smallest world,
    /// whose three plates are all it can hold. Climate distances that are
    /// really "how far into a continent" scale by this (ADR-0017).
    pub fn continent_scale(&self) -> f32 {
        let area = self.shape.size_x() as f64 * self.shape.size_z() as f64;
        let plate = (area / self.tectonics.plates().len() as f64).sqrt();
        (plate / PLATE_BLOCKS as f64).min(1.0) as f32
    }

    /// Two independent noises in `[-1, 1]` for wandering biome boundaries
    /// (M13): one for temperature, one for precipitation. Hashed with salts
    /// apart from every terrain octave's, so they do not follow the terrain.
    pub fn boundary_noise(&self, x: i64, z: i64) -> (f32, f32) {
        let (x, z) = (self.shape.canonical_x(x), self.shape.canonical_z(z));
        let pair = |salt: u64| {
            let (mut sum, mut norm) = (0.0, 0.0);
            for (oct, w) in self.boundary.iter().zip(BOUNDARY_WEIGHTS.iter()) {
                let o = Octave {
                    salt: oct.salt ^ salt,
                    ..*oct
                };
                sum += self.value_noise(x, z, &o) * w;
                norm += w;
            }
            sum / norm
        };
        (pair(0x7E3D_0001), pair(0x9A1F_0002))
    }

    /// Surface-patch noise in `[-1, 1]` (M13): decides, within a biome,
    /// which columns take its secondary surface — tens of blocks across.
    pub fn patch_noise(&self, x: i64, z: i64) -> f32 {
        let (x, z) = (self.shape.canonical_x(x), self.shape.canonical_z(z));
        let o = |oct: &Octave| Octave {
            salt: oct.salt ^ 0x5A7C_0003,
            ..*oct
        };
        let a = self.value_noise(x, z, &o(&self.patch[0]));
        let b = self.value_noise(x, z, &o(&self.patch[1]));
        (a * PATCH_WEIGHTS[0] + b * PATCH_WEIGHTS[1]) / (PATCH_WEIGHTS[0] + PATCH_WEIGHTS[1])
    }

    /// The meander warp at a column (M14): a smooth displacement, each
    /// component in `[-1, 1]`, varying at `wavelength` blocks — blended
    /// between the two octaves either side of it, so a river widening
    /// downstream lengthens its bends smoothly. Not part of the height;
    /// salted apart from every terrain octave, and tiled with the world.
    pub fn meander_warp(&self, x: i64, z: i64, wavelength: f32) -> (f32, f32) {
        let (x, z) = (self.shape.canonical_x(x), self.shape.canonical_z(z));
        let w = wavelength.clamp(
            MEANDER_WAVELENGTHS[0] as f32,
            MEANDER_WAVELENGTHS[MEANDER_WAVELENGTHS.len() - 1] as f32,
        );
        let k = MEANDER_WAVELENGTHS[..MEANDER_WAVELENGTHS.len() - 1]
            .iter()
            .rposition(|&m| m as f32 <= w)
            .unwrap_or(0);
        let lo = MEANDER_WAVELENGTHS[k] as f32;
        let t = ((w - lo) / lo).clamp(0.0, 1.0);
        let pair = |oct: &Octave| {
            let o = |salt: u64| Octave {
                salt: oct.salt ^ salt,
                ..*oct
            };
            (
                self.value_noise(x, z, &o(0x3EA2_0001)),
                self.value_noise(x, z, &o(0x3EA2_0002)),
            )
        };
        let a = pair(&self.meander[k]);
        if t == 0.0 {
            return a; // on an octave: the other weighs nothing
        }
        let b = pair(&self.meander[k + 1]);
        (lerp(a.0, b.0, t), lerp(a.1, b.1, t))
    }

    /// The single composition point. Every stage feeds in here and nowhere
    /// else, so a future erosion pass has one place to attach.
    fn surface_canonical(&self, x: i64, z: i64) -> Surface {
        self.tectonics
            .column_canonical(x, z, |column| self.surface_of(column, x, z))
    }

    /// [`Elevation::surface_canonical`], given the column's place in the
    /// tectonic framework.
    fn surface_of(&self, column: &crate::tectonics::Column, x: i64, z: i64) -> Surface {
        let (c, wander) = self.continent_from(column, x, z);
        let c = c - self.continent_offset;
        let CoastalProfile {
            mut base,
            land,
            deep,
        } = coastal_profile(c);

        // What the boundaries near this column do to it: the mountain belt
        // it is under, and — as heights — the trench, the ocean ridge or
        // the rift it is in. The shelf, between land and deep water, has
        // none of them, and is not asked.
        let (mut belt, mut trench, mut ridge) = (0.0f32, 0.0f32, 0.0f32);
        let (mut rift_floor, mut rift_shoulder) = (0.0f32, 0.0f32);
        if land > 0.0 || deep > 0.0 {
            column.boundaries(|boundary, over, fade| {
                let (over, fade) = (over as f32, fade as f32);
                belt = belt.max(boundary.belt(f64::from(over)) as f32 * fade);
                match boundary {
                    // The plate going under bends down into a trench off
                    // the edge of the one above.
                    Boundary::Convergent {
                        kind: Convergence::ContinentalArc | Convergence::IslandArc,
                        ..
                    } => {
                        let t = (-over - TRENCH_OFFSET) / TRENCH_HALF_WIDTH;
                        trench = trench.max((1.0 - smoothstep01(t.abs())) * fade);
                    }
                    Boundary::Ridge => {
                        let t = over.abs();
                        let rise = RIDGE_RISE * (1.0 - (t / BOUNDARY_REACH as f32).sqrt());
                        let cut =
                            RIDGE_RIFT_DEPTH * (1.0 - smoothstep01(t / RIDGE_RIFT_HALF_WIDTH));
                        ridge = ridge.max((rise - cut) * fade);
                    }
                    Boundary::Rift => {
                        // The valley wanders either side of the line it
                        // follows, by the noise that crinkles coasts: a
                        // rift is a chain of basins, not a canal.
                        let t = (over + wander * RIFT_WANDER).abs();
                        let wall = smoothstep01(
                            (t - RIFT_FLOOR_HALF_WIDTH)
                                / (RIFT_WALL_HALF_WIDTH - RIFT_FLOOR_HALF_WIDTH),
                        );
                        let away = smoothstep01(
                            (t - RIFT_WALL_HALF_WIDTH)
                                / (BOUNDARY_REACH as f32 - RIFT_WALL_HALF_WIDTH),
                        );
                        // Down on the floor, up the wall to the shoulder,
                        // then away to nothing.
                        rift_floor = rift_floor.max((1.0 - wall) * fade);
                        rift_shoulder = rift_shoulder.max(wall * (1.0 - away) * fade);
                    }
                    _ => {}
                }
            });
        }
        // How much of a trench, a ridge's rift or a continental rift is
        // here, of the most there could be: they vary along their length.
        let along = if trench > 0.0 || rift_floor > 0.0 {
            let n = self.value_noise(x, z, &self.along_strike) * 0.5 + 0.5;
            ALONG_STRIKE_FLOOR + (1.0 - ALONG_STRIKE_FLOOR) * n
        } else {
            1.0
        };

        // Sea floor: gated on `deep` so a trench cannot appear off a beach,
        // nor a ridge stand on a shelf.
        base += (ridge - trench * along * TRENCH_EXTRA) * deep;
        // A rift's floor drops by an amount that varies along it; its
        // shoulders stand the same all the way.
        let rift_depth = lerp(
            RIFT_DEPTH.0,
            RIFT_DEPTH.1,
            (along - ALONG_STRIKE_FLOOR) / (1.0 - ALONG_STRIKE_FLOOR),
        );
        base += (rift_shoulder * RIFT_SHOULDER - rift_floor * rift_depth) * land;

        // Mountains: the ridged field, let through by the framework's
        // belt. `uplift` is the earned-by-extent rule — see it.
        let massif = if land > 0.0 && belt > 0.0 {
            let belt = (self.belt_gain * belt).min(1.0);
            uplift(spined(self.ridged_c(x, z), belt) * land * belt)
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
        Surface {
            height: (h.round() as i64).clamp(
                WORLD_Y_MIN_BLOCKS + BOUND_MARGIN,
                WORLD_Y_MAX_BLOCKS - BOUND_MARGIN,
            ),
            relief,
            uplift: massif,
        }
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
        self.tectonics
            .column_canonical(x, z, |column| self.continent_from(column, x, z).0)
    }

    /// The continent field from a column's search of the framework: how
    /// far inside continental crust it is, as the field's slope at a
    /// coast, with the coast's own noise — which is what makes a shore of
    /// a crust's edge. With the field comes that noise itself, in
    /// `[-1, 1]`, for whatever else should wander as the coast does.
    fn continent_from(&self, column: &crate::tectonics::Column, x: i64, z: i64) -> (f32, f32) {
        let inland = column.continentalness() as f32;
        let far = smoothstep01(inland.abs() / COAST_REACH as f32 * 2.0 - 1.0);
        let wander = self.fbm(x, z, &self.coast, &COAST_WEIGHTS);
        let noise = wander * COAST_NOISE * lerp(1.0, COAST_NOISE_FAR, far);
        let per_unit = if inland >= 0.0 {
            COAST_BLOCKS_PER_UNIT.0
        } else {
            COAST_BLOCKS_PER_UNIT.1
        };
        (inland / per_unit + noise, wander)
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

    /// [`Elevation::orogeny`] above its floor, as a share of the rest: what
    /// uplift is made from, before the coast and the belt let it through.
    fn ridged_c(&self, x: i64, z: i64) -> f32 {
        ((self.orogeny_c(x, z) - OROGENY_FLOOR) / (1.0 - OROGENY_FLOOR)).clamp(0.0, 1.0)
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

/// Blocks of uplift for a share `u` of full mountain building.
///
/// Squared smoothstep, not a raw cube. Both make high uplift rare, but a
/// cube's slope keeps CLIMBING to the summit, so the crest ends up the
/// steepest part of the mountain — and ridge noise puts a crease there
/// already. Smoothstep flattens to zero slope at both ends: gentle
/// foothills, a rounded crest, the steep ground in between where a
/// mountain's steep ground belongs. This is the earned-by-extent rule:
/// the ridged field is low-frequency, so a high value implies a wide
/// region, and squaring makes the payoff for width steep.
fn uplift(u: f32) -> f32 {
    let shaped = smoothstep(u);
    shaped * shaped * MAX_UPLIFT
}

/// The ridged field with a belt's spine through it: see [`BELT_SPINE`].
fn spined(ridged: f32, belt: f32) -> f32 {
    ridged + (1.0 - ridged) * BELT_SPINE * belt * belt
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
pub(crate) fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every world has ranges (M13), of any size and whatever its plates
    /// did: one short of them has its belts strengthened to the minimum.
    #[test]
    fn every_world_has_mountain_ranges() {
        let range_share = |e: &Elevation| {
            let size = e.shape.size_x();
            let (mut land, mut ranges) = (0u32, 0u32);
            for j in 0..128 {
                for i in 0..128 {
                    let s = e.surface(i * size / 128, j * size / 128);
                    if s.height > 0 {
                        land += 1;
                        ranges += u32::from(s.uplift >= RANGE_UPLIFT);
                    }
                }
            }
            ranges as f32 / land as f32
        };
        let mut strengthened = 0;
        for size in [32_768, 65_536, WorldShape::DEFAULT.size_x()] {
            let shape = WorldShape::new(size, size).unwrap();
            for seed in 0..8u64 {
                let e = Elevation::new(seed.wrapping_mul(0x9E37_79B9) ^ 0x7e22ac0de, shape);
                strengthened += u32::from(e.belt_gain > 1.0);
                assert!((1.0..=MAX_BELT_GAIN).contains(&e.belt_gain));
                assert!(
                    range_share(&e) >= 0.6 * MIN_RANGE_SHARE,
                    "size {size}, seed {seed}: {:.3} of land under ranges (gain {})",
                    range_share(&e),
                    e.belt_gain
                );
            }
        }
        // Most worlds have ranges enough from their plates alone; how many
        // needed help is worth a line in the log.
        eprintln!("{strengthened} of 24 worlds had their belts strengthened");
    }

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

    /// Everything near a column that the framework says shapes it.
    fn near(e: &Elevation, x: i64, z: i64) -> Vec<(Boundary, f64, f64)> {
        let mut found = Vec::new();
        e.tectonics().column(x, z, |column| {
            column.boundaries(|boundary, over, fade| found.push((boundary, over, fade)));
        });
        found
    }

    /// Milestone 16, criterion 3: the terrain is where the framework puts
    /// it. No mountain stands off a belt, and no high one stands anywhere
    /// but on a collision, an arc or a weld being squeezed today; every
    /// trench is on the plate going under, off the edge of the one above.
    #[test]
    fn mountains_and_trenches_are_where_the_plates_put_them() {
        for seed in [SEED, 1, 2, 3] {
            let e = Elevation::new(seed, WorldShape::DEFAULT);
            let size = WorldShape::DEFAULT.size_x();
            let (mut uplifted, mut high, mut trenches) = (0u32, 0u32, 0u32);
            for k in 0..120_000i64 {
                let (x, z) = (k * 7_919 % size, k * 104_729 % size);
                let s = e.surface(x, z);
                if s.uplift > 0.0 {
                    uplifted += 1;
                    assert!(
                        e.tectonics().belt(x, z) > 0.0,
                        "seed {seed}: {} of uplift at ({x}, {z}), off every belt",
                        s.uplift
                    );
                }
                // 700 blocks of uplift needs a belt of 0.68 or more: only
                // the strong kinds reach it (unless this world's belts
                // were strengthened, which the gain accounts for).
                if s.uplift >= 700.0 {
                    high += 1;
                    let strongest = near(&e, x, z)
                        .iter()
                        .map(|&(b, over, fade)| b.belt(over) * fade)
                        .fold(0.0, f64::max);
                    assert!(
                        strongest * f64::from(e.belt_gain) >= 0.68,
                        "seed {seed}: a {}-block massif at ({x}, {z}) on a belt of {strongest}",
                        s.uplift
                    );
                    assert!(
                        near(&e, x, z)
                            .iter()
                            .any(|&(b, _, _)| b.belt_strength() * f64::from(e.belt_gain) >= 0.68),
                        "seed {seed}: a high range at ({x}, {z}) on no strong boundary"
                    );
                }
                if (s.height as f32) < -(ABYSSAL_DEPTH + TRENCH_EXTRA * 0.5) {
                    trenches += 1;
                    assert!(
                        near(&e, x, z).iter().any(|&(b, over, _)| matches!(
                            b,
                            Boundary::Convergent {
                                kind: Convergence::ContinentalArc | Convergence::IslandArc,
                                ..
                            }
                        ) && over < 0.0),
                        "seed {seed}: a trench at ({x}, {z}) that is not on a plate going under"
                    );
                }
            }
            assert!(
                uplifted > 5_000 && high > 100 && trenches > 100,
                "seed {seed}: {uplifted} uplifted, {high} high, {trenches} in trenches"
            );
        }
    }

    /// The sea floor rises to a ridge where plates part, and a continent
    /// being pulled apart drops a valley between raised shoulders.
    #[test]
    fn ridges_rise_and_rifts_drop() {
        let (mut crest, mut plain) = (Vec::new(), Vec::new());
        let (mut floor, mut shoulder) = (Vec::new(), Vec::new());
        for seed in [SEED, 1, 2, 3] {
            let e = Elevation::new(seed, WorldShape::DEFAULT);
            let size = WorldShape::DEFAULT.size_x();
            for k in 0..200_000i64 {
                let (x, z) = (k * 7_919 % size, k * 104_729 % size);
                let s = e.surface(x, z);
                let around = near(&e, x, z);
                // Deep water only: on a shelf nothing is raised.
                if s.height < -150 {
                    let ridge = around
                        .iter()
                        .find(|&&(b, _, fade)| b == Boundary::Ridge && fade > 0.9);
                    match ridge {
                        Some(&(_, over, _)) if (400.0..1_000.0).contains(&over.abs()) => {
                            crest.push(s.height);
                        }
                        None if around.is_empty() => plain.push(s.height),
                        _ => {}
                    }
                }
                if s.height > -200 && s.uplift == 0.0 {
                    let rift = around
                        .iter()
                        .find(|&&(b, _, fade)| b == Boundary::Rift && fade > 0.9);
                    if let Some(&(_, over, _)) = rift {
                        // The valley wanders up to RIFT_WANDER from its
                        // line, so "the floor" is taken as the lowest and
                        // "the shoulder" well clear of it.
                        if over.abs() < 200.0 {
                            floor.push(s.height);
                        } else if (1_900.0..2_600.0).contains(&over.abs()) {
                            shoulder.push(s.height);
                        }
                    }
                }
            }
        }
        let mean = |v: &[i64]| v.iter().sum::<i64>() as f64 / v.len() as f64;
        assert!(
            crest.len() > 300 && plain.len() > 3_000,
            "{} {}",
            crest.len(),
            plain.len()
        );
        assert!(
            mean(&crest) > mean(&plain) + 35.0,
            "ridge flanks at {:.0}, the plain at {:.0}",
            mean(&crest),
            mean(&plain)
        );
        assert!(
            floor.len() > 60 && shoulder.len() > 100,
            "{} {}",
            floor.len(),
            shoulder.len()
        );
        assert!(
            mean(&floor) < mean(&shoulder) - 25.0,
            "rift floors at {:.0}, their shoulders at {:.0}",
            mean(&floor),
            mean(&shoulder)
        );
    }

    /// Tool: the numbers ADR-0010 tuned the terrain to, world by world —
    /// what milestone 16 must hold while it moves where things are
    /// (`cargo test -p vox-worldgen --release terrain_stats -- --ignored
    /// --nocapture`; `SURVEY_SEEDS`, `WORLD_MAP_SIZE`).
    ///
    /// Sampled along east–west lines 16 blocks apart in x: land share;
    /// the share of land under ranges and how uplift is spread over it;
    /// land heights; how much ground a 3, 9 and 30-minute walk spans; how
    /// wide a landmass, a sea, a range and the gap between ranges are;
    /// how far the shore is from deep water and from the lowlands' full
    /// height; and the share of the world in trenches.
    #[test]
    #[ignore]
    fn terrain_stats() {
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<i64>().ok());
        let seeds = env("SURVEY_SEEDS").unwrap_or(6) as u64;
        let size = env("WORLD_MAP_SIZE").unwrap_or(WorldShape::DEFAULT.size_x());
        let shape = WorldShape::new(size, size).unwrap();
        const STEP: i64 = 16;
        let lines = 96i64;
        let median = |v: &mut Vec<i64>| {
            v.sort_unstable();
            if v.is_empty() { 0 } else { v[v.len() / 2] }
        };
        let mean = |v: &[i64]| v.iter().sum::<i64>() as f64 / v.len().max(1) as f64;
        println!(
            "seed  land% | range% up>0 >100 >400 >800 | h50 h90 h99 max | walk 1k 3k 10k | flat% \
             | landmass sea  range gap(mean) | shore>deep shore>low | trench%"
        );
        for seed in 0..seeds {
            let seed = if seed == 0 { SEED } else { seed };
            let e = Elevation::new(seed, shape);
            let n = size / STEP;
            let (mut land, mut total) = (0i64, 0i64);
            let mut up = [0i64; 4];
            let mut heights = Vec::new();
            let mut walks = [Vec::new(), Vec::new(), Vec::new()];
            let (mut flat, mut flat_of) = (0i64, 0i64);
            let (mut land_runs, mut sea_runs, mut range_runs) =
                (Vec::new(), Vec::new(), Vec::new());
            let (mut ranges_met, mut land_length) = (0i64, 0i64);
            let (mut to_deep, mut to_low) = (Vec::new(), Vec::new());
            let mut trench = 0i64;
            for line in 0..lines {
                let z = line * size / lines + 37;
                let row: Vec<Surface> = (0..n).map(|i| e.surface(i * STEP, z)).collect();
                let at = |i: i64| row[i.rem_euclid(n) as usize];
                // Start each line on a coast, so runs are whole.
                let start = (0..n)
                    .find(|&i| (at(i).height > 0) != (at(i - 1).height > 0))
                    .unwrap_or(0);
                let (mut run, mut range_run) = (0i64, 0i64);
                for k in 0..n {
                    let i = start + k;
                    let s = at(i);
                    let is_land = s.height > 0;
                    total += 1;
                    trench += i64::from((s.height as f32) < -(ABYSSAL_DEPTH + TRENCH_EXTRA * 0.5));
                    if is_land != (at(i - 1).height > 0) && k > 0 {
                        if is_land {
                            sea_runs.push(run * STEP)
                        } else {
                            land_runs.push(run * STEP)
                        }
                        run = 0;
                    }
                    run += 1;
                    if is_land != (at(i - 1).height > 0) {
                        // A coast: how far out is deep water, how far in
                        // the lowlands' full rise.
                        let (out, inward) = if is_land { (-1, 1) } else { (1, -1) };
                        let reach = |dir: i64, ok: &dyn Fn(i64) -> bool| {
                            (0..n / 4)
                                .find(|&d| ok(at(i + dir * d).height))
                                .map(|d| d * STEP)
                        };
                        if let Some(d) = reach(out, &|h| h <= -130) {
                            to_deep.push(d);
                        }
                        if let Some(d) = reach(inward, &|h| h >= 35) {
                            to_low.push(d);
                        }
                    }
                    if !is_land {
                        continue;
                    }
                    land += 1;
                    land_length += STEP;
                    heights.push(s.height);
                    for (slot, bar) in up.iter_mut().zip([0.5, 100.0, 400.0, 800.0]) {
                        *slot += i64::from(s.uplift >= bar);
                    }
                    let in_range = s.uplift >= 200.0;
                    if in_range {
                        range_run += 1;
                    } else if range_run > 0 {
                        range_runs.push(range_run * STEP);
                        ranges_met += 1;
                        range_run = 0;
                    }
                    if k % 64 == 0 {
                        flat_of += 1;
                        flat += i64::from((at(i + 1).height - s.height).abs() <= 2);
                        for (slot, span) in walks.iter_mut().zip([1_000, 3_000, 10_000]) {
                            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
                            for j in 0..=span / STEP {
                                let h = at(i + j).height;
                                (lo, hi) = (lo.min(h), hi.max(h));
                            }
                            slot.push(hi - lo);
                        }
                    }
                }
            }
            heights.sort_unstable();
            let h = |q: f64| heights[((heights.len() - 1) as f64 * q) as usize];
            let pct = |a: i64, b: i64| 100.0 * a as f64 / b.max(1) as f64;
            println!(
                "{:4x} {:5.1} | {:5.2} {:5.1} {:4.1} {:4.1} {:4.2} | {:3} {:3} {:4} {:4} | {:4} {:4} {:4} \
                 | {:4.1} | {:6.0} {:6.0} {:5.0} {:6.0} | {:5} {:5} | {:.3}",
                seed & 0xffff,
                pct(land, total),
                pct(up[2], land),
                pct(up[0], land),
                pct(up[1], land),
                pct(up[2], land),
                pct(up[3], land),
                h(0.5),
                h(0.9),
                h(0.99),
                h(1.0),
                median(&mut walks[0]),
                median(&mut walks[1]),
                median(&mut walks[2]),
                pct(flat, flat_of),
                mean(&land_runs),
                mean(&sea_runs),
                mean(&range_runs),
                land_length as f64 / ranges_met.max(1) as f64,
                median(&mut to_deep),
                median(&mut to_low),
                pct(trench, total),
            );
        }
    }

    /// Tool: what a column's height costs, three ways a caller asks for it
    /// (`cargo test -p vox-worldgen --release height_cost -- --ignored
    /// --nocapture`): columns scattered over the world; whole 32 × 32
    /// chunk columns, as generation asks; and a grid 64 blocks apart, as a
    /// far LOD level does. Best of seven.
    #[test]
    #[ignore]
    fn height_cost() {
        let e = Elevation::new(SEED, WorldShape::DEFAULT);
        const N: i64 = 409_600;
        let time = |name: &str, at: &dyn Fn(i64) -> (i64, i64)| {
            let (mut best, mut sum) = (f64::MAX, 0i64);
            for _ in 0..7 {
                let start = std::time::Instant::now();
                sum = (0..N)
                    .map(|k| {
                        let (x, z) = at(k);
                        e.height(x, z)
                    })
                    .sum();
                best = best.min(start.elapsed().as_nanos() as f64 / N as f64);
            }
            println!("height, {name:14} {best:6.0} ns/column   (checksum {sum})");
        };
        time("scattered", &|k| {
            (k * 7_919 % 204_800, k * 104_729 % 204_800)
        });
        time("chunk columns", &|k| {
            let (patch, i) = (k / 1_024, k % 1_024);
            (
                patch * 7_919 % 204_800 + i % 32,
                patch * 104_729 % 204_800 + i / 32,
            )
        });
        time("grid of 64", &|k| {
            let (patch, i) = (k / 1_024, k % 1_024);
            (
                patch * 7_919 % 204_800 + 64 * (i % 32),
                patch * 104_729 % 204_800 + 64 * (i / 32),
            )
        });
    }
}
