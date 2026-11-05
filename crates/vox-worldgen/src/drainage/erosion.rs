//! Valley erosion (M14 task 2): rivers cut the ground they run on.
//!
//! Stream-power incision — the rate a river lowers its bed grows with its
//! discharge and its slope, `E = K·Q^m·S` — solved implicitly, outlets
//! first, as Braun & Willett (2013) do: each cell relaxes toward its
//! receiver, already lowered this pass, so a knickpoint runs upstream in one
//! sweep and no step is ever unstable. `m = ½` (the usual exponent, and a
//! square root is exact on every machine). A few passes, re-routing between
//! them.
//!
//! **Discharge, not area, drives it**: a river's power is the water it
//! carries, so the wet tropics and the mountains' windward sides are cut
//! deep, deserts hardly at all — and their closed basins survive, as
//! Earth's do (the Great Basin, the Tarim, the Sahara's depressions). A
//! hollow on wet ground is cut through at its rim instead, the river
//! draining it; erosion never fills, and never lowers a bed below the one
//! it flows into, nor below sea level at the coast.
//!
//! **Walls retreat**: where a cut leaves a valley side steeper than
//! [`MAX_WALL_SLOPE`] (and steeper than the ground was), the side is
//! lowered to it, as hillslopes fail and wash into the river — so valleys
//! widen as they deepen and stay walkable (ADR-0010). Slopes erosion did
//! not make are left alone: mountains keep their own.
//!
//! It runs on a grid half the drainage grid's resolution (256-block cells
//! on the default world), four times cheaper; what it returns is the
//! change, interpolated: the elevation's erosion stage, read by the
//! drainage grid and by chunks and the LOD alike, while climate keeps
//! reading the base (ADR-0017).

use super::flood;

/// Passes of route-then-incise.
const PASSES: usize = 8;
/// Incision per pass for a river of [`Q_REF`] m³/s over a [`STEP_REF`]
/// step: the fraction of its drop to its receiver it loses. Smaller rivers
/// cut as the square root of their discharge.
const STRENGTH: f64 = 0.1;
/// A small stream's discharge: ~40 drainage cells of temperate runoff
/// (~0.7 km²). Our discharges are honest for our areas, which are small.
const Q_REF: f64 = 0.01;
const STEP_REF: f64 = 256.0;
/// The steepest valley side erosion leaves, rise over run (~31°).
const MAX_WALL_SLOPE: f64 = 0.6;
/// Canyons (M15): where a caprock holds the walls up — flash-flood
/// uplands, by weight — rivers cut harder and the walls stand steeper, so a
/// river crossing a plateau cuts a canyon rather than opening a valley
/// (the Colorado through its plateau). Incision multiplied by up to
/// `1 + CANYON_CUT`; sides up to [`CANYON_WALL_SLOPE`] (~72°).
const CANYON_CUT: f64 = 3.0;
const CANYON_WALL_SLOPE: f64 = 3.0;

/// Glacial troughs (M15 task 3, ADR-0021 §4 step 3): where ice lay in the
/// last ice age, glaciers followed the valleys and carved them into U's.
/// Ice weight from which a cell carries ice, and from which it is full.
const ICE_FROM: f32 = 0.5;
/// Ice flux (glaciated drainage cells upstream, by weight) below which a
/// cell is a valley head, not a trough.
const ICE_FLUX_FROM: f64 = 4.0;
/// Trough depth: blocks per √flux, the most, and the share of the valley's
/// own relief (the 5×5 cells' highest ground above it) it may reach — ice
/// deepens valleys in proportion to the relief that channels it; on a
/// lowland ice sheet it only scours (the lake districts).
const TROUGH_DEPTH_PER_ROOT: f64 = 8.0;
const TROUGH_MAX_DEPTH: f64 = 400.0;
const TROUGH_RELIEF_SHARE: f64 = 0.6;
/// Trough half-width: blocks per flux^¼ (floors ~100–400 wide).
const TROUGH_WIDTH_PER_QUARTER: f64 = 30.0;
/// The relief (the rim above the valley, blocks) over which ice comes to
/// be channelled into a trough, and the flux from which a trough is full.
const TROUGH_RELIEF: (f64, f64) = (100.0, 250.0);
const TROUGH_FLUX_FULL: f64 = 16.0;
/// Shallower than this, no trough is kept, blocks.
const TROUGH_MIN_DEPTH: f64 = 3.0;
/// The widest a trough's half-width gets, blocks.
const TROUGH_MAX_HALF: f64 = 200.0;
/// A trough meeting the sea is cut to this share of its depth there: the
/// fjord's sill.
const SILL_SHARE: f64 = 0.3;

