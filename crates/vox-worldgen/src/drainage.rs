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
//!    becomes a gorge. Every land cell drains, in the end, to the sea.
//! 4. **Accumulate discharge**: each cell's runoff — precipitation less
//!    evaporation (ADR-0017's climate; evaporation estimated by Köppen's
//!    aridity threshold, as the biome classifier judges dryness) — summed
//!    down the receiver tree. Discharge is in m³/s, honestly from our areas
//!    and Earth's rainfall; widths and depths are scaled from it for play.
//!
//! Deterministic on every machine: integer heights, ties broken by cell
//! index, one sequential flood, float sums taken in a fixed order.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rayon::prelude::*;
use vox_core::WorldShape;

use crate::biome::aridity_threshold;
use crate::climate::{ClimateMap, temperature_at};
use crate::elevation::Elevation;

mod hollows;
pub use hollows::Cover;

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
/// Evaporation from open water, as a multiple of Köppen's aridity threshold
/// (`20·T + 140` mm, the dryness the biome classifier judges land by): ~280
/// mm a year at 0 °C, ~700 at 10 °C, ~1 300 at 25 °C — Earth's lake
/// evaporation runs from a few hundred mm in the boreal to 1 500–2 000 over
/// hot deserts' salt lakes (the Dead Sea, Lake Chad).
const LAKE_EVAPORATION: f32 = 2.0;
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

