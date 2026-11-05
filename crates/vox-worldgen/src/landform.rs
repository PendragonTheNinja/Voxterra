//! Landform processes (M15 task 1, ADR-0021): where each of the five
//! processes that shape Earth's land acts, as a weight in `[0, 1]` per
//! column.
//!
//! `docs/landforms.md` is the catalogue these follow. A landform is made by
//! its process, never scattered: dune seas where wind works on dry, low,
//! sandy ground; benches, mesas and canyons where flash floods cut dry
//! uplands; U-valleys and fjords where ice lay in the last ice age;
//! inselbergs where deep tropical weathering was stripped off old plains;
//! rounded, dissected hills where rain is plentiful. The landform layers
//! (later tasks) read these weights; until they exist the weights are an
//! instrument — the world map's process layer — and change no terrain.
//!
//! ## Inputs: the base ground, never the landformed one
//!
//! Weights read the elevation field's own surface (height, relief, uplift),
//! the stage-1 climate, the latitude, and two stage-1 fields of their own
//! ([`ProcessMap`]: how far a place lies below the ground upwind, and how
//! much sand the wind brings). Never the ground the landforms make, nor the
//! drainage network's erosion, which is built on that ground — a mesa must
//! not raise its own flash-flood weight (ADR-0021's order).
//!
//! ## Smooth and noisy, never a coarse grid
//!
//! Every input is continuous in `(x, z)`: the climate and process maps are
//! bilinear between cell centres, the surface is the elevation field's, and
//! temperature and rain carry M13's boundary noise (the same jitter the
//! biomes use, so a process edge frays as a biome edge does). Every
//! threshold is a smoothstep. M14's lesson: anything read straight off a
//! coarse grid shows through as squares.
//!
//! ## Deterministic on every machine
//!
//! Basic arithmetic, `floor` and [`crate::climate::exp`] only, as in the
//! climate map.

mod deposits;
mod dunes;
mod inselbergs;
mod mesas;

use rayon::prelude::*;
use vox_core::WorldShape;

use crate::biome::{self, ICE_SHEET_BELOW_C, aridity_threshold};
use crate::climate::{
    ClimateMap, blur, eastward_share, exp, lerp4, piecewise, smoothstep, temperature_at,
};
use crate::elevation::{Elevation, Surface};

pub use deposits::{Deposit, Deposits};
pub use dunes::{Dune, DuneKind, Dunes};
pub use inselbergs::{Inselbergs, Rock};
pub use mesas::{MesaPart, Mesas};

// --- The ice-age climate ----------------------------------------------------

/// Ice-age cooling, °C: [`ICE_AGE_COOLING_LOW_C`] up to 20° of latitude,
/// rising smoothly to [`ICE_AGE_COOLING_HIGH_C`] at 50° and beyond. Earth
/// ~20 000 years ago was ~4 °C colder in the tropics and 10–15 °C colder at
/// high latitudes (`docs/landforms.md`, *The ice-age climate*).
const ICE_AGE_COOLING_LOW_C: f32 = 4.0;
const ICE_AGE_COOLING_HIGH_C: f32 = 11.0;
/// sin 20° and sin 50°: the ramp's ends.
const ICE_AGE_RAMP: (f32, f32) = (0.342, 0.766);
/// Ice-age mean annual temperature at which ground is fully glaciated, and
/// above which it was never under ice, °C, at [`SNOW_REFERENCE_MM`] of
/// precipitation. Glaciers flow below their snowline, so ice lay down to
/// about 0 °C; the weight is 0.5 near +0.4.
const GLACIATED_FULL_C: f32 = -1.0;
const GLACIATED_NONE_C: f32 = 2.0;
/// Snow decides where ice lies as much as cold does: a glacier's
/// equilibrium line sits at warmer temperatures where more snow falls
/// (Ohmura et al. 1992: the summer temperature at the line rises with
/// annual precipitation), so wet ranges glaciate lower and dry ones higher.
/// The shift of the thresholds above, °C, by today's precipitation, mm;
/// none at the reference, where the catalogue's table holds.
const SNOW_REFERENCE_MM: f32 = 1_000.0;
const SNOW_SHIFT_C: [(f32, f32); 6] = [
    (100.0, -8.0),
    (250.0, -5.0),
    (500.0, -2.0),
    (SNOW_REFERENCE_MM, 0.0),
    (2_000.0, 1.5),
    (4_000.0, 2.5),
];
/// And where almost no snow falls, no ice sheet grows however cold it is:
/// Siberia and Beringia stayed ice-free through the last ice age. Ice's
/// weight fades out below these totals of today's precipitation, mm.
const SNOW_STARVED_MM: (f32, f32) = (200.0, 450.0);
/// Where ice lies today (M13's ice sheet, below [`ICE_SHEET_BELOW_C`] at
/// the column's own height) it lay in the ice age too, however little snow
/// falls: Antarctica and Greenland are polar deserts under ice. The weight
/// is full this far below the ice sheet's threshold, °C, and none this far
/// above it.
const ICE_SHEET_BLEND_C: f32 = 2.0;

