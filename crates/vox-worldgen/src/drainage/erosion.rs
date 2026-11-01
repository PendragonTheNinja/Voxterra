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

/// The erosion stage: the change erosion made, on its own grid.
pub(super) struct Erosion {
    nx: usize,
    nz: usize,
    cell_x: f64,
    cell_z: f64,
    /// Blocks, never positive, at each cell's centre.
    delta: Vec<f32>,
}

impl Erosion {
    /// Erode the drainage grid's ground (`fnx`×`fnz` cells of `cell`
    /// blocks): `base` heights, the `sea`, and each cell's `runoff`, m³/s.
    pub(super) fn build(
        fnx: usize,
        fnz: usize,
        cell: (f64, f64),
        base: &[i32],
        sea: &[bool],
        runoff: &[f64],
    ) -> Self {
        // Two by two drainage cells to one: the mean height, the summed
        // runoff, sea where most of it is.
        let (nx, nz) = (fnx / 2, fnz / 2);
        let n = nx * nz;
        let fine = |i: usize, j: usize, di: usize, dj: usize| (2 * j + dj) * fnx + 2 * i + di;
        let mut h = vec![0.0f64; n];
        let mut wet = vec![false; n];
        let mut shed = vec![0.0f64; n];
        for j in 0..nz {
            for i in 0..nx {
                let c = j * nx + i;
                let (mut sum, mut seas) = (0i64, 0);
                for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let f = fine(i, j, di, dj);
                    sum += i64::from(base[f]);
                    seas += usize::from(sea[f]);
                    shed[c] += runoff[f];
                }
                h[c] = sum as f64 / 4.0;
                wet[c] = seas >= 2;
            }
        }
        let start = h.clone();
        let (cell_x, cell_z) = (cell.0 * 2.0, cell.1 * 2.0);
        let diagonal = cell_x.hypot(cell_z);
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
                let f = STRENGTH * (q[c] / Q_REF).sqrt() * (STEP_REF / step(c, r));
                h[c] = (h[c] + f * floor) / (1.0 + f);
            }
            retreat_walls(nx, nz, &mut h, &start, &wet, step);
        }
        let delta = (0..n).map(|c| (h[c] - start[c]).min(0.0) as f32).collect();
        Self {
            nx,
            nz,
            cell_x,
            cell_z,
            delta,
        }
    }

    /// The change erosion made at a point, blocks (never positive):
    /// bilinear between cell centres, on any lap of the world.
    pub(super) fn at(&self, x: f64, z: f64) -> f32 {
        let u = x / self.cell_x - 0.5;
        let v = z / self.cell_z - 0.5;
        let (fu, fv) = (u.floor(), v.floor());
        let (tu, tv) = ((u - fu) as f32, (v - fv) as f32);
        let wrap = |a: f64, n: usize| (a as i64).rem_euclid(n as i64) as usize;
        let (i0, j0) = (wrap(fu, self.nx), wrap(fv, self.nz));
        let (i1, j1) = ((i0 + 1) % self.nx, (j0 + 1) % self.nz);
        let d = |i: usize, j: usize| self.delta[j * self.nx + i];
        let top = d(i0, j0) + (d(i1, j0) - d(i0, j0)) * tu;
        let bottom = d(i0, j1) + (d(i1, j1) - d(i0, j1)) * tu;
        top + (bottom - top) * tv
    }
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
            let allowed = (MAX_WALL_SLOPE * step(c, nb)).max(start[c] - start[nb]);
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