/// The world's drainage, on a grid of cells, row-major (`j * nx + i`).
pub struct DrainageMap {
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
    /// Ground at the cell's centre, blocks.
    height: Vec<i32>,
    /// The water surface here if it is a lake, else the river bed: the
    /// ground, cut down where a river breaches a hollow's rim. Never rises
    /// downstream.
    spill: Vec<i32>,
    /// Whether the cell is sea: below sea level and part of an ocean.
    sea: Vec<bool>,
    /// What covers the cell.
    cover: Vec<Cover>,
    /// Whether water ends here without reaching the sea: a closed basin.
    sink: Vec<bool>,
    /// The cell this one drains into; itself for the sea.
    receiver: Vec<u32>,
    /// Water leaving the cell, m³/s.
    discharge: Vec<f32>,
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
        let cells = |size: i64| (size / CELL_TARGET).clamp(MIN_CELLS, MAX_CELLS) as usize;
        let (nx, nz) = (cells(shape.size_x()), cells(shape.size_z()));
        let cell_x = shape.size_x() as f64 / nx as f64;
        let cell_z = shape.size_z() as f64 / nz as f64;
        let centre = |c: usize| {
            let (i, j) = (c % nx, c / nx);
            (
                ((i as f64 + 0.5) * cell_x).floor() as i64,
                ((j as f64 + 0.5) * cell_z).floor() as i64,
            )
        };
        let height: Vec<i32> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                elevation.height(x, z) as i32
            })
            .collect();
        let sea = seas(nx, nz, &height);
        // Runoff per cell, and what a lake over it would cost: its
        // evaporation less the rain on it, plus the runoff it no longer
        // sheds — always positive. Both m³/s.
        let per_mm = cell_x * cell_z / 1000.0 / SECONDS_PER_YEAR;
        let (runoff, demand): (Vec<f64>, Vec<f64>) = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                if sea[c] {
                    return (0.0, 0.0);
                }
                let (x, z) = centre(c);
                let (sea_level_c, precip) = climate.sample(x, z);
                let t = temperature_at(sea_level_c, i64::from(height[c].max(0)));
                let threshold = aridity_threshold(t);
                let runoff = (precip - threshold).max(0.0);
                let evaporation = LAKE_EVAPORATION * threshold;
                (
                    f64::from(runoff) * per_mm,
                    f64::from(runoff + evaporation - precip) * per_mm,
                )
            })
            .unzip();
        let cold: Vec<bool> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                let (sea_level_c, _) = climate.sample(x, z);
                temperature_at(sea_level_c, i64::from(height[c].max(0))) < GLACIATED_BELOW_C
            })
            .collect();
        let held = hollows::settle(nx, nz, &height, &sea, &cold, &runoff, &demand);
        let (surface, mut cover, sink) = (held.surface, held.cover, held.sink);
        let lake: Vec<bool> = cover.iter().map(|&c| c == Cover::Lake).collect();
        let (receiver, order) = flood(nx, nz, &surface, &sea, &sink, &lake);
        let still: Vec<bool> = (0..nx * nz).map(|c| lake[c] || sink[c]).collect();
        let spill = channel_levels(&surface, &receiver, &order, &still);
        // A cut can end below a lake it flows into: that water is the
        // lake's backwater.
        for c in 0..nx * nz {
            if cover[c] == Cover::Land && spill[c] > height[c] {
                cover[c] = Cover::Lake;
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
        Self {
            nx,
            nz,
            cell_x,
            cell_z,
            height,
            spill,
            sea,
            cover,
            sink,
            receiver,
            discharge,
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
        let r = self.receiver[c] as usize;
        (
            self.height[c],
            self.spill[c],
            self.discharge[c],
            (r % self.nx, r / self.nx),
        )
    }

    /// Whether the cell is sea.
    pub fn is_sea(&self, i: usize, j: usize) -> bool {
        self.sea[j * self.nx + i]
    }

    /// Whether the cell is under a lake, fresh or salt.
    pub fn is_lake(&self, i: usize, j: usize) -> bool {
        matches!(self.cover(i, j), Cover::Lake | Cover::SaltLake)
    }

    /// What covers the cell.
    pub fn cover(&self, i: usize, j: usize) -> Cover {
        self.cover[j * self.nx + i]
    }

    /// Whether water ends in the cell without reaching the sea.
    pub fn is_sink(&self, i: usize, j: usize) -> bool {
        self.sink[j * self.nx + i]
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
                let r = self.receiver[at] as usize;
                if r == at {
                    end[at] = if self.sink[at] { 2 } else { 1 };
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

/// The level of the water in every cell: a lake's surface, and elsewhere
/// the ground cut down along the flow so it never rises downstream — a
/// breached rim becomes the river's channel.
fn channel_levels(surface: &[i32], receiver: &[u32], order: &[u32], lake: &[bool]) -> Vec<i32> {
    let mut level = surface.to_vec();
    // Donors come later in flood order than their receivers, so walking the
    // order backwards lowers each receiver to its lowest donor before the
    // water moves on. Lake surfaces are held at their spill.
    for &c in order.iter().rev() {
        let c = c as usize;
        let r = receiver[c] as usize;
        if r != c && !lake[r] && level[r] > level[c] {
            level[r] = level[c];
        }
    }
    // A cut cannot go below the water it flows into: walking outlets first,
    // raise each cell to its receiver's level. What rises above its ground
    // is the lake's backwater.
    for &c in order {
        let c = c as usize;
        let r = receiver[c] as usize;
        if r != c && level[c] < level[r] {
            level[c] = level[r];
        }
    }
    level
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
            heap.push(Reverse((key, surface[c], c as u32)));
        }
    }
    if heap.is_empty() {
        // A world with no sea (none is legal, but be total): its lowest
        // cell is the outlet.
        let lowest = (0..n).min_by_key(|&c| (surface[c], c)).unwrap();
        receiver[lowest] = lowest as u32;
        heap.push(Reverse((surface[lowest], surface[lowest], lowest as u32)));
    }
    while let Some(Reverse((level, _, c))) = heap.pop() {
        order.push(c);
        for nb in neighbours(c as usize, nx, nz) {
            if receiver[nb] != u32::MAX {
                continue;
            }
            receiver[nb] = c;
            let low = if lake[nb] { i32::MAX } else { surface[nb] };
            heap.push(Reverse((surface[nb].max(level), low, nb as u32)));
        }
    }
    (receiver, order)
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
                let r = d.receiver[c] as usize;
                if d.sea[c] {
                    assert_eq!(r, c, "the sea is its own outlet");
                    continue;
                }
                if d.sink[c] {
                    assert_eq!(r, c, "a closed basin is its own outlet");
                    assert!(d.spill[c] >= d.height[c], "a closed basin above its ground");
                    continue;
                }
                assert!(neighbours(c, nx, nz).contains(&r), "receiver not adjacent");
                assert!(d.spill[r] <= d.spill[c], "water runs uphill at ({i}, {j})");
                if d.is_lake(i, j) {
                    assert!(d.spill[c] > d.height[c], "a lake with no depth");
                } else {
                    assert!(d.spill[c] <= d.height[c], "a river bed above its ground");
                }
                let mut at = c;
                for _ in 0..nx * nz {
                    if d.sea[at] || d.sink[at] {
                        break;
                    }
                    at = d.receiver[at] as usize;
                }
                assert!(
                    d.sea[at] || d.sink[at],
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
                let r = d.receiver[c] as usize;
                !d.sea[c] && (d.sea[r] && r != c || d.sink[c])
            })
            .map(|c| f64::from(d.discharge[c]))
            .sum();
        let shed: f64 = runoff.iter().sum();
        let lakes: f64 = (0..n)
            .filter(|&c| d.cover[c] == Cover::Lake)
            .map(|c| demand[c])
            .sum();
        assert!(shed > 0.0);
        assert!(arrives <= shed * 1.001, "{arrives} arrives of {shed} shed");
        assert!(
            arrives >= (shed - lakes) * 0.999,
            "{arrives} arrives of {shed} shed less {lakes} evaporated"
        );
    }

    /// Criterion 4, in part: lakes are most common on ice-scoured ground,
    /// as on Earth (the boreal and tundra lake districts).
    #[test]
    fn lakes_are_commonest_where_ice_scoured() {
        let shape = WorldShape::new(65_536, 65_536).unwrap();
        let e = Elevation::new(3, shape);
        let climate = ClimateMap::build(&e, shape);
        let d = DrainageMap::build(&e, &climate, shape);
        let (nx, nz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        let (mut cold, mut cold_lake, mut warm, mut warm_lake) = (0u32, 0u32, 0u32, 0u32);
        for j in 0..nz {
            for i in 0..nx {
                let c = j * nx + i;
                if d.sea[c] {
                    continue;
                }
                let x = ((i as f64 + 0.5) * cw) as i64;
                let z = ((j as f64 + 0.5) * ch) as i64;
                let (t0, _) = climate.sample(x, z);
                let lake = u32::from(d.is_lake(i, j));
                if temperature_at(t0, i64::from(d.height[c].max(0))) < GLACIATED_BELOW_C {
                    cold += 1;
                    cold_lake += lake;
                } else {
                    warm += 1;
                    warm_lake += lake;
                }
            }
        }
        let (fc, fw) = (
            f64::from(cold_lake) / f64::from(cold),
            f64::from(warm_lake) / f64::from(warm),
        );
        assert!(
            fc > 1.5 * fw,
            "glaciated lakes {fc:.3} vs elsewhere {fw:.3}"
        );
    }

    #[test]
    fn the_network_is_deterministic() {
        let (a, b) = (small(9), small(9));
        assert_eq!(a.receiver, b.receiver);
        assert_eq!(a.spill, b.spill);
        assert!(
            a.discharge
                .iter()
                .zip(&b.discharge)
                .all(|(x, y)| x.to_bits() == y.to_bits())
        );
    }
}