/// How much colder the last ice age was than today, °C, by
/// `abs_s = |sin(latitude)|`.
pub fn ice_age_cooling(abs_s: f32) -> f32 {
    ICE_AGE_COOLING_LOW_C
        + (ICE_AGE_COOLING_HIGH_C - ICE_AGE_COOLING_LOW_C)
            * smoothstep(ICE_AGE_RAMP.0, ICE_AGE_RAMP.1, abs_s)
}

/// Ice-age mean annual temperature of a column, °C, from today's sea-level
/// temperature there.
pub fn ice_age_temperature(sea_level_c: f32, abs_s: f32, height: i64) -> f32 {
    temperature_at(sea_level_c - ice_age_cooling(abs_s), height)
}

// --- The weights' thresholds (tuning targets, docs/landforms.md) -------------

/// Mountains: uplift (blocks of height raised by mountain building) over
/// which no process but ice shapes the ground. Real ranges, not any
/// uplift: almost all high ground here is raised by orogeny, as Earth's
/// dry plateaus are (Colorado, Iran, Anatolia), and those are flash-flood
/// country. A range in this generator is 400 blocks of uplift and more.
const MOUNTAIN_UPLIFT: (f32, f32) = (200.0, 400.0);

/// Wind: aridity (precipitation over Köppen's threshold) at which it is full
/// and at which it stops; relief at which ground is too rough for a dune
/// sea; height over which a plateau is too high.
const WIND_ARIDITY: (f32, f32) = (0.3, 0.6);
const WIND_RELIEF: (f32, f32) = (0.25, 0.5);
const WIND_HEIGHT: (f32, f32) = (100.0, 150.0);
/// The share of wind's weight that needs no sand supply or basin: dry, flat,
/// low ground anywhere has some, a basin downwind of sand has all.
const WIND_UNSUPPLIED: f32 = 0.35;

/// Flash flood: aridity at which it is full and at which it stops; the
/// height over which an upland begins; relief at which badlands make it
/// strongest, and the share it has on smooth uplands.
const FLOOD_ARIDITY: (f32, f32) = (0.6, 0.9);
const FLOOD_HEIGHT: (f32, f32) = (80.0, 140.0);
/// The height at or below which flash floods never act (the generator's
/// first, cheapest test).
pub const FLOOD_FROM_HEIGHT: i64 = FLOOD_HEIGHT.0 as i64;
const FLOOD_RELIEF: (f32, f32) = (0.2, 0.45);
const FLOOD_SMOOTH_SHARE: f32 = 0.5;

/// Weathering: tropical temperatures, °C; the aridity band of savanna and
/// dry forest (rising, then falling); relief over which plains are too
/// rough.
const WEATHERING_TEMPERATURE_C: (f32, f32) = (18.0, 21.0);
/// The temperature (°C) below which deep weathering never acts.
pub const WEATHERING_FROM_C: f32 = WEATHERING_TEMPERATURE_C.0;
const WEATHERING_ARIDITY_RISE: (f32, f32) = (0.5, 0.7);
const WEATHERING_ARIDITY_FALL: (f32, f32) = (1.8, 2.6);
const WEATHERING_RELIEF: (f32, f32) = (0.3, 0.55);

