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
    /// Whether the cell is under a lake.
    lake: Vec<bool>,
    /// The cell this one drains into; itself for the sea.
    receiver: Vec<u32>,
    /// Water leaving the cell, m³/s.
    discharge: Vec<f32>,
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
        // Runoff per cell, m³/s.
        let area = cell_x * cell_z;
        let runoff: Vec<f32> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                if sea[c] {
                    return 0.0;
                }
                let (x, z) = centre(c);
                let (sea_level_c, precip) = climate.sample(x, z);
                let t = temperature_at(sea_level_c, i64::from(height[c].max(0)));
                let mm = (precip - aridity_threshold(t)).max(0.0);
                (f64::from(mm) / 1000.0 * area / SECONDS_PER_YEAR) as f32
            })
            .collect();

        let cold: Vec<bool> = (0..nx * nz)
            .into_par_iter()
            .map(|c| {
                let (x, z) = centre(c);
                let (sea_level_c, _) = climate.sample(x, z);
                temperature_at(sea_level_c, i64::from(height[c].max(0))) < GLACIATED_BELOW_C
            })
            .collect();
        let (surface, lake) = lakes(nx, nz, &height, &sea, &cold);
        let (receiver, order) = flood(nx, nz, &surface, &sea, &lake);
        let spill = channel_levels(&surface, &receiver, &order, &lake);
        let lake: Vec<bool> = (0..nx * nz)
            .map(|c| !sea[c] && spill[c] > height[c])
            .collect();

        // Downstream accumulation in reverse flood order: every cell is
        // reached after its receiver, so reversed, every cell is summed
        // before the water moves on.
        let mut discharge = runoff;
        for &c in order.iter().rev() {
            let r = receiver[c as usize] as usize;
            if r != c as usize {
                discharge[r] += discharge[c as usize];
            }
        }
        Self {
            nx,
            nz,
            cell_x,
            cell_z,
            height,
            spill,
            sea,
            lake,
            receiver,
            discharge,
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

    /// Whether the cell is under a lake.
    pub fn is_lake(&self, i: usize, j: usize) -> bool {
        self.lake[j * self.nx + i]
    }
}

