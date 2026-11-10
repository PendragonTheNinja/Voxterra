//! The drainage network: stage 1 of hydrology (M14 task 1).
//!
//! A river must know its upstream (`docs/vision.md` §1.3), which no chunk can
//! see; so, like the climate map (ADR-0017), the network is computed once per
//! world on a coarse grid over the whole torus, and chunks read it.
//!
//! 1. **Sample** the elevation field at each cell's centre.
//! 2. **Find the hollows and decide which hold lakes**: a merge tree of the
//!    depressions (Barnes et al., 2020), judged from the outermost in. Deep
//!    basins and ice-scoured hollows hold lakes to their spill level; the
//!    rest are breached, as rivers breach them over geological time. Which
//!    lakes dry into salt pans is decided by their water budget later
//!    (task 1, closed basins).
//! 3. **Route flow** in one priority flood (Barnes et al., 2014) seeded from
//!    the sea over the water surface: each cell's *receiver* — the neighbour
//!    its water flows to — is the one it was reached from. The channel level
//!    is cut down along the flow so water never runs uphill; a breached rim
//!    becomes a gorge. Every land cell drains, in the end, to the sea or a
//!    closed basin.
//! 4. **Accumulate discharge**: each cell's runoff — precipitation less
//!    evaporation (ADR-0017's climate; the Budyko curve,
//!    [`crate::climate::runoff`]) — summed down the receiver tree, less
//!    what lakes evaporate; closed basins keep what reaches them. Discharge is in m³/s, honestly from our areas
//!    and Earth's rainfall; widths and depths are scaled from it for play.
//!
//! Deterministic on every machine: integer heights, ties broken by cell
//! index, one sequential flood, float sums taken in a fixed order.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rayon::prelude::*;
use vox_core::WorldShape;

use crate::climate::{ClimateMap, potential_evaporation, runoff, temperature_at};
use crate::elevation::Elevation;

mod cache;
mod erosion;
mod hollows;
mod rivers;
pub use cache::{CacheKey, FORMAT_VERSION as CACHE_FORMAT_VERSION};
use erosion::Erosion;
pub use hollows::Cover;
pub use rivers::{Bed, Body, MAX_FLOW, Near, RiverKind, STREAM_FROM_M3S, Water, river_kind};

/// Target size of a drainage cell, in blocks. Rivers need finer cells than
/// climate (ADR-0017's 256); the grid is capped so the largest worlds stay
/// buildable.
const CELL_TARGET: i64 = 128;
const MIN_CELLS: i64 = 256;
const MAX_CELLS: i64 = 2_048;
/// The smallest body of ground below sea level that counts as sea, in cells
/// (~30 km² at 128-block cells). Smaller ones are inland pits — land
/// hollows that fill like any other, or not, by their own water.
const SEA_MIN_CELLS: usize = 2_048;
/// Evaporation from a lake, as a multiple of the potential evaporation:
/// open water loses a little more than a well-watered field, and a salt
/// lake in a hot desert most of all (the Dead Sea, Lake Chad: 1 500–2 000
/// mm a year).
const LAKE_EVAPORATION: f32 = 1.2;
/// The least runoff valleys were carved by, mm/yr: a semi-arid
/// steppe's, as every land was wetter at some time in the ages its valleys
/// took to cut. Today's lakes and closed basins follow today's climate.
const PALEO_RUNOFF_MM: f64 = 100.0;
/// Seconds in a year, for mm/yr over an area to m³/s.
const SECONDS_PER_YEAR: f64 = 31_557_600.0;

/// Which hollows stay lakes. Noise is full of closed hollows; real
/// landscapes have few outside glaciated ground, rifts and deserts, because
/// over geological time rivers cut through every rim they can. So a hollow
/// stays a lake if it is a deep basin — a mean depth of at least
/// [`LAKE_MIN_MEAN_DEPTH`] blocks over [`LAKE_MIN_CELLS`] cells (~1 km
/// across), a rift or a tectonic sag — or, on ground scoured by ice (below
/// [`GLACIATED_BELOW_C`]), a small hollow at least
/// [`LAKE_MIN_DEPTH_GLACIATED`] deep: the boreal and tundra "land of ten
/// thousand lakes". Everything else has its rim breached and the river runs
/// through, including the broad shallow basins the elevation noise leaves
/// on plains (mean depth ~20 over thousands of cells), which no river would
/// have left closed.
///
/// Measured on the default world (seed 0x7e22ac0de, 204 800 blocks), before
/// valley erosion (task 2): lakes cover ~27% of glaciated land (Finland's
/// Lakeland is a quarter water, the Canadian Shield more in places) and ~6%
/// elsewhere, ~14% overall, against Earth's ~3.7% of unglaciated land
/// (Verpoorter et al., 2014). The excess is the continental interior: a flat
/// plateau at `LOWLAND_RISE` pitted by detail noise, deranged until erosion
/// cuts it; the shares are re-measured once it does.
const LAKE_MIN_MEAN_DEPTH: i64 = 60;
const LAKE_MIN_CELLS: usize = 60;
const LAKE_MIN_DEPTH_GLACIATED: i32 = 10;
const LAKE_MIN_CELLS_GLACIATED: usize = 3;
const LAKE_MAX_CELLS_GLACIATED: usize = 200;
/// Mean annual temperature (°C, at the ground) below which a hollow counts
/// as ice-scoured.
const GLACIATED_BELOW_C: f32 = 2.0;

/// A cell that drains into itself: the sea, a closed basin.
const HERE: u8 = 8;
/// How often the ground is sampled along a reach to seat the river in it
/// and lay its profile, blocks.
const SEAT_SPACING: f64 = 4.0;
/// How much of a reach's start its node is seated against.
const SEAT_NEAR: f64 = 0.125;
/// The points of a reach's long profile.
pub(crate) const PROFILE_STEPS: usize = 8;