/// Rain: aridity at which it starts and at which it is full.
const RAIN_ARIDITY: (f32, f32) = (0.8, 3.0);

// --- Stage 1: basins and sand -----------------------------------------------

/// Distance upwind over which "the ground upwind" is averaged, blocks, at
/// the default world's continent size: a basin is low against the plateau
/// or range it lies in, tens of kilometres across.
const BASIN_UPWIND_LENGTH: f32 = 6_000.0;
/// How far below the ground upwind a place must lie to be a full basin,
/// blocks.
const BASIN_DEPTH: f32 = 60.0;
/// Distance downwind over which sand from dry lowland carries, blocks:
/// dune seas lie downwind of their source, a few tens of kilometres here.
const SAND_CARRY_LENGTH: f32 = 10_000.0;
/// Sand source: dry lowland, by aridity (full source below the first, none
/// above the second) and height (blocks).
const SAND_ARIDITY: (f32, f32) = (0.3, 0.6);
const SAND_HEIGHT: (f32, f32) = (100.0, 200.0);

/// The five processes' weights at one column, each in `[0, 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Processes {
    /// Wind: dune seas.
    pub wind: f32,
    /// Flash floods: benches, mesas, buttes, canyons.
    pub flood: f32,
    /// Ice, in the last ice age or now: U-valleys, cirques, fjords.
    pub ice: f32,
    /// Deep weathering: inselbergs.
    pub weathering: f32,
    /// Rain: rounded, dissected hills.
    pub rain: f32,
}

impl Processes {
    /// The processes in catalogue order, with their names (maps, surveys).
    pub fn named(&self) -> [(&'static str, f32); 5] {
        [
            ("wind", self.wind),
            ("flash flood", self.flood),
            ("ice", self.ice),
            ("weathering", self.weathering),
            ("rain", self.rain),
        ]
    }
}

/// Everything a column's weights are made from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    /// The elevation field's own surface (not eroded, not landformed).
    pub surface: Surface,
    /// Today's sea-level temperature, °C, boundary noise included.
    pub sea_level_c: f32,
    /// Annual precipitation, mm, boundary noise included.
    pub precipitation: f32,
    /// `|sin(latitude)|`.
    pub abs_s: f32,
    /// [`ProcessMap`]: how far below the ground upwind, `[0, 1]`.
    pub basin: f32,
    /// [`ProcessMap`]: sand the wind brings, `[0, 1]`.
    pub sand: f32,
}

/// Ice's weight from a column's ice-age temperature, °C, and today's
/// precipitation, mm: cold enough for the snow that falls there, and
/// enough snow at all.
pub fn glaciation(ice_age_c: f32, precip: f32) -> f32 {
    let shift = piecewise(&SNOW_SHIFT_C, precip);
    (1.0 - smoothstep(
        GLACIATED_FULL_C + shift,
        GLACIATED_NONE_C + shift,
        ice_age_c,
    )) * smoothstep(SNOW_STARVED_MM.0, SNOW_STARVED_MM.1, precip)
}

/// Whether wind can have any weight at a column, from its sea-level
/// temperature and precipitation before boundary noise and its base height:
/// a cheap, conservative test (it allows the most the noise could make
/// dry), so the dune layer skips the full weights almost everywhere.
pub fn wind_possible(sea_level_c: f32, precip: f32, base_height: i64) -> bool {
    if base_height <= 0 || base_height as f32 >= WIND_HEIGHT.1 {
        return false;
    }
    let warmest = temperature_at(sea_level_c + biome::TEMPERATURE_JITTER_C, base_height);
    let driest = precip * (1.0 - biome::PRECIPITATION_JITTER);
    driest / aridity_threshold(warmest) < WIND_ARIDITY.1
}

