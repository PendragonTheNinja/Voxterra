//! Landform processes: the fourth stage-1 map (M15 task 1, ADR-0023).
//!
//! M13 gave each place a climate and M14 its rivers; the ground itself is
//! still one kind of terrain everywhere. On Earth each climate shapes the
//! land with its own processes — wind piles sand, flash floods cut dry
//! plateaus, ice gouges valleys, deep weathering leaves domes on tropical
//! plains, rain rounds and dissects humid hills (`docs/landforms.md`). This
//! map says, for every place, how strongly each of the five acts there: a
//! weight in `[0, 1]` per process. The landform layers (M15 tasks 2–4) are
//! multiplied by it; nothing here changes a block.
//!
//! ## Why a stage-1 map
//!
//! Four of the weights could be computed per column from the climate map
//! and the elevation field. Wind cannot: a dune sea is where sand *ends
//! up*, which depends on what lies upwind — the same reason rainfall needed
//! stage 1 (ADR-0017). So sand is carried along each row of the grid as
//! moisture is, and the rest are computed beside it on the same grid, at
//! the climate map's resolution, so every weight reads the same cells.
//!
//! ## The weights (the catalogue's table, as arithmetic)
//!
//! "Aridity" is precipitation over Köppen's threshold
//! ([`crate::biome::aridity_threshold`]), the index the biomes use: under
//! 0.5 desert, 0.5–1 steppe, above 1 humid. Heights and relief are the
//! UNERODED base, as climate reads them: landforms come before the
//! drainage network (the M15 spec, *Water must still fit the ground*).
//!
//! - **Wind** — where sand comes to rest: [`sand_sweep`].
//! - **Flash flood** — dry uplands: arid, above the lowlands, off the
//!   mountains, stronger the rougher (badlands).
//! - **Ice** — where ice lay in the last ice age: [`ice_age_cooling`].
//! - **Weathering** — old tropical plains: hot, neither desert nor
//!   rainforest, flat, never glaciated.
//! - **Rain** — humid land off the mountains.
//!
//! ## Deterministic on every machine
//!
//! As in [`crate::climate`]: only `+ − × ÷`, comparisons and `floor`; rows
//! and cells are computed independently and nothing is reduced across
//! threads.

use rayon::prelude::*;
use vox_core::WorldShape;

use crate::biome::{MOUNTAIN_UPLIFT, aridity_threshold};
use crate::climate::{ClimateMap, blur, eastward_share, smoothstep, temperature_at};
use crate::elevation::{Elevation, MAX_UPLIFT};

/// One place's process weights, each in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// Sand comes to rest here: dune seas.
    pub wind: f32,
    /// Flash floods on a dry upland: benches, mesas, canyons.
    pub flood: f32,
    /// Ice lay here in the last ice age: troughs, cirques, fjords.
    pub ice: f32,
    /// Deep tropical weathering on an old plain: inselbergs.
    pub weathering: f32,
    /// Plentiful rain: rounded, dissected hills.
    pub rain: f32,
}

impl Weights {
    pub const NONE: Self = Self {
        wind: 0.0,
        flood: 0.0,
        ice: 0.0,
        weathering: 0.0,
        rain: 0.0,
    };

    /// In the order of the catalogue: wind, flash flood, ice, weathering,
    /// rain.
    pub fn as_array(self) -> [f32; 5] {
        [self.wind, self.flood, self.ice, self.weathering, self.rain]
    }
}

/// What one grid cell is, as the weights read it.
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Base height at the cell's centre, blocks above sea level.
    height: f32,
    /// [`Elevation::relief`], `[0, 1]`.
    relief: f32,
    /// 0 off the ranges, 1 where the biomes call the ground mountain.
    mountain: f32,
    land: bool,
    /// Mean annual temperature at the ground, °C.
    temperature: f32,
    /// Annual precipitation, mm.
    precip: f32,
    /// Precipitation over Köppen's aridity threshold.
    aridity: f32,
    /// |sin(latitude)|.
    abs_s: f32,
}

// --- The ice age -----------------------------------------------------------

/// How much colder the last ice age was at this latitude, °C.
///
/// Earth ~20 000 years ago was ~4 °C colder in the tropics and 10–15 °C
/// colder at high latitudes. Ours: 4 °C to 20° of latitude, rising smoothly
/// to 11 °C at 50° and beyond. With M13's latitude curve and lapse rate that
/// puts ice-age ice at sea level from ~47° poleward (Earth's sheets reached
/// 40–50°) and on any ground above ~720 blocks at the equator
/// (`docs/landforms.md`, *The ice-age climate*).
pub fn ice_age_cooling(abs_s: f32) -> f32 {
    // sin 20° = 0.342, sin 50° = 0.766.
    ICE_AGE_TROPICS_C + (ICE_AGE_POLES_C - ICE_AGE_TROPICS_C) * smoothstep(0.342, 0.766, abs_s)
}
const ICE_AGE_TROPICS_C: f32 = 4.0;
const ICE_AGE_POLES_C: f32 = 11.0;