/// The bit of a cell's cover marking a closed basin's sink.
const SINK: u8 = 0x80;
/// The cover byte's low bits: the `Cover` itself.
const COVER_BITS: u8 = 0x07;
/// The cover byte's climate bits (M14 criterion 10): the cell is arid
/// (precipitation under half its potential evaporation), or ice-scoured —
/// what makes a river braided or a wadi.
const ARID: u8 = 0x08;
const COLD: u8 = 0x10;

/// The world's drainage, on a grid of cells, row-major (`j * nx + i`).
pub struct DrainageMap {
    /// Unique to this map in the process: keys stage 2's per-thread cache
    /// of nearby reaches, so no map ever reads another's.
    id: u64,
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
    // Packed, ~11 bytes a cell (ADR-0019): heights fit the world's vertical
    // range in an `i16`, a receiver is one of eight directions.
    /// Ground at the cell's centre, blocks.
    height: Vec<i16>,
    /// The water surface here if it is a lake, else the river bed: the
    /// ground, cut down where a river breaches a hollow's rim. Never rises
    /// downstream.
    level: Vec<i16>,
    /// What covers the cell (`Cover as u8`), with [`SINK`] set where water
    /// ends without reaching the sea: a closed basin.
    cover: Vec<u8>,
    /// The way the cell drains: an index into [`neighbours`], or [`HERE`]
    /// for the sea and closed basins.
    flow: Vec<u8>,
    /// Water leaving the cell, m³/s.
    discharge: Vec<f32>,
    /// Long profiles of the reaches that fall (ADR-0020 §10): by cell
    /// index, ascending, each its water at PROFILE_STEPS points along it,
    /// from its own level (255) to its receiver's (0).
    profile: Vec<(u32, [u8; PROFILE_STEPS])>,
    /// The elevation's erosion stage (task 2).
    erosion: Erosion,
    /// The water budget, kept for the conservation test.
    #[cfg(test)]
    budget: (Vec<f64>, Vec<f64>),
}

impl std::fmt::Debug for DrainageMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DrainageMap({}×{} cells of {:.0} blocks)",
            self.nx, self.nz, self.cell_x
        )
    }
}