/// Whether flash floods can have any weight at a column: as
/// [`wind_possible`], cheap and conservative.
pub fn flood_possible(sea_level_c: f32, precip: f32, base_height: i64) -> bool {
    if base_height as f32 <= FLOOD_HEIGHT.0 {
        return false;
    }
    let warmest = temperature_at(sea_level_c + biome::TEMPERATURE_JITTER_C, base_height);
    let driest = precip * (1.0 - biome::PRECIPITATION_JITTER);
    driest / aridity_threshold(warmest) < FLOOD_ARIDITY.1
}

/// Whether ice can have any weight at a column, from today's sea-level
/// temperature before boundary noise, `|sin(latitude)|` and its base
/// height: cheap and conservative (the warmest the noise could make it,
/// the wettest snow's shift).
pub fn ice_possible(sea_level_c: f32, abs_s: f32, base_height: i64) -> bool {
    let warmest = ice_age_temperature(
        sea_level_c + biome::TEMPERATURE_JITTER_C,
        abs_s,
        base_height,
    );
    warmest < GLACIATED_NONE_C + SNOW_SHIFT_C[SNOW_SHIFT_C.len() - 1].1
}

/// The five weights from a column's inputs. Pure; `docs/landforms.md`'s
/// table, *The process weights*, row by row.
pub fn weights(i: &Inputs) -> Processes {
    let s = i.surface;
    let h = s.height;
    let t = temperature_at(i.sea_level_c, h);
    let aridity = i.precipitation / aridity_threshold(t);
    let land = if h > 0 { 1.0 } else { 0.0 };
    let not_mountain = 1.0 - smoothstep(MOUNTAIN_UPLIFT.0, MOUNTAIN_UPLIFT.1, s.uplift);
    let hf = h as f32;

    let ice_now = 1.0
        - smoothstep(
            ICE_SHEET_BELOW_C - ICE_SHEET_BLEND_C,
            ICE_SHEET_BELOW_C + ICE_SHEET_BLEND_C,
            t,
        );
    let ice = glaciation(
        ice_age_temperature(i.sea_level_c, i.abs_s, h),
        i.precipitation,
    )
    .max(ice_now);
    // Under today's ice sheet only ice shapes the ground.
    let land = land * (1.0 - ice_now);

    let wind = land
        * (1.0 - smoothstep(WIND_ARIDITY.0, WIND_ARIDITY.1, aridity))
        * (1.0 - smoothstep(WIND_RELIEF.0, WIND_RELIEF.1, s.relief))
        * (1.0 - smoothstep(WIND_HEIGHT.0, WIND_HEIGHT.1, hf))
        * not_mountain
        * (WIND_UNSUPPLIED + (1.0 - WIND_UNSUPPLIED) * i.basin.max(i.sand));

    let flood = land
        * (1.0 - smoothstep(FLOOD_ARIDITY.0, FLOOD_ARIDITY.1, aridity))
        * smoothstep(FLOOD_HEIGHT.0, FLOOD_HEIGHT.1, hf)
        * not_mountain
        * (FLOOD_SMOOTH_SHARE
            + (1.0 - FLOOD_SMOOTH_SHARE) * smoothstep(FLOOD_RELIEF.0, FLOOD_RELIEF.1, s.relief));

    let weathering = land
        * smoothstep(WEATHERING_TEMPERATURE_C.0, WEATHERING_TEMPERATURE_C.1, t)
        * smoothstep(
            WEATHERING_ARIDITY_RISE.0,
            WEATHERING_ARIDITY_RISE.1,
            aridity,
        )
        * (1.0
            - smoothstep(
                WEATHERING_ARIDITY_FALL.0,
                WEATHERING_ARIDITY_FALL.1,
                aridity,
            ))
        * (1.0 - smoothstep(WEATHERING_RELIEF.0, WEATHERING_RELIEF.1, s.relief))
        * not_mountain
        * (1.0 - ice);

    let rain = land * smoothstep(RAIN_ARIDITY.0, RAIN_ARIDITY.1, aridity) * not_mountain;

    Processes {
        wind,
        flood,
        ice,
        weathering,
        rain,
    }
}