/// The ice weight from a ground's ICE-AGE mean annual temperature: glaciers
/// flow below their snowline, so ground counts as glaciated from 0 °C, not
/// from the −5 °C snowline. Half at 0 °C, full from −2, none above +2: the
/// band is the ice margin, where valleys were only sometimes occupied.
///
/// And ice needs snow. Cold alone built no ice sheet over Siberia or
/// Beringia in the last ice age — they were polar desert, frozen and
/// bare, and are unglaciated lowland today — while wetter Scandinavia and
/// Canada at the same latitudes lay under kilometres of ice. So the weight
/// falls away where precipitation is too little to outlast a summer's melt.
///
/// Unless there is no summer melt at all. Antarctica's interior gets less
/// snow than the Sahara gets rain and holds four kilometres of ice, because
/// none of it ever thaws. Below [`ICE_NO_MELT_C`] cold alone is enough —
/// which is also what keeps this weight at 1 everywhere M13's ice-sheet
/// biome stands today, whatever the precipitation.
pub fn ice_weight(ice_age_c: f32, precip_mm: f32) -> f32 {
    let cold = 1.0 - smoothstep(-ICE_MARGIN_C, ICE_MARGIN_C, ice_age_c);
    let snow = smoothstep(ICE_SNOW_MM.0, ICE_SNOW_MM.1, precip_mm);
    let no_melt = 1.0 - smoothstep(ICE_NO_MELT_C.0, ICE_NO_MELT_C.1, ice_age_c);
    cold * (snow + (1.0 - snow) * no_melt)
}
const ICE_MARGIN_C: f32 = 2.0;
/// Ice-age mean annual temperature below which nothing melts and ice
/// builds from any snowfall (fully from the first value), °C. The ice-sheet
/// biome is ground below −14 °C today, 18–25 °C colder than these then.
const ICE_NO_MELT_C: (f32, f32) = (-16.0, -8.0);
/// Annual precipitation under which cold ground stays bare, and from which
/// it carries ice fully, mm. Today's values stand in for the ice age's (it
/// was drier, but so was everywhere; the contrast between wet and dry cold
/// lands is what matters).
const ICE_SNOW_MM: (f32, f32) = (150.0, 400.0);

// --- The dry lands ---------------------------------------------------------

/// Aridity below which ground is desert-dry for the wind (full strength),
/// and above which sand no longer moves at all: vegetation holds it.
const WIND_ARID: (f32, f32) = (0.4, 0.6);
/// Height above which ground is a plateau, not a basin, blocks: dunes stay
/// off the uplands (lowlands are ~70, ADR-0010).
const WIND_LOWLAND: (f32, f32) = (110.0, 160.0);
/// Aridity up to which flash floods shape the land, and from which rain is
/// regular enough to round it instead.
const FLOOD_ARID: (f32, f32) = (0.6, 0.9);
/// Height from which dry ground is an upland, blocks.
const FLOOD_UPLAND: (f32, f32) = (80.0, 130.0);
/// Relief over which flash-flood sculpture strengthens toward the badlands'
/// (`biome::BADLANDS_RELIEF` = 0.45); a flat dry upland still has benches.
const FLOOD_RELIEF: (f32, f32) = (0.25, 0.5);
const FLOOD_FLAT: f32 = 0.6;

/// Mean annual temperature from which dry ground is worked by wind and
/// flash floods rather than frost, °C: cold deserts (the Gobi, ~0–8 °C) are,
/// polar deserts are not.
const DRY_THAWED_C: (f32, f32) = (-4.0, 2.0);

/// Sand: how much a cell of bare desert gives the wind, per cell crossed.
const SAND_SUPPLY: f32 = 1.0;
/// Cold deserts weather rock to sand more slowly and are mostly gravel and
/// bare rock (the Gobi): this share of a hot desert's supply, reached below
/// [`SAND_COLD_C`].
const SAND_COLD_SUPPLY: f32 = 0.35;
const SAND_COLD_C: (f32, f32) = (6.0, 18.0);
/// Share of the wind's load a perfect trap takes per cell: a flat basin
/// floor below the ground upwind.
const SAND_DEPOSIT: f32 = 0.45;
/// How far below the ground upwind a cell must lie to trap sand, blocks:
/// none until it is the first value lower, fully from the second. Level
/// ground traps nothing — sand crosses a flat gravel plain and keeps going,
/// which is why most of a desert is bare and its sand is gathered into
/// seas. (With any trapping on the level, every cell of a long plain ends
/// up holding exactly what it supplies, and the whole desert is thin sand.)
const SAND_DROP: (f32, f32) = (3.0, 30.0);
/// Relief over which a cell stops holding sand: dunes need flat ground.
const SAND_FLAT: (f32, f32) = (0.18, 0.40);
/// How quickly the "ground upwind" follows the ground crossed, per cell:
/// ~16 cells, four kilometres — so a basin goes on trapping well in from
/// its upwind edge, and a sea can fill it (`docs/landforms.md`: seas 2–15
/// km across).
const SAND_UPWIND_MIX: f32 = 0.06;
/// Sand at rest from which a cell is a dune sea, and from which it is one
/// fully. Tuned, with the constants above, to the catalogue's coverage —
/// about a fifth to a third of hot-desert land, a tenth of cold — measured
/// over seeds by `process_survey`.
const SAND_ERG: (f32, f32) = (0.85, 1.25);

// --- The wet lands ---------------------------------------------------------

/// Mean annual temperature over which deep chemical weathering runs, °C
/// (`biome::TROPICAL_FROM_C` = 20).
const WEATHERING_HOT_C: (f32, f32) = (18.0, 22.0);
/// Aridity window of savanna and tropical dry forest: wetter than desert,
/// drier than the rainforest's wettest.
const WEATHERING_FROM: (f32, f32) = (0.5, 0.8);
const WEATHERING_TO: (f32, f32) = (1.7, 2.3);
/// Relief over which a plain stops being one.
const WEATHERING_FLAT: (f32, f32) = (0.15, 0.35);
/// Aridity from which rain shapes the land, and at which it does so fully.
const RAIN_HUMID: (f32, f32) = (0.8, 3.0);

/// Uplift, as a share of the tallest, over which ground turns from foothill
/// to mountain: centred on the biomes' own line.
const MOUNTAIN: (f32, f32) = (0.5 * MOUNTAIN_UPLIFT, 1.5 * MOUNTAIN_UPLIFT);