/// The erosion stage: the change erosion made, on its own grid.
pub(super) struct Erosion {
    pub(super) nx: usize,
    pub(super) nz: usize,
    pub(super) cell_x: f64,
    pub(super) cell_z: f64,
    /// Blocks, never positive, at each cell's centre.
    pub(super) delta: Vec<f32>,
    /// Glacial troughs (M15): one per glaciated drainage cell, a U-shaped
    /// surface along the line from its centre to its receiver's — the
    /// ground is the lower of its own and the nearest troughs'.
    pub(super) troughs: Troughs,
}

/// The glacial troughs, on the drainage grid (M15 task 3). Kept as lines,
/// not a grid of heights: a 128-block grid interpolated showed through as
/// squares (M14's lesson); a U about a line follows the valley.
#[derive(Default)]
pub(super) struct Troughs {
    /// Drainage cells per axis, blocks per cell.
    pub(super) grid: (usize, usize, f64, f64),
    /// Per drainage cell: its trough's index in `cells`, or `u32::MAX`.
    pub(super) index: Vec<u32>,
    /// Per drainage cell: whether any trough is within [`SEARCH`] cells.
    pub(super) near: Vec<bool>,
    pub(super) cells: Vec<Trough>,
}

/// One trough cell, the line from its centre to its receiver's: the
/// floor's level, the depth cut, the half-width and the rim (the ground
/// the U widens out to) at its centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Trough {
    pub(super) cell: u32,
    pub(super) receiver: u32,
    pub(super) floor: f32,
    pub(super) depth: f32,
    pub(super) half: f32,
    pub(super) rim: f32,
    /// Whether no trough flows into it: a glacier's head, where the trough
    /// tapers in from nothing (round caps there drew bowls on every
    /// hillside a short glacier began on).
    pub(super) head: bool,
}

/// How many cells around a column its troughs are looked for.
const SEARCH: i64 = 3;
/// A trough's walls beyond its floor's half-width, rise over run, and the
/// last stretch before the search's edge, blocks, over which they rise
/// past any ground — so no trough ever ends in a step where the search
/// stops looking.
const TROUGH_WALL: f32 = 3.0;
const TROUGH_EDGE: f32 = 32.0;

/// A trough's height above its floor at `d` blocks from its line: a
/// parabola over the floor (`depth` at `half`), steep walls beyond, and a
/// rise past any ground in the last [`TROUGH_EDGE`] blocks of `reach`.
fn u_profile(d: f32, half: f32, depth: f32, reach: f32) -> f32 {
    let e = (d / half).min(1.0);
    let mut h = depth * e * e + TROUGH_WALL * (d - half).max(0.0);
    let edge = d - (reach - TROUGH_EDGE);
    if edge > 0.0 {
        h += 100.0 * edge;
    }
    h
}