/// Stage 1 for the processes: per climate cell, how far it lies below the
/// ground upwind ("basin") and how much sand the wind brings from dry
/// lowland upwind ("sand"), both in `[0, 1]`. On the climate map's grid and
/// sampled with its weights; built from the base elevation and climate
/// only, so the drainage network (built later, on landformed ground) is not
/// an input — desert rivers' beds as a sand source wait for it.
pub struct ProcessMap {
    basin: Vec<f32>,
    sand: Vec<f32>,
    /// Per cell: whether flash floods can act on any column whose climate
    /// reads this cell (`flood_possible` at the most a column there could
    /// read: the 3×3 cells around). Lets the generator skip the mesa layer
    /// with one lookup, which matters because river shaping asks for
    /// many neighbours' ground per column.
    floodable: Vec<bool>,
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
}

/// Its size, not its cells.
impl std::fmt::Debug for ProcessMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProcessMap({} cells)", self.basin.len())
    }
}

impl ProcessMap {
    /// Build from the climate map's cells. Deterministic; parallel over
    /// rows. Winds blow along rows, as the climate's do: westerlies (toward
    /// +x) where [`eastward_share`] is 1, easterlies where it is 0, both
    /// carried and blended between.
    pub fn build(climate: &ClimateMap, elevation: &Elevation, shape: WorldShape) -> Self {
        let (nx, nz) = climate.cells();
        let cell = climate.cell_blocks().0 as f32;
        let continents = elevation.continent_scale();
        let upwind_k = 1.0 - exp(-cell / (BASIN_UPWIND_LENGTH * continents));
        let sand_k = 1.0 - exp(-cell / (SAND_CARRY_LENGTH * continents));
        let cell_z = climate.cell_blocks().1;

        let rows: Vec<(Vec<f32>, Vec<f32>)> = (0..nz)
            .into_par_iter()
            .map(|j| {
                let z = ((j as f64 + 0.5) * cell_z).floor() as i64;
                let abs_s = (shape.latitude_sine(z) as f32).abs();
                let w = eastward_share(abs_s);
                // Ground (sea counts as sea level) and sand source per cell.
                let mut ground = Vec::with_capacity(nx);
                let mut source = Vec::with_capacity(nx);
                for i in 0..nx {
                    let (sea_c, precip, elev, land) = climate.cell(i, j);
                    ground.push(elev.max(0.0));
                    let t = temperature_at(sea_c, elev.max(0.0) as i64);
                    let aridity = precip / aridity_threshold(t);
                    source.push(
                        land * (1.0 - smoothstep(SAND_ARIDITY.0, SAND_ARIDITY.1, aridity))
                            * (1.0 - smoothstep(SAND_HEIGHT.0, SAND_HEIGHT.1, elev)),
                    );
                }
                let (up_e, sand_e) = carry(&ground, &source, upwind_k, sand_k, true);
                let (up_w, sand_w) = carry(&ground, &source, upwind_k, sand_k, false);
                let mut basin = Vec::with_capacity(nx);
                let mut sand = Vec::with_capacity(nx);
                for i in 0..nx {
                    let up = up_w[i] + (up_e[i] - up_w[i]) * w;
                    let land = climate.cell(i, j).3;
                    basin.push(land * smoothstep(0.0, BASIN_DEPTH, up - ground[i]));
                    sand.push(sand_w[i] + (sand_e[i] - sand_w[i]) * w);
                }
                (basin, sand)
            })
            .collect();
        let mut basin: Vec<f32> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
        let mut sand: Vec<f32> = rows.iter().flat_map(|r| r.1.iter().copied()).collect();
        blur(&mut basin, nx, nz);
        blur(&mut sand, nx, nz);
        let floodable = (0..nx * nz)
            .map(|c| {
                let (i, j) = (c % nx, c / nx);
                (0..9).any(|k| {
                    let ii = (i + nx + k % 3 - 1) % nx;
                    let jj = (j + nz + k / 3 - 1) % nz;
                    let (sea_c, precip, _, _) = climate.cell(ii, jj);
                    flood_possible(sea_c, precip, FLOOD_HEIGHT.0 as i64 + 1)
                })
            })
            .collect();
        let (cell_x, cell_z) = climate.cell_blocks();
        Self {
            basin,
            sand,
            floodable,
            nx,
            nz,
            cell_x,
            cell_z,
        }
    }