impl DrainageMap {
    /// Build the network for one world from its elevation and climate.
    pub fn build(elevation: &Elevation, climate: &ClimateMap, shape: WorldShape) -> Self {
        let (nx, nz) = grid(shape);
        let cell_x = shape.size_x() as f64 / nx as f64;
        let cell_z = shape.size_z() as f64 / nz as f64;
        let centre = |c: usize| {
            let (i, j) = (c % nx, c / nx);
            (
                ((i as f64 + 0.5) * cell_x).floor() as i64,
                ((j as f64 + 0.5) * cell_z).floor() as i64,
            )
        };
        let base: Vec<i32> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                elevation.height(x, z) as i32
            })
            .collect();
        let sea = seas(nx, nz, &base);
        // Runoff per cell (the Budyko water balance), and what a lake over
        // it would cost: its evaporation less the rain on it, plus the
        // runoff it no longer sheds — never negative. Both m³/s.
        let per_mm = cell_x * cell_z / 1000.0 / SECONDS_PER_YEAR;
        let water: Vec<(f32, f32)> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                let (sea_level_c, precip) = climate.sample(x, z);
                let t = temperature_at(sea_level_c, i64::from(base[c].max(0)));
                (precip, potential_evaporation(t))
            })
            .collect();
        let (runoff, demand): (Vec<f64>, Vec<f64>) = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                if sea[c] {
                    return (0.0, 0.0);
                }
                let (x, z) = centre(c);
                let (sea_level_c, precip) = climate.sample(x, z);
                let t = temperature_at(sea_level_c, i64::from(base[c].max(0)));
                let runoff = runoff(t, precip);
                let evaporation = LAKE_EVAPORATION * potential_evaporation(t);
                (
                    f64::from(runoff) * per_mm,
                    f64::from((runoff + evaporation - precip).max(0.0)) * per_mm,
                )
            })
            .unzip();
        let cold: Vec<bool> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                let (sea_level_c, _) = climate.sample(x, z);
                temperature_at(sea_level_c, i64::from(base[c].max(0))) < GLACIATED_BELOW_C
            })
            .collect();
        // Valleys first (task 2): the hollows and the network are those of
        // the eroded ground.
        // The valleys are older than today's climate: carved through wetter
        // ages too (the Sahara's wadis ran in the Holocene's green Sahara),
        // so erosion runs on at least [`PALEO_RUNOFF_MM`] everywhere.
        let carving: Vec<f64> = runoff
            .iter()
            .zip(&sea)
            .map(|(&r, &s)| {
                if s {
                    0.0
                } else {
                    r.max(PALEO_RUNOFF_MM * per_mm)
                }
            })
            .collect();
        let erosion = Erosion::build(nx, nz, (cell_x, cell_z), &base, &sea, &carving);
        let height: Vec<i32> = (0..nx * nz)
            .map(|c| {
                let (x, z) = centre(c);
                base[c] + erosion.at(x as f64, z as f64).round() as i32
            })
            .collect();
        let held = hollows::settle(
            nx,
            nz,
            &hollows::Ground {
                height: &height,
                sea: &sea,
                cold: &cold,
                climate: &water,
                runoff: &runoff,
                demand: &demand,
            },
        );
        let (surface, mut cover, sink) = (held.surface, held.cover, held.sink);
        let lake: Vec<bool> = cover.iter().map(|&c| c == Cover::Lake).collect();
        let (receiver, order) = flood(nx, nz, &surface, &sea, &sink, &lake);
        // The sea's level is its surface, sea level, and like a lake's it is
        // never cut: a river cannot be graded below the sea it ends in. (It
        // had been the seabed's depth, and a river draining an inland pit
        // below sea level was cut to the pit's floor all the way to the
        // coast, 50–100 blocks under the land.)
        let still: Vec<bool> = (0..nx * nz).map(|c| lake[c] || sink[c] || sea[c]).collect();
        let water_surface: Vec<i32> = (0..nx * nz)
            .map(|c| if sea[c] { 0 } else { surface[c] })
            .collect();
        let (mut spill, ponded) = channel_levels(&water_surface, &receiver, &order, &still);
        for c in 0..nx * nz {
            if sea[c] {
                spill[c] = surface[c].min(0); // the sea keeps its depth on the map
            }
        }
        // Downstream accumulation in reverse flood order: every cell is
        // reached after its receiver, so reversed, every cell is summed
        // before the water moves on. A lake loses its evaporation; what
        // leaves it is never less than nothing. Closed basins keep all.
        let mut q: Vec<f64> = (0..nx * nz)
            .map(|c| {
                if cover[c] == Cover::Lake {
                    runoff[c] - demand[c]
                } else {
                    runoff[c]
                }
            })
            .collect();
        for &c in order.iter().rev() {
            let c = c as usize;
            let r = receiver[c] as usize;
            if r == c {
                continue;
            }
            let leaving = cover[c] == Cover::Lake && cover[r] != Cover::Lake;
            q[r] += if leaving { q[c].max(0.0) } else { q[c] };
        }
        let discharge: Vec<f32> = q.iter().map(|&v| v.max(0.0) as f32).collect();
        // Ground below the level water reaches: a lake's backwater where it
        // backs up from a lake; elsewhere, below a river's grade behind a
        // rim it could only cut so far, filled with sediment over geological
        // time — a flat alluvial floor (stage 2 raises it).
        for c in 0..nx * nz {
            let still_water = matches!(cover[receiver[c] as usize], Cover::Lake | Cover::SaltLake);
            // Backed-up water is the lake's whether or not a stream crosses
            // it: a river entering a lake ends there. (Exempting stream
            // cells left their straight flood paths as strips of land across
            // the lake.)
            if cover[c] == Cover::Land && spill[c] > height[c] && (ponded[c] || still_water) {
                cover[c] = Cover::Lake;
            }
        }
        // The full-resolution ground, which stage 2 shapes.
        let ground = |x: f64, z: f64| {
            let (x, z) = (x.floor() as i64, z.floor() as i64);
            elevation.height(x, z) as i32 + erosion.at(x as f64, z as f64).round() as i32
        };
        // The ground a river is seated under: none where a lake or the sea
        // lies — that is water, not ground to cut through (a river running
        // along a lake's shore had been seated by the lake's bed, 27 blocks
        // below the lake it drains: the owner's "insanely deep groove").
        // The lake's region is stage 2's: discs about its cells' centres
        // (`rivers::REGION_*`), where its water covers ground below it.
        let seat_ground = |x: f64, z: f64| {
            let (fi, fj) = ((x / cell_x).floor() as i64, (z / cell_z).floor() as i64);
            let at = |i: i64, j: i64| {
                (j.rem_euclid(nz as i64) as usize) * nx + i.rem_euclid(nx as i64) as usize
            };
            if matches!(cover[at(fi, fj)], Cover::Sea) {
                return i32::MAX;
            }
            let g = ground(x, z);
            let mut wobble = None;
            for dj in -1..=1 {
                for di in -1..=1 {
                    let c = at(fi + di, fj + dj);
                    if !matches!(cover[c], Cover::Lake | Cover::SaltLake) || g >= spill[c] {
                        continue;
                    }
                    let w = *wobble.get_or_insert_with(|| {
                        f64::from(
                            elevation
                                .meander_warp(
                                    x.floor() as i64,
                                    z.floor() as i64,
                                    rivers::REGION_WAVELENGTH,
                                )
                                .0,
                        )
                    });
                    let (cx, cz) = (
                        ((fi + di) as f64 + 0.5) * cell_x,
                        ((fj + dj) as f64 + 0.5) * cell_z,
                    );
                    let radius =
                        cell_x.min(cell_z) * (rivers::REGION_RADIUS + rivers::REGION_WOBBLE * w);
                    // Squared by multiplication, as everywhere in generation:
                    // only + − × ÷ and sqrt are bit-exact on every platform
                    // (`climate.rs`), and `powi` promises nothing.
                    let (dx, dz) = (x - cx, z - cz);
                    if (dx * dx + dz * dz).sqrt() <= radius {
                        return i32::MAX;
                    }
                }
            }
            g
        };
        // Rivers sit in their ground, never on it: the levels so far are the
        // cells' centres' ground, but a reach runs between jittered nodes
        // over the full-resolution ground, which a 128-block grid does not
        // see. Wherever that ground falls below the line, stage 2 had to
        // build it up to hold the water — embankments carrying rivers over
        // the land, 85 blocks high at a mountain waterfall's pool (the
        // owner's walk). So each reach's level is lowered until the line to
        // its receiver runs a block under the ground along it.
        // Reaches with a profile: streams, and the rivers leaving lakes
        // (whose water had run at the lake's level all the way to the next
        // node, its banks held up beside the river below — a slot).
        let lake_cell = |c: usize| matches!(cover[c], Cover::Lake | Cover::SaltLake);
        let stream = |c: usize| {
            let r = receiver[c] as usize;
            r != c
                && !sea[c]
                && (cover[c] == Cover::Land || (lake_cell(c) && !lake_cell(r)))
                && discharge[c] >= STREAM_FROM_M3S
        };
        let reach_line = |c: usize| {
            let (i, j) = ((c % nx) as i64, (c / nx) as i64);
            let r = receiver[c] as usize;
            let dir = neighbours(c, nx, nz)
                .iter()
                .position(|&nb| nb == r)
                .unwrap();
            let (di, dj) = rivers::OFFSETS[dir];
            let cell = (cell_x, cell_z);
            (
                rivers::node(c, i, j, cell),
                rivers::node(r, i + di, j + dj, cell),
            )
        };
        let water_level = |spill: &[i32], c: usize| if sea[c] { 0 } else { spill[c] };
        // The ground along a reach, every `spacing` blocks: at each step
        // the lowest of the line and points `belt` either side of it.
        let along = |c: usize, spacing: f64, belt: f64| -> Vec<i32> {
            let (a, b) = reach_line(c);
            let (dx, dz) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dz * dz).sqrt();
            let (ux, uz) = (dx / len, dz / len);
            let n = (len / spacing).ceil().max(2.0) as usize;
            (0..n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let (x, z) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                    let across = |f: f64| seat_ground(x - uz * belt * f, z + ux * belt * f);
                    if belt > 0.0 {
                        [-1.0, -0.5, 0.0, 0.5, 1.0]
                            .map(across)
                            .into_iter()
                            .min()
                            .unwrap()
                    } else {
                        across(0.0)
                    }
                })
                .collect()
        };
        // The river may lie anywhere across its belt — its meanders and
        // wander carry it there — and wherever it lies, the ground beside
        // it must hold it: a river crossing a hillside, seated on its
        // line, needed its downhill bank built up into a ridge.
        let samples: Vec<Vec<i32>> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                if !stream(c) {
                    return Vec::new();
                }
                let r = receiver[c] as usize;
                let drop = (water_level(&spill, c) - water_level(&spill, r)).max(0);
                let slope = drop as f32 / cell_x.min(cell_z) as f32;
                let belt = rivers::belt(discharge[r].max(discharge[c]), slope);
                along(c, SEAT_SPACING, f64::from(belt))
            })
            .collect();
        // Outlets first, so every receiver is seated before its donors: a
        // node a block under the ground around it, never below its
        // receiver. (It had been lowered until the straight line to its
        // receiver ran under the ground all the way, which on a concave
        // mountainside sank it tens of blocks — a river 27 below the lake it
        // drains, in a slot. The profile below follows the ground instead.)
        for &c in &order {
            let c = c as usize;
            // A lake keeps its level; only land is seated.
            if samples[c].is_empty() || cover[c] != Cover::Land {
                continue;
            }
            let lr = water_level(&spill, receiver[c] as usize).min(spill[c]);
            let near = ((samples[c].len() as f64 * SEAT_NEAR).ceil() as usize).max(1);
            let under = samples[c][..near].iter().min().unwrap().saturating_sub(1);
            spill[c] = spill[c].min(under).max(lr);
        }
        // Each reach's long profile: its water at PROFILE_STEPS points, a
        // block under the lowest ground across its belt as far as the next
        // point (so the level held between them is under it too), never
        // rising downstream, between its own level and its receiver's.
        // Water steps down a mountainside with its ground — rapids, and
        // waterfalls where the ground steps — instead of running a straight
        // line from node to node that cut grooves where the slope was
        // concave and stood on causeways where it was convex (the owner's
        // walk).
        let profile: Vec<(u32, [u8; PROFILE_STEPS])> = (0..nx * nz)
            .into_par_iter()
            .filter_map(|c| {
                let s = &samples[c];
                if s.is_empty() {
                    return None;
                }
                let (lc, lr) = (
                    spill[c],
                    water_level(&spill, receiver[c] as usize).min(spill[c]),
                );
                if lc == lr {
                    return None;
                }
                let n = s.len();
                let mut steps = [0u8; PROFILE_STEPS];
                for (k, step) in steps.iter_mut().enumerate() {
                    let upto = ((k + 1) * n / PROFILE_STEPS).min(n - 1);
                    let under = s[..=upto].iter().min().unwrap().saturating_sub(1);
                    let p = under.clamp(lr, lc);
                    *step = (f64::from(p - lr) * 255.0 / f64::from(lc - lr)).round() as u8;
                }
                Some((c as u32, steps))
            })
            .collect();
        let narrow = |v: i32| i16::try_from(v).expect("heights fit the world's vertical range");
        Self {
            id: next_id(),
            nx,
            nz,
            cell_x,
            cell_z,
            height: height.iter().map(|&h| narrow(h)).collect(),
            level: spill.iter().map(|&h| narrow(h)).collect(),
            cover: (0..nx * nz)
                .map(|c| {
                    let (precip, pet) = water[c];
                    cover[c] as u8
                        | if sink[c] { SINK } else { 0 }
                        | if precip < 0.5 * pet { ARID } else { 0 }
                        | if cold[c] { COLD } else { 0 }
                })
                .collect(),
            flow: (0..nx * nz)
                .map(|c| {
                    let r = receiver[c] as usize;
                    if r == c {
                        HERE
                    } else {
                        neighbours(c, nx, nz)
                            .iter()
                            .position(|&nb| nb == r)
                            .unwrap() as u8
                    }
                })
                .collect(),
            discharge,
            profile,
            erosion,
            #[cfg(test)]
            budget: (runoff, demand),
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

    /// The cell holding a column, on any lap of the world.
    pub fn cell_of(&self, x: i64, z: i64) -> (usize, usize) {
        let i = ((x as f64 / self.cell_x).floor() as i64).rem_euclid(self.nx as i64) as usize;
        let j = ((z as f64 / self.cell_z).floor() as i64).rem_euclid(self.nz as i64) as usize;
        (i, j)
    }

    /// Ground height, spill level, discharge (m³/s) and receiver `(i, j)` of
    /// a cell.
    pub fn cell(&self, i: usize, j: usize) -> (i32, i32, f32, (usize, usize)) {
        let c = j * self.nx + i;
        let r = self.receiver_of(c);
        (
            self.h(c),
            self.lvl(c),
            self.discharge[c],
            (r % self.nx, r / self.nx),
        )
    }

    fn h(&self, c: usize) -> i32 {
        i32::from(self.height[c])
    }

    fn lvl(&self, c: usize) -> i32 {
        i32::from(self.level[c])
    }

    fn receiver_of(&self, c: usize) -> usize {
        match self.flow[c] {
            HERE => c,
            d => neighbours(c, self.nx, self.nz)[d as usize],
        }
    }

    fn cover_at(&self, c: usize) -> Cover {
        Cover::from_u8(self.cover[c] & COVER_BITS)
    }

    fn sea_at(&self, c: usize) -> bool {
        self.cover_at(c) == Cover::Sea
    }

    fn arid_at(&self, c: usize) -> bool {
        self.cover[c] & ARID != 0
    }

    fn cold_at(&self, c: usize) -> bool {
        self.cover[c] & COLD != 0
    }

    fn sink_at(&self, c: usize) -> bool {
        self.cover[c] & SINK != 0
    }

    /// What erosion took from the base elevation at the cell's centre,
    /// blocks (never positive).
    pub fn erosion(&self, i: usize, j: usize) -> f32 {
        let x = ((i as f64 + 0.5) * self.cell_x).floor();
        let z = ((j as f64 + 0.5) * self.cell_z).floor();
        self.erosion_at(x, z)
    }

    /// The elevation's erosion stage at any column: what erosion took from
    /// the base, blocks (never positive), on any lap of the world.
    pub fn erosion_at(&self, x: f64, z: f64) -> f32 {
        self.erosion.at(x, z)
    }

    /// Whether the cell is sea.
    pub fn is_sea(&self, i: usize, j: usize) -> bool {
        self.sea_at(j * self.nx + i)
    }

    /// Whether the cell is under a lake, fresh or salt.
    pub fn is_lake(&self, i: usize, j: usize) -> bool {
        matches!(self.cover(i, j), Cover::Lake | Cover::SaltLake)
    }

    /// What covers the cell.
    pub fn cover(&self, i: usize, j: usize) -> Cover {
        self.cover_at(j * self.nx + i)
    }

    /// Whether water ends in the cell without reaching the sea.
    pub fn is_sink(&self, i: usize, j: usize) -> bool {
        self.sink_at(j * self.nx + i)
    }

    /// For every cell, row-major: whether its water ends in a closed basin
    /// rather than the sea — the world's endorheic land.
    pub fn endorheic(&self) -> Vec<bool> {
        let n = self.height.len();
        // 0 unknown, 1 sea, 2 closed basin.
        let mut end = vec![0u8; n];
        let mut path = Vec::new();
        for start in 0..n {
            let mut at = start;
            while end[at] == 0 {
                let r = self.receiver_of(at);
                if r == at {
                    end[at] = if self.sink_at(at) { 2 } else { 1 };
                    break;
                }
                path.push(at);
                at = r;
            }
            let e = end[at];
            for c in path.drain(..) {
                end[c] = e;
            }
        }
        end.into_iter().map(|e| e == 2).collect()
    }
}