impl Erosion {
    /// Erode the drainage grid's ground (`fnx`×`fnz` cells of `cell`
    /// blocks): `base` heights, the `sea`, each cell's `runoff`, m³/s, and
    /// its `canyon` weight and its `ice` weight (`[0, 1]`, M15).
    #[allow(clippy::too_many_arguments)] // the drainage grid's fields, each its own
    pub(super) fn build(
        fnx: usize,
        fnz: usize,
        cell: (f64, f64),
        base: &[i32],
        sea: &[bool],
        runoff: &[f64],
        canyon: &[f32],
        ice: &[f32],
    ) -> Self {
        // Two by two drainage cells to one: the mean height, the summed
        // runoff, sea where most of it is.
        let (nx, nz) = (fnx / 2, fnz / 2);
        let n = nx * nz;
        let fine = |i: usize, j: usize, di: usize, dj: usize| (2 * j + dj) * fnx + 2 * i + di;
        let mut h = vec![0.0f64; n];
        let mut wet = vec![false; n];
        let mut shed = vec![0.0f64; n];
        let mut cap = vec![0.0f64; n];
        for j in 0..nz {
            for i in 0..nx {
                let c = j * nx + i;
                let (mut sum, mut seas) = (0i64, 0);
                for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let f = fine(i, j, di, dj);
                    sum += i64::from(base[f]);
                    seas += usize::from(sea[f]);
                    shed[c] += runoff[f];
                    cap[c] += 0.25 * f64::from(canyon[f]);
                }
                h[c] = sum as f64 / 4.0;
                wet[c] = seas >= 2;
            }
        }
        let start = h.clone();
        let (cell_x, cell_z) = (cell.0 * 2.0, cell.1 * 2.0);
        let diagonal = (cell_x * cell_x + cell_z * cell_z).sqrt(); // sqrt, not hypot: exact everywhere
        let step = |a: usize, b: usize| {
            if a % nx == b % nx {
                cell_z
            } else if a / nx == b / nx {
                cell_x
            } else {
                diagonal
            }
        };
        let none = vec![false; n];
        let mut surface = vec![0i32; n];
        let mut q = vec![0.0f64; n];
        for _ in 0..PASSES {
            for c in 0..n {
                surface[c] = h[c].round() as i32;
            }
            let (receiver, order) = flood(nx, nz, &surface, &wet, &none, &none);
            q.copy_from_slice(&shed);
            for &c in order.iter().rev() {
                let r = receiver[c as usize] as usize;
                if r != c as usize {
                    q[r] += q[c as usize];
                }
            }
            for &c in &order {
                let c = c as usize;
                if wet[c] {
                    continue;
                }
                let r = receiver[c] as usize;
                let floor = if wet[r] { 0.0 } else { h[r] };
                if h[c] <= floor {
                    continue; // a hollow's floor: erosion does not fill
                }
                let f = STRENGTH
                    * (q[c] / Q_REF).sqrt()
                    * (STEP_REF / step(c, r))
                    * (1.0 + CANYON_CUT * cap[c]);
                h[c] = (h[c] + f * floor) / (1.0 + f);
            }
            retreat_walls(nx, nz, &mut h, &start, &wet, &cap, step);
        }
        let delta = (0..n).map(|c| (h[c] - start[c]).min(0.0) as f32).collect();
        let mut erosion = Self {
            nx,
            nz,
            cell_x,
            cell_z,
            delta,
            troughs: Troughs::default(),
        };
        let troughs = glaciate(fnx, fnz, cell, base, sea, ice, |x, z| erosion.at(x, z));
        erosion.troughs = troughs;
        erosion
    }

    /// The rivers' change at a point, blocks (never positive): bilinear
    /// between cell centres, on any lap of the world.
    pub(super) fn at(&self, x: f64, z: f64) -> f32 {
        bilinear(
            &self.delta,
            self.nx,
            self.nz,
            x / self.cell_x,
            z / self.cell_z,
        )
    }

    /// The ground at a point after erosion, from its ground `h` before:
    /// cut by the rivers, then the lower of that and any glacial trough's
    /// U (M15).
    pub(super) fn apply(&self, x: f64, z: f64, h: i64) -> i64 {
        let h = h + self.at(x, z).round() as i64;
        match self.troughs.at(x, z) {
            Some(t) => h.min(t.floor() as i64),
            None => h,
        }
    }
}

impl Troughs {
    /// The troughs from their cells (ascending), with the index and the
    /// search flags rebuilt.
    pub(super) fn from_cells(nx: usize, nz: usize, cx: f64, cz: f64, cells: Vec<Trough>) -> Self {
        let n = nx * nz;
        let mut index = vec![u32::MAX; n];
        let mut near = vec![false; n];
        let at = |i: i64, j: i64| {
            (j.rem_euclid(nz as i64) as usize) * nx + i.rem_euclid(nx as i64) as usize
        };
        for (k, t) in cells.iter().enumerate() {
            let c = t.cell as usize;
            index[c] = k as u32;
            let (i, j) = ((c % nx) as i64, (c / nx) as i64);
            for dj in -SEARCH..=SEARCH {
                for di in -SEARCH..=SEARCH {
                    near[at(i + di, j + dj)] = true;
                }
            }
        }
        Self {
            grid: (nx, nz, cx, cz),
            index,
            near,
            cells,
        }
    }