    /// Whether flash floods can act at a column at all (conservative).
    pub fn floodable(&self, x: i64, z: i64) -> bool {
        let i = ((x as f64 / self.cell_x).floor() as i64).rem_euclid(self.nx as i64) as usize;
        let j = ((z as f64 / self.cell_z).floor() as i64).rem_euclid(self.nz as i64) as usize;
        self.floodable[j * self.nx + i]
    }

    /// Basin and sand at the four cells and weights the climate map gives.
    fn sample(&self, corners: [usize; 4], t: (f32, f32)) -> (f32, f32) {
        (
            lerp4(&self.basin, corners, t),
            lerp4(&self.sand, corners, t),
        )
    }
}

/// Along one row, in one wind direction, twice round the torus so the start
/// does not matter: per cell, the mean ground upwind (excluding the cell)
/// and the sand arriving (including the cell's own).
fn carry(
    ground: &[f32],
    source: &[f32],
    up_k: f32,
    sand_k: f32,
    east: bool,
) -> (Vec<f32>, Vec<f32>) {
    let n = ground.len();
    let (mut up, mut sand) = (vec![0.0; n], vec![0.0; n]);
    let start = if east { n - 1 } else { 0 };
    let (mut mean, mut carried) = (ground[start], source[start]);
    for lap in 0..2 {
        for k in 0..n {
            let i = if east { k } else { n - 1 - k };
            if lap == 1 {
                up[i] = mean;
            }
            mean += (ground[i] - mean) * up_k;
            carried += (source[i] - carried) * sand_k;
            if lap == 1 {
                sand[i] = carried.max(source[i]);
            }
        }
    }
    (up, sand)
}

/// A column's inputs, from the stage-1 maps and the base surface: the one
/// place they are put together (the generator, the world map and the tests
/// all come here).
pub(crate) fn inputs_at(
    climate: &ClimateMap,
    map: &ProcessMap,
    elevation: &Elevation,
    shape: WorldShape,
    x: i64,
    z: i64,
) -> Inputs {
    inputs_with(
        climate,
        map,
        elevation,
        shape,
        x,
        z,
        elevation.surface(x, z),
    )
}