/// Every weight but wind, from one cell.
fn local_weights(c: &Cell) -> Weights {
    let ice = ice_weight(c.temperature - ice_age_cooling(c.abs_s), c.precip);
    if !c.land {
        // The sea floor is shaped by none of these; ice is kept so a fjord
        // can be cut below sea level where a glaciated valley meets it.
        return Weights {
            ice,
            ..Weights::NONE
        };
    }
    let off_mountains = 1.0 - c.mountain;
    // A flash flood is a rainstorm on bare ground: a polar desert is as dry
    // as the Sahara and has none. Frozen ground belongs to the ice.
    let thawed = smoothstep(DRY_THAWED_C.0, DRY_THAWED_C.1, c.temperature) * (1.0 - ice);
    let flood = thawed
        * (1.0 - smoothstep(FLOOD_ARID.0, FLOOD_ARID.1, c.aridity))
        * smoothstep(FLOOD_UPLAND.0, FLOOD_UPLAND.1, c.height)
        * off_mountains
        * (FLOOD_FLAT + (1.0 - FLOOD_FLAT) * smoothstep(FLOOD_RELIEF.0, FLOOD_RELIEF.1, c.relief));
    let weathering = smoothstep(WEATHERING_HOT_C.0, WEATHERING_HOT_C.1, c.temperature)
        * smoothstep(WEATHERING_FROM.0, WEATHERING_FROM.1, c.aridity)
        * (1.0 - smoothstep(WEATHERING_TO.0, WEATHERING_TO.1, c.aridity))
        * (1.0 - smoothstep(WEATHERING_FLAT.0, WEATHERING_FLAT.1, c.relief))
        * off_mountains
        * (1.0 - ice);
    let rain = smoothstep(RAIN_HUMID.0, RAIN_HUMID.1, c.aridity) * off_mountains;
    Weights {
        wind: 0.0,
        flood,
        ice,
        weathering,
        rain,
    }
}

/// Carry sand along one row of the grid in one wind direction, twice round
/// the torus so the start does not matter, and return what comes to rest in
/// each cell.
///
/// The mechanism, mirrored from Earth's ergs: bare dry ground gives up
/// sand; the wind carries it; it settles where the ground falls away below
/// what the wind has just crossed and is flat enough to hold it — a basin
/// floor, the lee of an upland — and stays in the air over rough or rising
/// ground. Vegetation stops it: over ground too wet to be desert the load
/// is shed and held (no dunes form there), and the sea takes all of it.
/// So dune seas lie in desert basins downwind of their sand, in patches,
/// and most of a desert is bare rock and gravel — as most of the Sahara is.
fn sand_sweep(row: &[Cell], eastward: bool) -> Vec<f32> {
    let n = row.len();
    let mut rest = vec![0.0f32; n];
    let mut load = 0.0f32;
    let mut upwind = 0.0f32;
    for step in 0..2 * n {
        let i = if eastward { step % n } else { n - 1 - step % n };
        let c = &row[i];
        if !c.land {
            load = 0.0;
            upwind = 0.0; // the sea's level
            continue;
        }
        let dry = 1.0 - smoothstep(WIND_ARID.0, WIND_ARID.1, c.aridity);
        // Ground too wet to be desert holds what arrives and yields nothing.
        load *= dry;
        let cold = SAND_COLD_SUPPLY
            + (1.0 - SAND_COLD_SUPPLY) * smoothstep(SAND_COLD_C.0, SAND_COLD_C.1, c.temperature);
        load += SAND_SUPPLY * dry * cold;
        let trap = smoothstep(SAND_DROP.0, SAND_DROP.1, upwind - c.height)
            * (1.0 - smoothstep(SAND_FLAT.0, SAND_FLAT.1, c.relief))
            * (1.0 - c.mountain);
        let settled = load * SAND_DEPOSIT * trap;
        load -= settled;
        if step >= n {
            rest[i] = settled;
        }
        upwind += (c.height - upwind) * SAND_UPWIND_MIX;
    }
    rest
}

/// Every cell of the climate map's grid, as the weights read it.
fn read_cells(elevation: &Elevation, climate: &ClimateMap, shape: WorldShape) -> Vec<Cell> {
    let (nx, nz) = climate.cells();
    let (cell_x, cell_z) = climate.cell_blocks();
    (0..nx * nz)
        .into_par_iter()
        .map(|c| {
            let (i, j) = (c % nx, c / nx);
            let x = ((i as f64 + 0.5) * cell_x).floor() as i64;
            let z = ((j as f64 + 0.5) * cell_z).floor() as i64;
            let s = elevation.surface(x, z);
            let (sea_level_c, precip, _, land) = climate.cell(i, j);
            let temperature = temperature_at(sea_level_c, s.height.max(0));
            Cell {
                height: s.height as f32,
                relief: s.relief,
                mountain: smoothstep(MOUNTAIN.0, MOUNTAIN.1, s.uplift / MAX_UPLIFT),
                land: s.height > 0 && land >= 0.5,
                temperature,
                precip,
                aridity: precip / aridity_threshold(temperature),
                abs_s: shape.latitude_sine(z).abs() as f32,
            }
        })
        .collect()
}

/// Sand at rest in every cell: each row swept in each wind and blended by
/// the belt's share of each (the climate's own winds), then mixed a little
/// sideways, as the climate's rain is — winds are not single lines.
fn sand_at_rest(cells: &[Cell], nx: usize, nz: usize) -> Vec<f32> {
    let mut sand: Vec<f32> = cells
        .par_chunks(nx)
        .flat_map_iter(|row| {
            let east = eastward_share(row[0].abs_s);
            let (e, w) = (sand_sweep(row, true), sand_sweep(row, false));
            (0..nx).map(move |i| w[i] + (e[i] - w[i]) * east)
        })
        .collect();
    blur(&mut sand, nx, nz);
    sand
}