/// A fresh map id.
fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A 64-bit mix (splitmix64's finaliser): per-cell jitter that is the same
/// whoever asks.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The deepest a river cuts through a hollow's rim, blocks.
const MAX_CUT: i32 = 8;

/// Cells per axis of a world's drainage grid.
fn grid(shape: WorldShape) -> (usize, usize) {
    let cells = |size: i64| (size / CELL_TARGET).clamp(MIN_CELLS, MAX_CELLS) as usize;
    (cells(shape.size_x()), cells(shape.size_z()))
}

/// The level of the water in every cell: a lake's surface, and elsewhere
/// the ground cut down along the flow so it never rises downstream — a
/// breached rim becomes the river's channel.
/// Returns the levels, and per cell whether its level was raised by water
/// standing downstream — a lake's or a closed basin's backwater — rather
/// than by a river's grade.
fn channel_levels(
    surface: &[i32],
    receiver: &[u32],
    order: &[u32],
    lake: &[bool],
) -> (Vec<i32>, Vec<bool>) {
    let mut level = surface.to_vec();
    let mut ponded = vec![false; surface.len()];
    // Donors come later in flood order than their receivers, so walking the
    // order backwards lowers each receiver toward its lowest donor before
    // the water moves on — a rim cut through — but by no more than
    // [`MAX_CUT`]: erosion did the deep cutting, and a river does not dig a
    // trench to the bottom of every hollow it crosses. Lake surfaces are
    // held at their spill.
    for &c in order.iter().rev() {
        let c = c as usize;
        let r = receiver[c] as usize;
        if r != c && !lake[r] && level[r] > level[c] {
            level[r] = level[c].max(surface[r] - MAX_CUT);
        }
    }
    // A cut cannot go below the water it flows into: walking outlets first,
    // raise each cell to its receiver's level. What rises above its ground
    // is filled: a hollow a river runs through is filled by its sediment to
    // the river's grade (stage 2 raises its bed and banks), one without a
    // river holds a lake.
    for &c in order {
        let c = c as usize;
        let r = receiver[c] as usize;
        if r != c && level[c] < level[r] {
            level[c] = level[r];
            ponded[c] = lake[r] || ponded[r];
        }
    }
    (level, ponded)
}

/// Which cells are sea: ground below sea level in bodies of at least
/// [`SEA_MIN_CELLS`], connected through their eight neighbours.
fn seas(nx: usize, nz: usize, height: &[i32]) -> Vec<bool> {
    let n = nx * nz;
    let mut sea = vec![false; n];
    let mut seen = vec![false; n];
    let mut body = Vec::new();
    for start in 0..n {
        if seen[start] || height[start] > 0 {
            continue;
        }
        body.clear();
        seen[start] = true;
        let mut stack = vec![start];
        while let Some(c) = stack.pop() {
            body.push(c);
            for nb in neighbours(c, nx, nz) {
                if !seen[nb] && height[nb] <= 0 {
                    seen[nb] = true;
                    stack.push(nb);
                }
            }
        }
        if body.len() >= SEA_MIN_CELLS {
            for &c in &body {
                sea[c] = true;
            }
        }
    }
    sea
}

/// The eight neighbours of a cell on the torus.
fn neighbours(c: usize, nx: usize, nz: usize) -> [usize; 8] {
    let (i, j) = ((c % nx) as isize, (c / nx) as isize);
    let at = |di: isize, dj: isize| {
        let ii = (i + di).rem_euclid(nx as isize) as usize;
        let jj = (j + dj).rem_euclid(nz as isize) as usize;
        jj * nx + ii
    };
    [
        at(1, 0),
        at(-1, 0),
        at(0, 1),
        at(0, -1),
        at(1, 1),
        at(-1, 1),
        at(1, -1),
        at(-1, -1),
    ]
}

/// Priority flood from the sea and the closed basins over the water surface: receivers, and the
/// order cells were reached in (outlets first). Cells are taken lowest
/// spill level first, and among equal spill levels lowest surface first, so
/// that across a filled flat the water still finds the low ground rather
/// than running in straight lines by index — and lakes last, so a breached
/// hollow beside a lake drains out through its own cut rather than through
/// the lake, which would hold it up at the lake's level.
fn flood(
    nx: usize,
    nz: usize,
    surface: &[i32],
    sea: &[bool],
    sink: &[bool],
    lake: &[bool],
) -> (Vec<u32>, Vec<u32>) {
    let n = nx * nz;
    let mut receiver = vec![u32::MAX; n];
    let mut order = Vec::with_capacity(n);
    let mut heap = BinaryHeap::with_capacity(n / 4);
    for c in 0..n {
        if sea[c] || sink[c] {
            let key = if sea[c] { 0 } else { surface[c] };
            receiver[c] = c as u32;
            heap.push(Reverse((key, surface[c], c as u32, c as u32)));
        }
    }
    if heap.is_empty() {
        // A world with no sea (none is legal, but be total): its lowest
        // cell is the outlet.
        let lowest = (0..n).min_by_key(|&c| (surface[c], c)).unwrap();
        receiver[lowest] = lowest as u32;
        heap.push(Reverse((
            surface[lowest],
            surface[lowest],
            0,
            lowest as u32,
        )));
    }
    // Ties — a lake, a dead-flat floor — go first in, first out (the third
    // key counts pushes), so water crosses them by the shortest way rather
    // than snaking along rows in index order.
    let mut pushed = n as u32;
    let mut filled = vec![i32::MIN; n];
    while let Some(Reverse((level, _, _, c))) = heap.pop() {
        order.push(c);
        filled[c as usize] = level;
        for nb in neighbours(c as usize, nx, nz) {
            if receiver[nb] != u32::MAX {
                continue;
            }
            receiver[nb] = c;
            // A lake, or a hollow the flood is filling: crossed first in,
            // first out — the shortest way to its outlet — not down to its
            // bottom and back up.
            let still = lake[nb] || level > surface[nb];
            let low = if still { i32::MAX } else { surface[nb] };
            pushed += 1;
            heap.push(Reverse((surface[nb].max(level), low, pushed, nb as u32)));
        }
    }
    // The flood says which way water leaves a hollow; where the ground
    // slopes, water takes the steepest way down. So each cell drains to its
    // lowest neighbour among those reached before it — still a tree toward
    // the outlets, but the sea and a closed basin no longer race each other
    // across a plain by index order. Lakes keep the flood's paths to their
    // outlets.
    let mut rank = vec![0u32; n];
    for (k, &c) in order.iter().enumerate() {
        rank[c as usize] = k as u32;
    }
    for &c in &order {
        let c = c as usize;
        if receiver[c] as usize == c || lake[c] || filled[c] > surface[c] {
            continue;
        }
        let mut best = receiver[c] as usize;
        for nb in neighbours(c, nx, nz) {
            if rank[nb] < rank[c] && (surface[nb], nb) < (surface[best], best) {
                best = nb;
            }
        }
        receiver[c] = best as u32;
    }
    (receiver, order)
}

/// What a network looks like against Earth (criterion 4), for tests and the
/// survey.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct NetworkStats {
    /// Share of land draining to closed basins (Earth ~0.18).
    pub endorheic: f64,
    /// Lake share of ice-scoured land and of the rest.
    pub lakes_cold: f64,
    pub lakes_warm: f64,
    /// The longest flow path, divide to mouth, km.
    pub longest_km: f64,
    /// Hack's exponent: length ∝ area^h over every catchment of 64+ cells
    /// (Earth 0.5–0.6).
    pub hack: f64,
    /// Cells carrying a stream (0.05+ m³/s) per land cell, by aridity
    /// index: under 0.5, 0.5 to 1, over 1.
    pub density: [f64; 3],
    /// The largest discharge, m³/s.
    pub largest: f64,
}