/// [`inputs_at`] for a base surface already computed.
pub(crate) fn inputs_with(
    climate: &ClimateMap,
    map: &ProcessMap,
    elevation: &Elevation,
    shape: WorldShape,
    x: i64,
    z: i64,
    surface: Surface,
) -> Inputs {
    let (corners, t) = climate.corners(x, z);
    let (sea_c, precip) = climate.sample(x, z);
    // M13's boundary noise, through the biomes' own jitter, so a process's
    // edge frays as a biome's does.
    let jittered = biome::classify(
        surface,
        sea_c,
        precip,
        elevation.boundary_noise(x, z),
        false,
    );
    let (basin, sand) = map.sample(corners, t);
    Inputs {
        surface,
        sea_level_c: jittered.sea_level_temperature,
        precipitation: jittered.precipitation,
        abs_s: (shape.latitude_sine(z) as f32).abs(),
        basin,
        sand,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(degrees: f32) -> f32 {
        // Basic arithmetic is enough for a test: std's sin is fine here.
        degrees.to_radians().sin()
    }

    /// The catalogue's ice-age table: the height above which ice lay, by
    /// latitude, from the latitude curve alone (no continental cooling).
    #[test]
    fn ice_age_ice_lies_where_the_catalogue_says() {
        use crate::climate::{LAPSE_PER_BLOCK, sea_level_temperature_at_latitude};
        // (latitude °, catalogue's ice-age ice line, blocks).
        for (lat, expected) in [
            (0.0, 720.0),
            (20.0, 600.0),
            (30.0, 410.0),
            (40.0, 165.0),
            (45.0, 50.0),
            (50.0, 0.0),
            (70.0, 0.0),
        ] {
            let s = sine(lat);
            let t = sea_level_temperature_at_latitude(s) - ice_age_cooling(s);
            let line = (t / LAPSE_PER_BLOCK).max(0.0);
            assert!(
                (line - expected).abs() <= 15.0,
                "{lat}°: ice-age ice above {line:.0}, catalogue {expected}"
            );
        }
        assert_eq!(ice_age_cooling(0.0), 4.0);
        assert_eq!(ice_age_cooling(1.0), 11.0);
    }

    fn surface(height: i64, relief: f32, uplift: f32) -> Surface {
        Surface {
            height,
            relief,
            uplift,
            sand: 0,
            rock: false,
        }
    }

    /// A column at a latitude with a climate given as temperature at its
    /// own height and rain.
    fn at(lat: f32, s: Surface, t_here: f32, precip: f32, basin: f32, sand: f32) -> Processes {
        let sea_level_c = t_here + crate::climate::LAPSE_PER_BLOCK * s.height.max(0) as f32;
        weights(&Inputs {
            surface: s,
            sea_level_c,
            precipitation: precip,
            abs_s: sine(lat),
            basin,
            sand,
        })
    }

    /// Spot checks against Earth's places: the process that shaped each is
    /// the strongest there, and the processes that did not are absent.
    #[test]
    fn earth_places_have_their_processes() {
        // The Rub' al Khali: 26 °C, 40 mm, a low basin downwind of sand.
        let p = at(22.0, surface(60, 0.05, 0.0), 26.0, 40.0, 0.8, 0.9);
        assert!(p.wind > 0.9, "{p:?}");
        assert_eq!((p.flood, p.ice, p.weathering, p.rain), (0.0, 0.0, 0.0, 0.0));
        // The Colorado Plateau: 300 blocks, 12 °C, 250 mm (threshold 380),
        // rough.
        let p = at(37.0, surface(300, 0.45, 10.0), 12.0, 250.0, 0.0, 0.3);
        assert!(p.flood > 0.9, "{p:?}");
        assert_eq!((p.wind, p.weathering, p.rain), (0.0, 0.0, 0.0));
        // The Serengeti: 22 °C, 800 mm (aridity ~1.3), a flat old plain.
        let p = at(3.0, surface(80, 0.1, 0.0), 22.0, 800.0, 0.0, 0.0);
        assert!(p.weathering > 0.9, "{p:?}");
        assert_eq!((p.wind, p.flood, p.ice), (0.0, 0.0, 0.0));
        // The Appalachians: 11 °C, 1 200 mm (aridity ~3.2), rolling.
        let p = at(37.0, surface(150, 0.3, 20.0), 11.0, 1_200.0, 0.0, 0.0);
        assert!(p.rain > 0.9, "{p:?}");
        assert_eq!((p.wind, p.flood, p.ice, p.weathering), (0.0, 0.0, 0.0, 0.0));
        // Western Norway's lowlands: 60°, 7 °C, 2 000 mm — under the
        // ice-age ice sheet, wet today.
        let p = at(60.0, surface(20, 0.3, 0.0), 7.0, 2_000.0, 0.0, 0.0);
        assert!(p.ice > 0.99 && p.rain > 0.9, "{p:?}");
        // Siberia: 62°, an interior far colder than 0 °C in the ice age but
        // with 250 mm of snow and rain — never under the ice sheets.
        let p = at(62.0, surface(150, 0.1, 0.0), -8.0, 250.0, 0.0, 0.0);
        assert!(p.ice < 0.2, "{p:?}");
        // Antarctica's interior: a polar desert, 50 mm, under ice now.
        let p = at(85.0, surface(800, 0.1, 0.0), -40.0, 50.0, 0.0, 0.0);
        assert_eq!(p.ice, 1.0, "{p:?}");
        assert_eq!(
            (p.wind, p.flood, p.weathering, p.rain),
            (0.0, 0.0, 0.0, 0.0)
        );
        // Kilimanjaro's flanks at 800 blocks on the equator were glaciated;
        // the plain below was not.
        let mountain = surface(800, 0.5, 600.0);
        let t = 27.0 - crate::climate::LAPSE_PER_BLOCK * 800.0;
        assert!(at(3.0, mountain, t, 1_000.0, 0.0, 0.0).ice > 0.9);
        assert_eq!(
            at(3.0, surface(80, 0.1, 0.0), 26.0, 1_000.0, 0.0, 0.0).ice,
            0.0
        );
    }

    /// The catalogue's "zero where" column, on a grid of inputs.
    #[test]
    fn each_process_is_absent_where_the_catalogue_says() {
        for h in [-50, 0, 30, 120, 160, 400] {
            for relief in [0.0, 0.3, 0.6] {
                for uplift in [0.0, 150.0, 450.0] {
                    for t in [-15.0, 0.0, 12.0, 25.0] {
                        for precip in [20.0, 300.0, 900.0, 3_000.0] {
                            for lat in [0.0, 30.0, 60.0] {
                                let s = surface(h, relief, uplift);
                                let p = at(lat, s, t, precip, 1.0, 1.0);
                                let aridity = precip / aridity_threshold(t);
                                for (name, w) in p.named() {
                                    assert!((0.0..=1.0).contains(&w), "{name} {w}");
                                }
                                let mountain = uplift >= MOUNTAIN_UPLIFT.1;
                                let sea = h <= 0;
                                if aridity >= 0.6 || mountain || h >= 150 || sea {
                                    assert_eq!(p.wind, 0.0, "{s:?} {t} {precip}");
                                }
                                if aridity >= 0.9 || mountain || h <= 80 {
                                    assert_eq!(p.flood, 0.0, "{s:?} {t} {precip}");
                                }
                                if t <= 18.0 || aridity <= 0.5 || mountain || sea {
                                    assert_eq!(p.weathering, 0.0, "{s:?} {t} {precip}");
                                }
                                if aridity <= 0.8 || mountain || sea {
                                    assert_eq!(p.rain, 0.0, "{s:?} {t} {precip}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Wind's basin: a place lower than the ground upwind, in the wind's
    /// own direction — and sand downwind of its source, not upwind.
    #[test]
    fn basins_and_sand_follow_the_wind() {
        // Wind toward +x: a plateau at 0..10, a basin at 10..20.
        let mut ground = vec![300.0; 10];
        ground.extend(vec![100.0; 10]);
        let mut source = vec![0.0; 20];
        source[5] = 1.0;
        let (up, sand) = carry(&ground, &source, 0.3, 0.3, true);
        assert!(
            up[10] - ground[10] > 150.0,
            "the basin's first cell sees the plateau"
        );
        assert!(up[3] - ground[3] < 1.0, "the plateau is not a basin");
        assert!(sand[7] > 0.1 && sand[3] < 0.01, "sand blows downwind only");
        // The other way, the basin's edge by the plateau has only basin
        // upwind of it.
        let (up, _) = carry(&ground, &source, 0.3, 0.3, false);
        assert!(up[10] - ground[10] < 50.0);
    }

    /// Over a real world: weights are a pure function of the column (two
    /// generators agree, and any lap of the torus is the same column), and
    /// every process acts somewhere in the default world.
    #[test]
    fn real_world_weights_are_deterministic_and_all_present() {
        let shape = WorldShape::DEFAULT;
        let g = crate::Generator::new(0x7e22ac0de, shape);
        let other = crate::Generator::new(0x7e22ac0de, shape);
        let mut seen = [0u32; 5];
        let step = shape.size_x() / 96;
        for j in 0..96 {
            for i in 0..96 {
                let (x, z) = (i * step, j * step);
                let p = g.processes(x, z);
                assert_eq!(p, other.processes(x, z));
                assert_eq!(p, g.processes(x + shape.size_x(), z - shape.size_z()));
                for (k, (_, w)) in p.named().iter().enumerate() {
                    seen[k] += u32::from(*w > 0.5);
                }
            }
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
    }
}