    /// The lowest trough surface at a point, blocks, if a trough reaches
    /// it: the U `floor + depth·(d / half)²` about each trough line near,
    /// out to its rim. Along a line its values run from the cell's to its
    /// receiver's — or, past the snout, taper to nothing.
    pub(super) fn at(&self, x: f64, z: f64) -> Option<f32> {
        if self.cells.is_empty() {
            return None;
        }
        let (nx, nz, cx, cz) = self.grid;
        let reach = SEARCH as f32 * cx.min(cz) as f32;
        let (fi, fj) = ((x / cx).floor() as i64, (z / cz).floor() as i64);
        let wrap = |i: i64, j: i64| {
            (j.rem_euclid(nz as i64) as usize) * nx + i.rem_euclid(nx as i64) as usize
        };
        if !self.near[wrap(fi, fj)] {
            return None;
        }
        // The shortest step from one cell index to another, across the seam.
        let step = |a: i64, b: i64, n: usize| {
            let d = (b - a).rem_euclid(n as i64);
            if d > n as i64 / 2 { d - n as i64 } else { d }
        };
        let mut best: Option<f32> = None;
        for dj in -SEARCH..=SEARCH {
            for di in -SEARCH..=SEARCH {
                let (ci, cj) = (fi + di, fj + dj);
                let k = self.index[wrap(ci, cj)];
                if k == u32::MAX {
                    continue;
                }
                let t = self.cells[k as usize];
                let r = t.receiver as usize;
                let (ri, rj) = ((r % nx) as i64, (r / nx) as i64);
                let (si, sj) = (step(ci, ri, nx), step(cj, rj, nz));
                // The line in this column's lap, between the network's own
                // jittered nodes, so the river runs on the trough's floor.
                let a = super::rivers::node(t.cell as usize, ci, cj, (cx, cz));
                let b = super::rivers::node(r, ci + si, cj + sj, (cx, cz));
                let (vx, vz) = (b.0 - a.0, b.1 - a.1);
                let len2 = vx * vx + vz * vz;
                let raw_u = if len2 > 0.0 {
                    ((x - a.0) * vx + (z - a.1) * vz) / len2
                } else {
                    0.0
                };
                let u = raw_u.clamp(0.0, 1.0);
                let (px, pz) = (a.0 + u * vx - x, a.1 + u * vz - z);
                let d = (px * px + pz * pz).sqrt() as f32;
                // Along the line, the ground before the ice and the depth
                // cut run from this cell's to its receiver's trough — or,
                // past the snout, the depth to nothing; the floor is the one
                // less the other, so a tapering trough rises to the ground
                // rather than leaving its floor deep.
                let next = self.index[r];
                let ground = t.floor + t.depth;
                let (to_ground, to_depth, to_half) = if next == u32::MAX {
                    (ground, 0.0, t.half)
                } else {
                    let n = self.cells[next as usize];
                    (n.floor + n.depth, n.depth, n.half)
                };
                let u = u as f32;
                let ground = ground + (to_ground - ground) * u;
                let mut depth = t.depth + (to_depth - t.depth) * u;
                if t.head {
                    // Nothing behind a glacier's head; it deepens from there.
                    depth *= if raw_u <= 0.0 { 0.0 } else { u };
                }
                if depth <= 0.0 {
                    continue;
                }
                let half = (t.half + (to_half - t.half) * u).max(1.0);
                let floor = ground - depth;
                let surface = floor + u_profile(d, half, depth, reach);
                if best.is_none_or(|b| surface < b) {
                    best = Some(surface);
                }
            }
        }
        best
    }
}