/// Which hollows hold lakes, and their surfaces: returns the water surface
/// per cell (the ground where there is no lake) and which cells are lake.
///
/// The hollows nest — a deep basin inside a broad shallow one inside a
/// plain — so they are found as a merge tree (a depression hierarchy,
/// Barnes et al., 2020): cells taken lowest first, each joining the
/// neighbours already taken; where two undrained groups meet, both are
/// full to that saddle, and they fill on together as one. A group that
/// meets the sea (or anything drained) spills. Then from the outermost
/// hollow in: a hollow kept by [`LAKE_MIN_MEAN_DEPTH`]'s rule is a lake to
/// its spill level, everything inside it included; a breached one leaves
/// each hollow nested in it to be judged on its own.
fn lakes(
    nx: usize,
    nz: usize,
    height: &[i32],
    sea: &[bool],
    cold: &[bool],
) -> (Vec<i32>, Vec<bool>) {
    const NONE: u32 = u32::MAX;
    struct Hollow {
        /// Where it spills or joins another; `i32::MAX` while it never does.
        spill: i32,
        cells: i64,
        sum_height: i64,
        deepest: i32,
        cold: i64,
        parent: u32,
    }
    let n = nx * nz;
    // Union-find over cells; index `n` is the sea, always drained.
    let mut up: Vec<u32> = (0..=n as u32).collect();
    fn root(up: &mut [u32], mut c: u32) -> u32 {
        while up[c as usize] != c {
            let next = up[up[c as usize] as usize];
            up[c as usize] = next;
            c = next;
        }
        c
    }
    // Per group root: the hollow it is filling, or NONE once drained.
    let mut filling = vec![NONE; n + 1];
    // Per cell: the hollow it joined.
    let mut joined = vec![NONE; n];
    let mut hollows: Vec<Hollow> = Vec::new();
    let mut taken = vec![false; n];
    for c in 0..n {
        if sea[c] {
            up[c] = n as u32;
            taken[c] = true;
        }
    }
    let mut land: Vec<u32> = (0..n as u32).filter(|&c| !sea[c as usize]).collect();
    land.sort_unstable_by_key(|&c| (height[c as usize], c));
    for &c in &land {
        let c = c as usize;
        let h = height[c];
        taken[c] = true;
        let mut mine = NONE; // this cell's group root, once it has one
        for nb in neighbours(c, nx, nz) {
            if !taken[nb] || nb == c {
                continue;
            }
            let theirs = root(&mut up, nb as u32);
            if mine == NONE {
                // Join the first group: extend it.
                up[c] = theirs;
                mine = theirs;
                continue;
            }
            if theirs == mine {
                continue;
            }
            // Two groups meet at this cell's height.
            let (a, b) = (filling[mine as usize], filling[theirs as usize]);
            for x in [a, b] {
                if x != NONE {
                    hollows[x as usize].spill = h;
                }
            }
            let merged = if a == NONE || b == NONE {
                NONE // either drains: both now drain
            } else {
                let id = hollows.len() as u32;
                let (ha, hb) = (&hollows[a as usize], &hollows[b as usize]);
                let joined_hollow = Hollow {
                    spill: i32::MAX,
                    cells: ha.cells + hb.cells,
                    sum_height: ha.sum_height + hb.sum_height,
                    deepest: ha.deepest.min(hb.deepest),
                    cold: ha.cold + hb.cold,
                    parent: NONE,
                };
                hollows[a as usize].parent = id;
                hollows[b as usize].parent = id;
                hollows.push(joined_hollow);
                id
            };
            // The sea's root stays the root, so it is always found drained.
            let (keep, gone) = if theirs as usize == n {
                (theirs, mine)
            } else {
                (mine, theirs)
            };
            up[gone as usize] = keep;
            filling[keep as usize] = merged;
            mine = keep;
        }
        if mine == NONE {
            // A pit: a new hollow.
            hollows.push(Hollow {
                spill: i32::MAX,
                cells: 0,
                sum_height: 0,
                deepest: h,
                cold: 0,
                parent: NONE,
            });
            filling[c] = (hollows.len() - 1) as u32;
            mine = c as u32;
        }
        let x = filling[mine as usize];
        joined[c] = x;
        if x != NONE {
            let hl = &mut hollows[x as usize];
            hl.cells += 1;
            hl.sum_height += i64::from(h);
            hl.cold += i64::from(cold[c]);
        }
    }
    // Hollows are created before their parents, so walking them backwards
    // settles every parent before its children.
    let mut level = vec![i32::MIN; hollows.len()];
    for id in (0..hollows.len()).rev() {
        let hl = &hollows[id];
        if hl.parent != NONE && level[hl.parent as usize] != i32::MIN {
            level[id] = level[hl.parent as usize];
            continue;
        }
        if hl.spill == i32::MAX {
            continue; // never fills (a world without sea): not a lake
        }
        // Statistics at the spill level, over everything inside.
        let volume = hl.cells * i64::from(hl.spill) - hl.sum_height;
        let basin = volume >= LAKE_MIN_MEAN_DEPTH * hl.cells && hl.cells >= LAKE_MIN_CELLS as i64;
        let glaciated = hl.cold * 2 >= hl.cells;
        let tarn = glaciated
            && hl.spill - hl.deepest >= LAKE_MIN_DEPTH_GLACIATED
            && (LAKE_MIN_CELLS_GLACIATED as i64..=LAKE_MAX_CELLS_GLACIATED as i64)
                .contains(&hl.cells);
        if basin || tarn {
            level[id] = hl.spill;
        }
    }
    let mut surface = height.to_vec();
    let mut lake = vec![false; n];
    for c in 0..n {
        let x = joined[c];
        if x != NONE && level[x as usize] > height[c] {
            surface[c] = level[x as usize];
            lake[c] = true;
        }
    }
    (surface, lake)
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

/// Priority flood from the sea over the water surface: receivers, and the
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
    lake: &[bool],
) -> (Vec<u32>, Vec<u32>) {
    let n = nx * nz;
    let mut receiver = vec![u32::MAX; n];
    let mut order = Vec::with_capacity(n);
    let mut heap = BinaryHeap::with_capacity(n / 4);
    for c in 0..n {
        if sea[c] {
            receiver[c] = c as u32;
            heap.push(Reverse((0i32, surface[c], c as u32)));
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
    /// neighbour whose spill level is no higher, the sea drains to itself,
    /// and following receivers from any land cell reaches the sea.
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
                assert!(neighbours(c, nx, nz).contains(&r), "receiver not adjacent");
                assert!(d.spill[r] <= d.spill[c], "water runs uphill at ({i}, {j})");
                if d.lake[c] {
                    assert!(d.spill[c] > d.height[c], "a lake with no depth");
                } else {
                    assert!(d.spill[c] <= d.height[c], "a river bed above its ground");
                }
                let mut at = c;
                for _ in 0..nx * nz {
                    if d.sea[at] {
                        break;
                    }
                    at = d.receiver[at] as usize;
                }
                assert!(d.sea[at], "({i}, {j}) never reaches the sea");
            }
        }
    }

    /// Discharge is conserved: everything the land sheds arrives at the sea.
    #[test]
    fn every_drop_reaches_the_sea() {
        let d = small(5);
        let n = d.height.len();
        let into_sea: f64 = (0..n)
            .filter(|&c| !d.sea[c] && d.sea[d.receiver[c] as usize])
            .map(|c| f64::from(d.discharge[c]))
            .sum();
        // Runoff over all land, recomputed: discharge of land cells minus
        // what flowed in from upstream.
        let mut inflow = vec![0.0f64; n];
        for c in 0..n {
            let r = d.receiver[c] as usize;
            if r != c {
                inflow[r] += f64::from(d.discharge[c]);
            }
        }
        let runoff: f64 = (0..n)
            .filter(|&c| !d.sea[c])
            .map(|c| f64::from(d.discharge[c]) - inflow[c])
            .sum();
        assert!(runoff > 0.0);
        assert!(
            ((into_sea - runoff) / runoff).abs() < 1e-3,
            "{into_sea} vs {runoff}"
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
                let lake = u32::from(d.lake[c]);
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