/// The planet's landform processes on the climate map's grid (stage 1).
pub struct ProcessMap {
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
    /// Per cell, row-major (`j * nx + i`): the five weights in catalogue
    /// order, as bytes — 3 MB at the default size, and a weight needs no
    /// more than 256 steps.
    cells: Vec<[u8; 5]>,
}

/// Its size, not its cells.
impl std::fmt::Debug for ProcessMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProcessMap({}×{} cells of {:.0} blocks)",
            self.nx, self.nz, self.cell_x
        )
    }
}

impl ProcessMap {
    /// Build the map for one world. Deterministic in `(elevation, climate,
    /// shape)`; parallel over cells and rows.
    pub fn build(elevation: &Elevation, climate: &ClimateMap, shape: WorldShape) -> Self {
        let (nx, nz) = climate.cells();
        let (cell_x, cell_z) = climate.cell_blocks();
        let cells = read_cells(elevation, climate, shape);
        let sand = sand_at_rest(&cells, nx, nz);

        let byte = |w: f32| (w.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        let cells = cells
            .par_iter()
            .zip(sand.par_iter())
            .map(|(c, &sand)| {
                let mut w = local_weights(c);
                if c.land {
                    w.wind = smoothstep(SAND_ERG.0, SAND_ERG.1, sand)
                        * smoothstep(DRY_THAWED_C.0, DRY_THAWED_C.1, c.temperature)
                        * (1.0 - w.ice)
                        * (1.0 - smoothstep(WIND_ARID.0, WIND_ARID.1, c.aridity))
                        * (1.0 - smoothstep(WIND_LOWLAND.0, WIND_LOWLAND.1, c.height))
                        * (1.0 - c.mountain);
                }
                w.as_array().map(byte)
            })
            .collect();
        Self {
            nx,
            nz,
            cell_x,
            cell_z,
            cells,
        }
    }

    /// Cells per axis: the climate map's.
    pub fn cells(&self) -> (usize, usize) {
        (self.nx, self.nz)
    }

    /// One cell's weights (for maps and tests).
    pub fn cell(&self, i: usize, j: usize) -> Weights {
        from_bytes(self.cells[j * self.nx + i].map(f32::from))
    }

    /// The weights at a column, which may be on any lap of the world:
    /// bilinear between the four cells around it, so nothing steps at a
    /// cell's edge.
    ///
    /// Smooth is not yet natural — a straight gradient between two cell
    /// centres still follows the grid (M14's lesson). The layers that use
    /// this add the wandering: they sample at a column displaced by the
    /// biome-boundary noise, as [`Self::sample_wandering`] does.
    pub fn sample(&self, x: i64, z: i64) -> Weights {
        let u = x as f64 / self.cell_x - 0.5;
        let v = z as f64 / self.cell_z - 0.5;
        let (fu, fv) = (u.floor(), v.floor());
        let i0 = (fu as i64).rem_euclid(self.nx as i64) as usize;
        let j0 = (fv as i64).rem_euclid(self.nz as i64) as usize;
        let (i1, j1) = ((i0 + 1) % self.nx, (j0 + 1) % self.nz);
        let (tu, tv) = ((u - fu) as f32, (v - fv) as f32);
        let at = |i: usize, j: usize| self.cells[j * self.nx + i];
        let (c00, c10, c01, c11) = (at(i0, j0), at(i1, j0), at(i0, j1), at(i1, j1));
        let mut out = [0.0f32; 5];
        for (k, o) in out.iter_mut().enumerate() {
            let top = f32::from(c00[k]) + (f32::from(c10[k]) - f32::from(c00[k])) * tu;
            let bottom = f32::from(c01[k]) + (f32::from(c11[k]) - f32::from(c01[k])) * tu;
            *o = top + (bottom - top) * tv;
        }
        from_bytes(out)
    }

    /// [`Self::sample`] at the column displaced by `noise` (each component
    /// in `[-1, 1]`: `Elevation::boundary_noise`) by up to
    /// [`WANDER_CELLS`] of a cell, so a process's edge wanders as a biome's
    /// does instead of running straight between cell centres.
    pub fn sample_wandering(&self, x: i64, z: i64, noise: (f32, f32)) -> Weights {
        let dx = (f64::from(noise.0) * WANDER_CELLS * self.cell_x).floor() as i64;
        let dz = (f64::from(noise.1) * WANDER_CELLS * self.cell_z).floor() as i64;
        self.sample(x + dx, z + dz)
    }
}

/// How far a process's edge may wander from the grid's, in cells.
pub const WANDER_CELLS: f64 = 0.45;

fn from_bytes(b: [f32; 5]) -> Weights {
    const S: f32 = 1.0 / 255.0;
    Weights {
        wind: b[0] * S,
        flood: b[1] * S,
        ice: b[2] * S,
        weathering: b[3] * S,
        rain: b[4] * S,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Generator;

    /// What a survey counts in one world, as land cells.
    #[derive(Default, Debug, Clone, Copy)]
    pub(super) struct Shares {
        land: f64,
        hot_desert: f64,
        hot_desert_dunes: f64,
        cold_desert: f64,
        cold_desert_dunes: f64,
        dry: f64,
        dry_flood: f64,
        savanna: f64,
        savanna_weathering: f64,
        humid: f64,
        humid_rain: f64,
        /// Land and glaciated land by latitude band: 0–20°, 20–40°, 40–50°,
        /// 50–90°.
        band: [f64; 4],
        band_ice: [f64; 4],
        /// Dune cells that are not desert, flood cells that are not dry, and
        /// so on: a process outside its climate. Must stay zero.
        misplaced: f64,
    }

    /// Count a world's process weights against the climates the catalogue
    /// puts them in (`docs/landforms.md`). "Has" a process: weight ≥ 0.5.
    pub(super) fn survey(g: &Generator) -> Shares {
        let (climate, map, shape) = (g.climate(), g.process(), g.shape());
        let (nx, nz) = map.cells();
        let (cell_x, cell_z) = climate.cell_blocks();
        let mut s = Shares::default();
        for j in 0..nz {
            let z = ((j as f64 + 0.5) * cell_z).floor() as i64;
            let lat = shape.latitude_degrees(z).abs();
            let band = [20.0, 40.0, 50.0].iter().filter(|&&b| lat >= b).count();
            for i in 0..nx {
                let x = ((i as f64 + 0.5) * cell_x).floor() as i64;
                let surface = g.elevation().surface(x, z);
                let (sea_c, precip, _, land) = climate.cell(i, j);
                if surface.height <= 0 || land < 0.5 {
                    continue;
                }
                let t = temperature_at(sea_c, surface.height);
                let a = precip / aridity_threshold(t);
                let w = map.cell(i, j);
                let has = |v: f32| f64::from(u8::from(v >= 0.5));
                s.land += 1.0;
                s.band[band] += 1.0;
                s.band_ice[band] += has(w.ice);
                if a < 0.5 && t >= 18.0 {
                    s.hot_desert += 1.0;
                    s.hot_desert_dunes += has(w.wind);
                } else if a < 0.5 && t >= -5.0 {
                    s.cold_desert += 1.0;
                    s.cold_desert_dunes += has(w.wind);
                }
                if a < 1.0 {
                    s.dry += 1.0;
                    s.dry_flood += has(w.flood);
                } else {
                    s.humid += 1.0;
                    s.humid_rain += has(w.rain);
                }
                if t >= 20.0 && a >= 1.0 && precip < 2_000.0 {
                    s.savanna += 1.0;
                    s.savanna_weathering += has(w.weathering);
                }
                // Each process only in its own climate.
                s.misplaced += f64::from(u8::from(
                    (w.wind >= 0.5 && a >= 0.6)
                        || (w.flood >= 0.5 && a >= 0.9)
                        || (w.rain >= 0.5 && a < 0.8)
                        || (w.weathering >= 0.5 && (t < 18.0 || a < 0.5)),
                ));
            }
        }
        s
    }

    /// Tuning aid: how much sand rests in desert cells, as percentiles.
    #[test]
    #[ignore]
    fn sand_percentiles() {
        for seed in 0..3u64 {
            let g = Generator::new(
                seed.wrapping_mul(0x9E37_79B9) ^ 0x7e22ac0de,
                WorldShape::DEFAULT,
            );
            let cells = read_cells(g.elevation(), g.climate(), g.shape());
            let (nx, nz) = g.climate().cells();
            let sand = sand_at_rest(&cells, nx, nz);
            for (name, hot) in [("hot", true), ("cold", false)] {
                let mut v: Vec<f32> = cells
                    .iter()
                    .zip(&sand)
                    .filter(|(c, _)| {
                        c.land
                            && c.aridity < 0.5
                            && (c.temperature >= 18.0) == hot
                            && c.temperature >= -5.0
                    })
                    .map(|(_, &s)| s)
                    .collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let q = |p: f64| {
                    v.get(((v.len() as f64 - 1.0) * p) as usize)
                        .copied()
                        .unwrap_or(0.0)
                };
                println!(
                    "seed {seed} {name} desert ({} cells): p50 {:.2} p70 {:.2} p80 {:.2} p90 {:.2} p95 {:.2} max {:.2}",
                    v.len(),
                    q(0.5),
                    q(0.7),
                    q(0.8),
                    q(0.9),
                    q(0.95),
                    q(1.0)
                );
            }
        }
    }

    // ---- The rules, one at a time ----

    fn cell(height: f32, relief: f32, temperature: f32, precip: f32, lat_deg: f32) -> Cell {
        // sin by table: tests need only a few latitudes, exactly.
        let abs_s = match lat_deg as i32 {
            0 => 0.0,
            15 => 0.2588,
            25 => 0.4226,
            45 => std::f32::consts::FRAC_1_SQRT_2,
            60 => 0.8660,
            80 => 0.9848,
            other => panic!("no sine for {other} degrees"),
        };
        Cell {
            height,
            relief,
            mountain: 0.0,
            land: true,
            temperature,
            precip,
            aridity: precip / aridity_threshold(temperature),
            abs_s,
        }
    }

    /// The catalogue's ice-age table (`docs/landforms.md`): the height above
    /// which ground was glaciated, by latitude — from the latitude curve,
    /// the lapse rate and [`ice_age_cooling`], with snow to spare.
    #[test]
    fn the_ice_age_reaches_where_the_catalogue_says() {
        use crate::climate::{LAPSE_PER_BLOCK, sea_level_temperature_at_latitude};
        // (sin latitude, the catalogue's "ice-age ice above", blocks).
        let table = [
            (0.0, 720.0),
            (0.3420, 600.0),
            (0.5, 410.0),
            (0.6428, 165.0),
            (std::f32::consts::FRAC_1_SQRT_2, 50.0),
        ];
        for (s, want) in table {
            let ice_age_sea = sea_level_temperature_at_latitude(s) - ice_age_cooling(s);
            let line = ice_age_sea / LAPSE_PER_BLOCK; // where the ice-age ground is 0 °C
            assert!(
                (line - want).abs() < 30.0,
                "sin latitude {s}: ice from {line:.0} blocks, catalogue says ~{want}"
            );
            let at = |h: f32| ice_weight(ice_age_sea - LAPSE_PER_BLOCK * h, 1_000.0);
            assert!(
                at(line + 80.0) > 0.9 && at(line - 80.0) < 0.1,
                "a margin, not a ramp"
            );
        }
        // From 50 degrees poleward: sea level.
        for s in [0.7660f32, 0.8660, 0.9848] {
            let t = sea_level_temperature_at_latitude(s) - ice_age_cooling(s);
            assert!(
                ice_weight(t, 1_000.0) > 0.5,
                "sin latitude {s}: {t:.1} C at sea level"
            );
        }
        assert_eq!(ice_age_cooling(0.0), 4.0);
        assert_eq!(ice_age_cooling(1.0), 11.0);
    }

    /// Ice needs cold AND snow — unless nothing ever melts.
    #[test]
    fn ice_needs_snow_unless_nothing_melts() {
        assert!(
            ice_weight(-6.0, 800.0) > 0.99,
            "cold and snowy: Scandinavia"
        );
        assert!(
            ice_weight(-6.0, 100.0) < 0.01,
            "cold and dry: ice-age Siberia"
        );
        assert!(
            ice_weight(-25.0, 40.0) > 0.99,
            "never thaws: Antarctica's interior"
        );
        assert!(ice_weight(6.0, 3_000.0) < 0.01, "wet but warm");
        assert!(
            (ice_weight(0.0, 800.0) - 0.5).abs() < 1e-6,
            "the margin is at 0 C"
        );
    }

    /// Each process in its own climate and landscape, and not in the others'.
    #[test]
    fn each_weight_follows_its_own_climate() {
        // A warm, rough, dry upland: the Colorado Plateau.
        let plateau = local_weights(&cell(180.0, 0.5, 16.0, 120.0, 25.0));
        assert!(plateau.flood > 0.9, "{plateau:?}");
        assert_eq!(
            (plateau.rain, plateau.weathering, plateau.ice),
            (0.0, 0.0, 0.0)
        );
        // The same ground flat still has benches; in a lowland basin, none.
        assert!(local_weights(&cell(180.0, 0.05, 16.0, 120.0, 25.0)).flood > 0.5);
        assert_eq!(
            local_weights(&cell(40.0, 0.5, 16.0, 120.0, 25.0)).flood,
            0.0
        );
        // A polar desert is as dry and has no rainstorms.
        let polar = local_weights(&cell(180.0, 0.5, -20.0, 60.0, 80.0));
        assert_eq!(polar.flood, 0.0);
        assert!(polar.ice > 0.99);

        // A hot plain, neither desert nor rainforest: the Serengeti.
        let savanna = local_weights(&cell(60.0, 0.05, 25.0, 900.0, 15.0));
        assert!(savanna.weathering > 0.9, "{savanna:?}");
        assert_eq!((savanna.flood, savanna.ice), (0.0, 0.0));
        // Not in the rainforest's wettest, not on hills, not in the cold.
        assert_eq!(
            local_weights(&cell(60.0, 0.05, 25.0, 3_000.0, 0.0)).weathering,
            0.0
        );
        assert_eq!(
            local_weights(&cell(60.0, 0.5, 25.0, 900.0, 15.0)).weathering,
            0.0
        );
        assert_eq!(
            local_weights(&cell(60.0, 0.05, 10.0, 600.0, 45.0)).weathering,
            0.0
        );

        // Rain rises with humidity and is nothing in a desert.
        let rain = |p: f32| local_weights(&cell(120.0, 0.3, 12.0, p, 45.0)).rain;
        assert_eq!(rain(150.0), 0.0);
        assert!(rain(600.0) < rain(900.0) && rain(900.0) < rain(1_200.0));
        assert!(rain(1_400.0) > 0.99);

        // Mountains keep only their ice; the sea floor only the ice age's.
        let mut peak = cell(900.0, 0.8, 16.0, 120.0, 25.0);
        peak.mountain = 1.0;
        let w = local_weights(&peak);
        assert_eq!((w.flood, w.weathering, w.rain), (0.0, 0.0, 0.0));
        let mut sea = cell(-50.0, 0.1, -8.0, 900.0, 60.0);
        sea.land = false;
        let w = local_weights(&sea);
        assert_eq!(
            (w.flood, w.weathering, w.rain, w.wind),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert!(w.ice > 0.9, "a fjord can be cut where ice met the sea");
    }

    /// Sand comes to rest in a flat basin below the ground upwind of it —
    /// downwind of its source, not on the upland, not on rough ground, and
    /// on whichever side the wind puts it.
    #[test]
    fn sand_settles_in_basins_downwind_of_its_source() {
        let desert = |height: f32, relief: f32| cell(height, relief, 24.0, 40.0, 25.0);
        let mut row = vec![desert(120.0, 0.1); 60];
        // An upland for 30 cells, then a basin for 20, then a rough range.
        for c in &mut row[30..50] {
            *c = desert(50.0, 0.05);
        }
        for c in &mut row[50..60] {
            *c = desert(200.0, 0.6);
        }
        let east = sand_sweep(&row, true);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (upland, basin, range) = (mean(&east[5..30]), mean(&east[31..50]), mean(&east[50..60]));
        assert!(basin > 4.0 * upland, "basin {basin}, upland {upland}");
        assert!(basin > 1.0, "enough for a dune sea: {basin}");
        assert_eq!(range, 0.0, "rough ground holds none");
        // The basin's upwind edge takes the most: the load is spent crossing it.
        assert!(east[31] > east[48], "{} then {}", east[31], east[48]);

        // The other wind reaches the basin over the range and finds it just
        // as low: sand again, but spent from the other edge.
        let west = sand_sweep(&row, false);
        assert!(mean(&west[31..50]) > 1.0);
        assert!(west[48] > west[31]);

        // Wet ground yields nothing and holds what arrives; the sea takes all.
        let mut green = row.clone();
        for c in &mut green[..30] {
            *c = cell(120.0, 0.1, 24.0, 2_000.0, 25.0);
        }
        for c in &mut green[50..] {
            c.land = false;
        }
        let east = sand_sweep(&green, true);
        assert_eq!(mean(&east[..30]), 0.0);
        assert!(
            mean(&east[31..50]) < 0.5 * basin,
            "only the basin's own sand"
        );
    }

    // ---- The map ----

    /// A small world, for the tests that need a whole map.
    fn small() -> Generator {
        Generator::new(0x15, WorldShape::new(32_768, 32_768).unwrap())
    }

    /// Same world, same map, byte for byte; and the generator's is the one
    /// `build` makes.
    #[test]
    fn the_map_is_deterministic() {
        let g = small();
        let again = ProcessMap::build(g.elevation(), g.climate(), g.shape());
        assert_eq!(g.process().cells, again.cells);
        assert_eq!(g.process().cells(), g.climate().cells());
        assert!(g.process().cells.iter().any(|c| c.iter().any(|&w| w > 0)));
    }

    /// Sampling is the cell at a cell's centre, never outside the range of
    /// the cells around a column, the same on every lap of the world, and
    /// bounded when it wanders.
    #[test]
    fn sampling_is_smooth_and_wraps() {
        let g = small();
        let map = g.process();
        let (nx, nz) = map.cells();
        let (size_x, size_z) = (g.shape().size_x(), g.shape().size_z());
        let mut seen = 0;
        for (i, j) in [
            (0, 0),
            (nx - 1, nz - 1),
            (nx / 2, nz / 3),
            (17, 90),
            (100, 5),
        ] {
            let x = ((i as f64 + 0.5) * map.cell_x).floor() as i64;
            let z = ((j as f64 + 0.5) * map.cell_z).floor() as i64;
            let (here, cell) = (map.sample(x, z).as_array(), map.cell(i, j).as_array());
            for k in 0..5 {
                assert!(
                    (here[k] - cell[k]).abs() < 0.02,
                    "cell ({i}, {j}) weight {k}"
                );
            }
            assert_eq!(map.sample(x + 3 * size_x, z - 2 * size_z), map.sample(x, z));
            // Anywhere within the cell, and wandering: between the extremes
            // of the 3x3 cells around it.
            let around: Vec<[f32; 5]> = (-1..=1i64)
                .flat_map(|dj| (-1..=1i64).map(move |di| (di, dj)))
                .map(|(di, dj)| {
                    let wrap = |v: i64, n: usize| (v.rem_euclid(n as i64)) as usize;
                    map.cell(wrap(i as i64 + di, nx), wrap(j as i64 + dj, nz))
                        .as_array()
                })
                .collect();
            for (ox, oz, noise) in [
                (40, -60, (0.0, 0.0)),
                (-90, 20, (1.0, -1.0)),
                (0, 0, (-1.0, 1.0)),
            ] {
                let w = map.sample_wandering(x + ox, z + oz, noise).as_array();
                for k in 0..5 {
                    let lo = around.iter().map(|a| a[k]).fold(f32::MAX, f32::min);
                    let hi = around.iter().map(|a| a[k]).fold(f32::MIN, f32::max);
                    assert!(
                        w[k] >= lo - 0.01 && w[k] <= hi + 0.01,
                        "weight {k} left its cells"
                    );
                    seen += usize::from(w[k] > 0.0);
                }
            }
        }
        assert!(seen > 0);
    }

    /// Over whole worlds: no process outside its climate, anywhere; dune
    /// seas over the catalogue's share of the deserts (about a fifth to a
    /// third of hot desert, about a tenth of cold); the ice age reaching
    /// further toward the equator band by band.
    #[test]
    fn processes_fall_where_the_catalogue_puts_them() {
        let mut all = Shares::default();
        for seed in [0x7e22ac0de, 0x15] {
            let s = survey(&Generator::new(seed, WorldShape::DEFAULT));
            assert_eq!(
                s.misplaced, 0.0,
                "seed {seed:#x}: a process outside its climate"
            );
            for (sum, v) in [
                (&mut all.hot_desert, s.hot_desert),
                (&mut all.hot_desert_dunes, s.hot_desert_dunes),
                (&mut all.cold_desert, s.cold_desert),
                (&mut all.cold_desert_dunes, s.cold_desert_dunes),
                (&mut all.dry, s.dry),
                (&mut all.dry_flood, s.dry_flood),
                (&mut all.savanna, s.savanna),
                (&mut all.savanna_weathering, s.savanna_weathering),
                (&mut all.humid, s.humid),
                (&mut all.humid_rain, s.humid_rain),
            ] {
                *sum += v;
            }
            for b in 0..4 {
                all.band[b] += s.band[b];
                all.band_ice[b] += s.band_ice[b];
            }
        }
        let hot = pct(all.hot_desert_dunes, all.hot_desert);
        let cold = pct(all.cold_desert_dunes, all.cold_desert);
        assert!(
            (15.0..=35.0).contains(&hot),
            "dune seas on {hot:.1}% of hot desert"
        );
        assert!(
            (5.0..=20.0).contains(&cold),
            "dune seas on {cold:.1}% of cold desert"
        );
        assert!(cold < hot);
        assert!(pct(all.dry_flood, all.dry) > 1.0, "dry uplands exist");
        assert!(
            pct(all.savanna_weathering, all.savanna) > 10.0,
            "weathered plains exist"
        );
        assert!(pct(all.humid_rain, all.humid) > 30.0);
        let ice: Vec<f64> = (0..4).map(|b| pct(all.band_ice[b], all.band[b])).collect();
        assert!(
            ice[0] < 12.0,
            "tropics: only the highest ground, got {:.0}%",
            ice[0]
        );
        assert!(
            ice[0] < ice[1] && ice[1] < ice[2] && ice[2] <= ice[3],
            "{ice:?}"
        );
        assert!(
            ice[3] > 95.0,
            "beyond 50 degrees the lowlands were under ice"
        );
    }

    fn pct(part: f64, whole: f64) -> f64 {
        if whole > 0.0 {
            100.0 * part / whole
        } else {
            0.0
        }
    }

    /// `cargo test -p vox-worldgen --release process_survey -- --ignored
    /// --nocapture`: each process's share of the climate it belongs to, over
    /// `SURVEY_SEEDS` seeds (default 6) at `SURVEY_SIZE` blocks (default the
    /// default world) — the numbers the constants above are tuned to.
    #[test]
    #[ignore]
    fn process_survey() {
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<i64>().ok());
        let seeds = env("SURVEY_SEEDS").unwrap_or(6) as u64;
        let size = env("SURVEY_SIZE").unwrap_or(WorldShape::DEFAULT.size_x());
        let shape = WorldShape::new(size, size).expect("a legal size");
        println!(
            "seed | land | dunes: hot desert, cold desert | flood: dry land | \
             weathering: savanna | rain: humid | ice by latitude 0-20 20-40 40-50 50-90 | misplaced"
        );
        for seed in 0..seeds {
            let g = Generator::new(seed.wrapping_mul(0x9E37_79B9) ^ 0x7e22ac0de, shape);
            let s = survey(&g);
            println!(
                "{seed:>4} | {:>6.0} | {:>4.1}% of {:>4.1}%, {:>4.1}% of {:>4.1}% | {:>4.1}% of {:>4.1}% | \
                 {:>4.1}% of {:>4.1}% | {:>4.1}% of {:>4.1}% | {:>3.0}% {:>3.0}% {:>3.0}% {:>3.0}% | {}",
                s.land,
                pct(s.hot_desert_dunes, s.hot_desert),
                pct(s.hot_desert, s.land),
                pct(s.cold_desert_dunes, s.cold_desert),
                pct(s.cold_desert, s.land),
                pct(s.dry_flood, s.dry),
                pct(s.dry, s.land),
                pct(s.savanna_weathering, s.savanna),
                pct(s.savanna, s.land),
                pct(s.humid_rain, s.humid),
                pct(s.humid, s.land),
                pct(s.band_ice[0], s.band[0]),
                pct(s.band_ice[1], s.band[1]),
                pct(s.band_ice[2], s.band[2]),
                pct(s.band_ice[3], s.band[3]),
                s.misplaced,
            );
        }
    }

    /// `cargo test -p vox-worldgen --release process_map -- --ignored
    /// --nocapture`: the whole planet's process weights as one picture
    /// (`WORLD_MAP_SEED`, `WORLD_MAP_SIZE`, `WORLD_MAP_PX`, `WORLD_MAP_DIR` as
    /// for `world_map`). Land is grey, darker with height; each process
    /// tints it by its weight — wind gold, flash flood red, ice pale blue,
    /// weathering violet, rain green.
    #[test]
    #[ignore]
    fn process_map() {
        const TINT: [[f32; 3]; 5] = [
            [236.0, 196.0, 64.0],
            [204.0, 84.0, 52.0],
            [170.0, 222.0, 246.0],
            [150.0, 92.0, 176.0],
            [70.0, 150.0, 84.0],
        ];
        let (g, px, dir) = crate::map::tests::map_world();
        let shape = g.shape();
        let step = shape.size_x() as f64 / f64::from(px);
        let map = g.process();
        let mut rgb = vec![0u8; (px * px * 3) as usize];
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let h = g.elevation().height(x, z);
                let w = map.sample_wandering(x, z, g.elevation().boundary_noise(x, z));
                let colour = if h <= 0 {
                    // The sea, with the ice age's reach over it faintly.
                    [
                        18.0 + 30.0 * w.ice,
                        36.0 + 40.0 * w.ice,
                        70.0 + 50.0 * w.ice,
                    ]
                } else {
                    let grey = 205.0 - 110.0 * (h as f32 / 900.0).min(1.0);
                    let weights = w.as_array();
                    let strongest = weights.iter().fold(0.0f32, |m, &v| m.max(v));
                    let total: f32 = weights.iter().sum::<f32>().max(1e-6);
                    let mut c = [0.0f32; 3];
                    for (k, tint) in TINT.iter().enumerate() {
                        for ch in 0..3 {
                            c[ch] += tint[ch] * weights[k] / total;
                        }
                    }
                    [0, 1, 2].map(|ch| grey + (c[ch] - grey) * strongest)
                };
                let at = ((j * px + i) * 3) as usize;
                rgb[at..at + 3].copy_from_slice(&colour.map(|v| v.clamp(0.0, 255.0) as u8));
            }
        }
        let path = dir.join(format!("process-{:x}-{}.png", g.seed(), shape.size_x()));
        crate::map::write_png(&path, px, px, &rgb).unwrap();
        println!("wrote {}", path.display());
    }
}