/// Bilinear between the centres of a wrapping `nx`×`nz` grid, at `(u, v)`
/// in cells.
fn bilinear(field: &[f32], nx: usize, nz: usize, u: f64, v: f64) -> f32 {
    let (u, v) = (u - 0.5, v - 0.5);
    let (fu, fv) = (u.floor(), v.floor());
    let (tu, tv) = ((u - fu) as f32, (v - fv) as f32);
    let wrap = |a: f64, n: usize| (a as i64).rem_euclid(n as i64) as usize;
    let (i0, j0) = (wrap(fu, nx), wrap(fv, nz));
    let (i1, j1) = ((i0 + 1) % nx, (j0 + 1) % nz);
    let d = |i: usize, j: usize| field[j * nx + i];
    let top = d(i0, j0) + (d(i1, j0) - d(i0, j0)) * tu;
    let bottom = d(i0, j1) + (d(i1, j1) - d(i0, j1)) * tu;
    top + (bottom - top) * tv
}

/// Find the glacial troughs on the drainage grid's ground (M15 task 3):
/// `base` heights cut by the rivers (`fluvial`), the `sea`, and each
/// cell's ice weight. Ice follows the valleys the rivers made, its flux
/// the glaciated catchment upstream; each trough cell's floor is cut to a
/// depth that grows with √flux (as a glacier's thickness does), its U as
/// wide as flux^¼, out to the valley's rim. Ice may cut below its outlet
/// (rivers may not): the basins it leaves are found by the routing that
/// follows and fill as ribbon lakes; a trough below sea level at the
/// coast is a fjord, its mouth cut shallower — the sill. A small valley
/// joining a large one is cut less deep and is left hanging.
fn glaciate(
    fnx: usize,
    fnz: usize,
    (cx, cz): (f64, f64),
    base: &[i32],
    sea: &[bool],
    ice: &[f32],
    fluvial: impl Fn(f64, f64) -> f32,
) -> Troughs {
    if ice.iter().all(|&w| w < ICE_FROM) {
        return Troughs::default();
    }
    let n = fnx * fnz;
    let centre = |c: usize| ((c % fnx) as f64 + 0.5, (c / fnx) as f64 + 0.5);
    let start: Vec<f64> = (0..n)
        .map(|c| {
            let (u, v) = centre(c);
            f64::from(base[c]) + f64::from(fluvial(u * cx, v * cz))
        })
        .collect();
    let surface: Vec<i32> = start.iter().map(|&h| h.round() as i32).collect();
    let none = vec![false; n];
    let (receiver, order) = flood(fnx, fnz, &surface, sea, &none, &none);
    // Ice flux, down the valleys while they stay glaciated (the snout is
    // where the weight ends).
    let mut flux: Vec<f64> = ice
        .iter()
        .map(|&w| if w >= ICE_FROM { f64::from(w) } else { 0.0 })
        .collect();
    for &c in order.iter().rev() {
        let (c, r) = (c as usize, receiver[c as usize] as usize);
        if r != c && flux[c] > 0.0 && ice[r] >= ICE_FROM {
            flux[r] += flux[c];
        }
    }
    let at = |i: i64, j: i64| {
        (j.rem_euclid(fnz as i64) as usize) * fnx + i.rem_euclid(fnx as i64) as usize
    };
    let mut cells = Vec::new();
    let mut kept = vec![false; n];
    let reach = SEARCH as f64 * cx.min(cz);
    for c in 0..n {
        let r = receiver[c] as usize;
        if flux[c] < ICE_FLUX_FROM || sea[c] || r == c {
            continue;
        }
        let (i, j) = ((c % fnx) as i64, (c / fnx) as i64);
        // The valley's rim: the highest ground around it.
        let mut rim = start[c];
        for dj in -2..=2 {
            for di in -2..=2 {
                rim = rim.max(start[at(i + di, j + dj)]);
            }
        }
        // Only valleys with relief to channel the ice: a lowland ice sheet
        // scours (M14's lake districts), it does not dig troughs — and its
        // grid-routed lines across flat ground drew a lattice of grooves.
        // Ramps, not thresholds: a cell just over one beside cells just
        // under it stood alone as a short groove with rounded ends.
        let ramp = |lo: f64, hi: f64, v: f64| {
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let full = ramp(f64::from(ICE_FROM), 1.0, f64::from(ice[c]))
            * ramp(TROUGH_RELIEF.0, TROUGH_RELIEF.1, rim - start[c])
            * ramp(ICE_FLUX_FROM, TROUGH_FLUX_FULL, flux[c]);
        let mut depth = (TROUGH_DEPTH_PER_ROOT * flux[c].sqrt())
            .min(TROUGH_MAX_DEPTH)
            .min(TROUGH_RELIEF_SHARE * (rim - start[c]))
            * full;
        if sea[r] {
            depth *= SILL_SHARE;
        }
        if depth < TROUGH_MIN_DEPTH {
            continue;
        }
        let floor = start[c] - depth;
        let half = (TROUGH_WIDTH_PER_QUARTER * flux[c].sqrt().sqrt()).min(TROUGH_MAX_HALF);
        // The U meets the rim within the search, or stops there.
        rim = rim.min(floor + depth * (reach / half).powi(2));
        kept[c] = true;
        cells.push(Trough {
            cell: c as u32,
            receiver: r as u32,
            floor: floor as f32,
            depth: depth as f32,
            half: half as f32,
            rim: rim as f32,
            head: true,
        });
    }
    // A head is a trough no kept trough flows into.
    let mut fed = vec![false; n];
    for t in &cells {
        if kept[t.receiver as usize] {
            fed[t.receiver as usize] = true;
        }
    }
    for t in &mut cells {
        t.head = !fed[t.cell as usize];
    }
    Troughs::from_cells(fnx, fnz, cx, cz, cells)
}

/// Lower valley sides left steeper than [`MAX_WALL_SLOPE`] by the cutting,
/// never below the slope the ground already had: two raster sweeps, each
/// taking the neighbours already swept.
fn retreat_walls(
    nx: usize,
    nz: usize,
    h: &mut [f64],
    start: &[f64],
    wet: &[bool],
    cap: &[f64],
    step: impl Fn(usize, usize) -> f64,
) {
    let at = |i: usize, j: usize, di: isize, dj: isize| {
        let ii = (i as isize + di).rem_euclid(nx as isize) as usize;
        let jj = (j as isize + dj).rem_euclid(nz as isize) as usize;
        jj * nx + ii
    };
    let mut relax = |i: usize, j: usize, back: bool| {
        let c = j * nx + i;
        if wet[c] {
            return;
        }
        let s: isize = if back { 1 } else { -1 };
        for (di, dj) in [(s, 0), (s, s), (0, s), (-s, s)] {
            let nb = at(i, j, di, dj);
            let below = if wet[nb] { 0.0 } else { h[nb] };
            let wall = MAX_WALL_SLOPE + (CANYON_WALL_SLOPE - MAX_WALL_SLOPE) * cap[c];
            let allowed = (wall * step(c, nb)).max(start[c] - start[nb]);
            if h[c] > below + allowed {
                h[c] = below + allowed;
            }
        }
    };
    for j in 0..nz {
        for i in 0..nx {
            relax(i, j, false);
        }
    }
    for j in (0..nz).rev() {
        for i in (0..nx).rev() {
            relax(i, j, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canyons (M15): the same plateau, the same rain — with a caprock
    /// (flash-flood weight 1) its river cuts deeper and its walls stand
    /// steeper than without.
    #[test]
    fn caprock_cuts_canyons() {
        // 64×64 drainage cells of 128 blocks: a plateau 300 high falling
        // gently east to the sea in the last columns, a shallow trough
        // along its middle row so one river forms.
        let (fnx, fnz) = (64usize, 64usize);
        let mut base = vec![0i32; fnx * fnz];
        let mut sea = vec![false; fnx * fnz];
        for j in 0..fnz {
            for i in 0..fnx {
                let c = j * fnx + i;
                if i >= fnx - 4 {
                    sea[c] = true;
                    base[c] = -20;
                } else {
                    let trough = (j as i32 - 32).abs().min(6);
                    base[c] = 300 - 2 * i as i32 + 3 * trough;
                }
            }
        }
        let runoff = vec![0.000_05f64; fnx * fnz];
        let erode = |w: f32| {
            Erosion::build(
                fnx,
                fnz,
                (128.0, 128.0),
                &base,
                &sea,
                &runoff,
                &vec![w; fnx * fnz],
                &vec![0.0; fnx * fnz],
            )
        };
        let (plain, capped) = (erode(0.0), erode(1.0));
        let deepest = |e: &Erosion| e.delta.iter().copied().fold(0.0f32, f32::min);
        assert!(
            deepest(&capped) < 1.15 * deepest(&plain),
            "canyon {} against valley {}",
            deepest(&capped),
            deepest(&plain)
        );
    }

    /// Glacial troughs (M15 task 3): a river's V in glaciated mountains is
    /// carved into a U — a floor several times wider at the bottom — and
    /// the carving only ever lowers the ground and leaves no step.
    #[test]
    fn ice_carves_a_valley_into_a_u() {
        // 64×64 drainage cells of 128 blocks: two mountain sides 0.5 rise
        // over run falling to a valley along the middle row, which falls
        // gently west to the sea.
        let (fnx, fnz) = (64usize, 64usize);
        let mut base = vec![0i32; fnx * fnz];
        let mut sea = vec![false; fnx * fnz];
        for j in 0..fnz {
            for i in 0..fnx {
                let c = j * fnx + i;
                if i < 2 {
                    sea[c] = true;
                    base[c] = -50;
                } else {
                    base[c] = 200 + 3 * i as i32 + 64 * (j as i32 - 32).abs();
                }
            }
        }
        let runoff = vec![0.000_01f64; fnx * fnz];
        let erode = |ice: f32| {
            Erosion::build(
                fnx,
                fnz,
                (128.0, 128.0),
                &base,
                &sea,
                &runoff,
                &vec![0.0; fnx * fnz],
                &vec![ice; fnx * fnz],
            )
        };
        let (river, glacier) = (erode(0.0), erode(1.0));
        assert!(river.troughs.cells.is_empty() && !glacier.troughs.cells.is_empty());
        // Across the valley at x = 40 cells: the ground before (the V the
        // rivers left) and after.
        let x = 40.5 * 128.0;
        let before = |z: i64| {
            let h = f64::from(base[32 * fnx + 40]) + 0.5 * (z as f64 - 32.5 * 128.0).abs();
            river.apply(x, z as f64, h as i64)
        };
        let after = |z: i64| glacier.apply(x, z as f64, before(z));
        let zs = (24 * 128..41 * 128).step_by(1).collect::<Vec<i64>>();
        let floor = |f: &dyn Fn(i64) -> i64| {
            let low = zs.iter().map(|&z| f(z)).min().unwrap();
            zs.iter().filter(|&&z| f(z) <= low + 10).count()
        };
        let (v, u) = (floor(&before), floor(&after));
        assert!(
            u >= 3 * v,
            "floor within 10 blocks of the bottom: {u} blocks wide against the V's {v}"
        );
        for w in zs.windows(2) {
            assert!(after(w[0]) <= before(w[0]));
            let step = (after(w[1]) - after(w[0])).abs();
            assert!(step <= 4, "a step of {step} at z = {}", w[0]);
        }
    }

    /// Canyon walls (M15): beside a cut 500 deep, a valley side retreats to
    /// 0.6 rise over run; under a caprock it stands at 3.
    #[test]
    fn caprock_holds_walls_steep() {
        let (nx, nz) = (9usize, 9usize);
        let start = vec![500.0f64; nx * nz];
        let centre = 4 * nx + 4;
        let side = |cap: f64| {
            let mut h = start.clone();
            h[centre] = 0.0;
            let step = |_: usize, _: usize| 100.0;
            retreat_walls(
                nx,
                nz,
                &mut h,
                &start,
                &vec![false; nx * nz],
                &vec![cap; nx * nz],
                step,
            );
            h[centre + 1]
        };
        assert_eq!(side(0.0), 0.6 * 100.0);
        assert_eq!(side(1.0), 3.0 * 100.0);
    }
}