#[cfg(test)]
impl DrainageMap {
    pub(crate) fn stats(&self, climate: &ClimateMap) -> NetworkStats {
        let (nx, nz) = (self.nx, self.nz);
        let n = nx * nz;
        let diagonal = self.cell_x.hypot(self.cell_z);
        // Area and longest path, donors first (Kahn's order on receivers).
        let mut donors = vec![0u32; n];
        for c in 0..n {
            let r = self.receiver_of(c);
            if r != c {
                donors[r] += 1;
            }
        }
        let mut ready: Vec<usize> = (0..n).filter(|&c| donors[c] == 0).collect();
        let (mut area, mut length) = (vec![1u32; n], vec![0.0f64; n]);
        while let Some(c) = ready.pop() {
            let r = self.receiver_of(c);
            if r == c {
                continue;
            }
            let step = if c % nx == r % nx {
                self.cell_z
            } else if c / nx == r / nx {
                self.cell_x
            } else {
                diagonal
            };
            area[r] += area[c];
            length[r] = length[r].max(length[c] + step);
            donors[r] -= 1;
            if donors[r] == 0 {
                ready.push(r);
            }
        }
        let endo = self.endorheic();
        let (mut land, mut closed, mut longest, mut largest) = (0u32, 0u32, 0.0f64, 0.0f64);
        let (mut cold, mut warm) = ((0u32, 0u32), (0u32, 0u32));
        let mut density = [(0u32, 0u32); 3];
        let (mut sx, mut sy, mut sxx, mut sxy, mut k) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for c in 0..n {
            if self.sea_at(c) {
                continue;
            }
            land += 1;
            closed += u32::from(endo[c]);
            let r = self.receiver_of(c);
            if r == c || self.sea_at(r) || self.sink_at(r) {
                longest = longest.max(length[c]);
            }
            let q = f64::from(self.discharge[c]);
            largest = largest.max(q);
            let x = ((c % nx) as f64 + 0.5) * self.cell_x;
            let z = ((c / nx) as f64 + 0.5) * self.cell_z;
            let (sea_level_c, precip) = climate.sample(x as i64, z as i64);
            let t = temperature_at(sea_level_c, i64::from(self.h(c).max(0)));
            let lake = u32::from(matches!(self.cover_at(c), Cover::Lake | Cover::SaltLake));
            let tally = if t < GLACIATED_BELOW_C {
                &mut cold
            } else {
                &mut warm
            };
            tally.0 += lake;
            tally.1 += 1;
            let pet = potential_evaporation(t);
            let zone = if pet <= 0.0 || precip >= pet {
                2
            } else if precip >= 0.5 * pet {
                1
            } else {
                0
            };
            density[zone].0 += u32::from(q >= 0.05);
            density[zone].1 += 1;
            if area[c] >= 64 && length[c] > 0.0 {
                let (lx, ly) = ((f64::from(area[c])).ln(), length[c].ln());
                sx += lx;
                sy += ly;
                sxx += lx * lx;
                sxy += lx * ly;
                k += 1.0;
            }
        }
        let share = |(a, b): (u32, u32)| f64::from(a) / f64::from(b.max(1));
        NetworkStats {
            endorheic: f64::from(closed) / f64::from(land),
            lakes_cold: share(cold),
            lakes_warm: share(warm),
            longest_km: longest / 1000.0,
            hack: (k * sxy - sx * sy) / (k * sxx - sx * sx),
            density: density.map(share),
            largest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(seed: u64) -> DrainageMap {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let e = Elevation::new(seed, shape);
        let c = ClimateMap::build(&e, shape);
        DrainageMap::build(&e, &c, shape)
    }

    /// Criterion 3: water runs downhill, always — every cell's receiver is a
    /// neighbour whose level is no higher, the sea and closed basins drain
    /// to themselves, and following receivers from any land cell reaches
    /// the sea or a closed basin.
    #[test]
    fn water_runs_downhill_to_the_sea() {
        let d = small(3);
        let (nx, nz) = d.cells();
        for j in 0..nz {
            for i in 0..nx {
                let c = j * nx + i;
                let r = d.receiver_of(c);
                if d.sea_at(c) {
                    assert_eq!(r, c, "the sea is its own outlet");
                    continue;
                }
                if d.sink_at(c) {
                    assert_eq!(r, c, "a closed basin is its own outlet");
                    assert!(d.lvl(c) >= d.h(c), "a closed basin above its ground");
                    continue;
                }
                assert!(neighbours(c, nx, nz).contains(&r), "receiver not adjacent");
                assert!(d.lvl(r) <= d.lvl(c), "water runs uphill at ({i}, {j})");
                if d.is_lake(i, j) {
                    assert!(d.lvl(c) > d.h(c), "a lake with no depth");
                } else {
                    // Above its ground only where sediment fills a hollow
                    // below a river's grade (stage 2 raises the floor).
                    assert!(
                        d.lvl(c) <= d.h(c) || d.cover_at(c) == Cover::Land,
                        "a river bed above its ground"
                    );
                }
                let mut at = c;
                for _ in 0..nx * nz {
                    if d.sea_at(at) || d.sink_at(at) {
                        break;
                    }
                    at = d.receiver_of(at);
                }
                assert!(
                    d.sea_at(at) || d.sink_at(at),
                    "({i}, {j}) never reaches the sea or a closed basin"
                );
            }
        }
    }

    /// Water is conserved: what the land sheds reaches the sea or a closed
    /// basin, less no more than the lakes on the way evaporate.
    #[test]
    fn every_drop_is_accounted_for() {
        let d = small(5);
        let n = d.height.len();
        let (runoff, demand) = &d.budget;
        let arrives: f64 = (0..n)
            .filter(|&c| {
                let r = d.receiver_of(c);
                !d.sea_at(c) && (d.sea_at(r) && r != c || d.sink_at(c))
            })
            .map(|c| f64::from(d.discharge[c]))
            .sum();
        let shed: f64 = runoff.iter().sum();
        let lakes: f64 = (0..n)
            .filter(|&c| d.cover_at(c) == Cover::Lake)
            .map(|c| demand[c])
            .sum();
        assert!(shed > 0.0);
        assert!(arrives <= shed * 1.001, "{arrives} arrives of {shed} shed");
        assert!(
            arrives >= (shed - lakes) * 0.999,
            "{arrives} arrives of {shed} shed less {lakes} evaporated"
        );
    }

    /// Criterion 4, on one world (the survey, `drainage_survey`, runs many):
    /// Hack's law holds, lakes are commonest on ice-scoured ground, streams
    /// densest where it is wet, and closed basins a minority of the land.
    #[test]
    fn the_network_is_earth_like() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let e = Elevation::new(3, shape);
        let climate = ClimateMap::build(&e, shape);
        let s = DrainageMap::build(&e, &climate, shape).stats(&climate);
        assert!((0.45..0.7).contains(&s.hack), "{s:?}");
        assert!(s.lakes_cold > 1.5 * s.lakes_warm, "{s:?}");
        assert!(s.density[0] < s.density[2], "{s:?}");
        assert!(s.endorheic < 0.5, "{s:?}");
    }

    /// Criterion 6: rivers sit in valleys that deepen downstream — the
    /// more water, the deeper the cut — and the valley sides erosion makes
    /// stay walkable.
    #[test]
    fn valleys_deepen_downstream_and_stay_walkable() {
        let d = small(11);
        let (nx, nz) = d.cells();
        // Valley depth: the ground three cells around, less the ground the
        // river runs on — by discharge, a decade at a time, where water runs.
        let mut bins = [(0.0f64, 0u32); 4];
        for j in 0..nz {
            for i in 0..nx {
                let c = j * nx + i;
                let q = d.discharge[c];
                if d.sea_at(c) || d.cover_at(c) != Cover::Land || q < 0.001 {
                    continue;
                }
                let mut around = 0.0f64;
                for (di, dj) in [
                    (3, 0),
                    (-3, 0),
                    (0, 3),
                    (0, -3),
                    (2, 2),
                    (-2, 2),
                    (2, -2),
                    (-2, -2),
                ] {
                    let ii = (i as isize + di).rem_euclid(nx as isize) as usize;
                    let jj = (j as isize + dj).rem_euclid(nz as isize) as usize;
                    around += f64::from(d.h(jj * nx + ii).max(0));
                }
                let b = if q < 0.01 {
                    0
                } else if q < 0.1 {
                    1
                } else if q < 1.0 {
                    2
                } else {
                    3
                };
                bins[b].0 += around / 8.0 - f64::from(d.h(c));
                bins[b].1 += 1;
            }
        }
        let mean: Vec<f64> = bins
            .iter()
            .map(|(s, k)| s / f64::from((*k).max(1)))
            .collect();
        eprintln!("valley depth by discharge decade: {mean:?} over {bins:?}");
        // Every river sits in a valley, and a stream's is deeper than a
        // rill's. (Larger rivers' valleys are not much deeper again, ~11–13
        // blocks: criterion 6 records it as open.)
        assert!(
            mean.iter().all(|&m| m > 0.0),
            "a river above its surroundings: {mean:?}"
        );
        assert!(mean[1..].iter().all(|&m| m > 2.0 * mean[0]), "{mean:?}");
        // The steepest slope the cutting adds, along both axes, over one
        // 16-block step: no worse than a steep hillside.
        let (cw, _) = d.cell_blocks();
        let span = (nx as f64 * cw) as i64;
        let mut steepest = 0.0f32;
        for z in (0..span).step_by(173) {
            for x in (0..span).step_by(16) {
                let (xf, zf) = (x as f64, z as f64);
                let here = d.erosion_at(xf, zf);
                steepest = steepest
                    .max((d.erosion_at(xf + 16.0, zf) - here).abs() / 16.0)
                    .max((d.erosion_at(xf, zf + 16.0) - here).abs() / 16.0);
            }
        }
        eprintln!("steepest slope erosion adds: {steepest:.2}");
        assert!(steepest < 1.0, "erosion adds a slope of {steepest:.2}");
    }

    /// Criterion 10: rivers differ as Earth's do — every kind appears;
    /// torrents run higher than meandering rivers; braided rivers only on
    /// arid or ice-scoured ground, wadis only on arid.
    #[test]
    fn rivers_differ_by_slope_discharge_and_sediment() {
        let d = small(11);
        let (nx, nz) = d.cells();
        let mut tally = std::collections::HashMap::new();
        for j in 0..nz {
            for i in 0..nx {
                let Some(kind) = d.river_kind_at(i, j) else {
                    continue;
                };
                let c = j * nx + i;
                let e = tally.entry(kind).or_insert((0u32, 0i64));
                e.0 += 1;
                e.1 += i64::from(d.h(c));
                match kind {
                    RiverKind::Braided => assert!(d.arid_at(c) || d.cold_at(c)),
                    RiverKind::Wadi => assert!(d.arid_at(c)),
                    _ => {}
                }
            }
        }
        eprintln!("river kinds (cells, mean height): {tally:?}");
        for kind in [
            RiverKind::Torrent,
            RiverKind::Braided,
            RiverKind::Meandering,
            RiverKind::Wadi,
        ] {
            assert!(tally.contains_key(&kind), "no {kind:?}: {tally:?}");
        }
        let mean = |k| {
            let (n, sum) = tally[&k];
            sum as f64 / f64::from(n)
        };
        assert!(
            mean(RiverKind::Torrent) > mean(RiverKind::Meandering),
            "{tally:?}"
        );
    }

    #[test]
    fn the_network_is_deterministic() {
        let (a, b) = (small(9), small(9));
        assert_eq!(a.flow, b.flow);
        assert_eq!(a.level, b.level);
        assert!(
            a.discharge
                .iter()
                .zip(&b.discharge)
                .all(|(x, y)| x.to_bits() == y.to_bits())
        );
    }
}
