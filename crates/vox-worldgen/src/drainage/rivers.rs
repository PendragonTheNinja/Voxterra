//! Stage 2 of hydrology (M14 task 3): rivers, lakes and salt flats in the
//! columns, read from the network as pure functions of `(x, z)` — so chunks,
//! the LOD and every readout agree, and any chunk is generated alone.
//!
//! **Channels.** Every cell carrying a stream ([`STREAM_FROM_M3S`]) is a reach
//! from its centre to its receiver's. Centres are jittered so rivers do not
//! run in grid lines; the query point is warped by the meander noise
//! (`Elevation::meander_warp`) at a wavelength ~12× the river's width (real
//! meanders run 10–14×) and an amplitude that grows as the slope falls —
//! lowland rivers wind, torrents run nearly straight. A reach's width,
//! depth and warp are interpolated from its upstream cell's to its
//! receiver's, so a tributary meets its main river where the main river's
//! own bends have put it.
//!
//! **Geometry** (the spec's scale table, Earth's hydraulic geometry): width
//! `8·√Q` blocks — 1 for a stream of 0.02 m³/s, ~40 for the largest rivers;
//! depth `2.8·Q^⅜`, 1 to 12 blocks. The water surface follows the network's
//! channel level, interpolated along the reach and floored to whole blocks:
//! settled, full blocks, stepping down the long profile (the hybrid's
//! layer 1, `docs/design/water.md`), never rising downstream.
//!
//! **Banks.** Inside the channel the ground is the bed, parabolic across.
//! Beyond it the ground may rise from one block above the water no faster
//! than a 45° bank — steeper ground is cut back, lower ground raised into a
//! levee — easing back to the natural ground by [`BANK_REACH`] blocks out, so
//! water is never held by air, and the shaping ends continuously however
//! far the river is cut below its surroundings.
//!
//! **Alluvium** fills the hollows a river's grade runs above — those its rim
//! cut could not drain — to that grade: flat floors, as sediment fills them
//! over geological time.
//!
//! **Lakes** fill every column of their cells whose ground is below the lake's
//! level — the shore follows the ground's own contours — and the low ground
//! of their neighbours, so a dip at a cell's edge is part of the lake rather
//! than a hole beside it. **Salt flats** crust the floors of closed basins.

use super::{Cover, DrainageMap, HERE, neighbours};

/// The least discharge drawn as a stream, m³/s.
pub const STREAM_FROM_M3S: f32 = 0.01;
/// Width, blocks per √(m³/s).
const WIDTH_PER_ROOT_Q: f32 = 8.0;
/// Depth, blocks per (m³/s)^⅜.
const DEPTH_PER_Q: f32 = 2.8;
const MAX_DEPTH: f32 = 12.0;
/// Meander wavelength per block of width.
const MEANDER_PER_WIDTH: f32 = 12.0;
/// Below this slope (block per block) a river meanders fully.
const MEANDER_SLOPE: f32 = 0.01;
/// How far a reach's centre may sit from its cell's centre, of a cell.
const JITTER: f64 = 0.3;
/// Grid-scale sinuosity every river has, blocks: breaks the eight
/// directions of the network into curves.
const BASE_WANDER: f32 = 14.0;
const BASE_WANDER_WAVELENGTH: f32 = 128.0;
/// How far beyond a channel its banks are shaped, blocks.
const BANK_REACH: f32 = 48.0;
/// The steepest bank: rise per block.
const BANK_SLOPE: f32 = 1.0;
/// Manning's roughness of a natural channel.
const MANNING_N: f32 = 0.035;
/// The slowest and fastest water drawn flowing, m/s.
const MIN_FLOW: f32 = 0.05;
pub const MAX_FLOW: f32 = 4.0;
/// A salt flat's crust reaches this far above its cell's floor, blocks.
const SALT_FLAT_RISE: i64 = 2;

/// What water a column holds, and what its ground is under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    /// The water surface, blocks; meaningless for [`Body::Dry`] and
    /// [`Body::SaltFlat`].
    pub level: i64,
    pub body: Body,
    /// The nearest river, if one is within reach of its banks: what
    /// floodplains, wetlands and deltas are classified from (task 4).
    pub near: Option<Near>,
}

impl Water {
    pub const DRY: Self = Self {
        level: i64::MIN,
        body: Body::Dry,
        near: None,
    };
}

/// The river nearest a column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    /// How far beyond its bank, blocks (negative inside the channel).
    pub out: f32,
    /// Its water level there, blocks.
    pub level: i64,
    /// Its slope, blocks per block.
    pub slope: f32,
    pub width: f32,
    /// Its discharge, m³/s.
    pub discharge: f32,
    /// Whether this reach flows into the sea: a river mouth.
    pub mouth: bool,
}

/// The kind of water a column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    Dry,
    /// A river channel, with its bed by the reach's slope.
    River(Bed),
    Lake,
    SaltLake,
    /// The dry, salt-crusted floor of a closed basin.
    SaltFlat,
    /// A wetland's shallow pool (task 4): water a block deep at the
    /// river's level — finite water, in the hybrid's terms.
    Pool,
    /// Waterlogged ground of a wetland, at the river's level.
    Wetland,
    /// Flat ground beside a lowland river, a block above it.
    Floodplain,
    /// The flats of a large river's mouth, at sea level.
    Delta,
}

/// A river bed, by the energy of the water over it: gravel where it is
/// steep, sand between, silt where it is slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bed {
    Gravel,
    Sand,
    Silt,
}

/// One reach near a column, placed in the column's own unwrapped frame.
#[derive(Clone)]
struct Reach {
    /// How far from its unwarped line anything of it can reach, blocks.
    out: f32,
    from: (f64, f64),
    to: (f64, f64),
    level: (f32, f32),
    width: (f32, f32),
    depth: (f32, f32),
    wander: (f32, f32),
    slope: f32,
    discharge: f32,
    mouth: bool,
}

thread_local! {
    /// The reaches that can touch the cell last asked about, keyed by map id
    /// and the cell (unwrapped): columns come in runs across a chunk or an
    /// LOD node, so most queries reuse the list instead of rebuilding 25.
    static NEAR: std::cell::RefCell<(u64, i64, i64, Vec<Reach>)> =
        const { std::cell::RefCell::new((0, i64::MIN, i64::MIN, Vec::new())) };
}

impl DrainageMap {
    /// The reaches of the 5×5 cells around `(ci, cj)` (unwrapped) that can
    /// touch a column inside it.
    fn reaches_near(&self, ci: i64, cj: i64, out: &mut Vec<Reach>) {
        out.clear();
        let centre = (
            (ci as f64 + 0.5) * self.cell_x,
            (cj as f64 + 0.5) * self.cell_z,
        );
        let half_diagonal = 0.5 * (self.cell_x * self.cell_x + self.cell_z * self.cell_z).sqrt();
        for dj in -2..=2 {
            for di in -2..=2 {
                if let Some(reach) = self.reach(ci + di, cj + dj) {
                    let (d, _) = segment(centre.0, centre.1, reach.from, reach.to);
                    if d - half_diagonal <= f64::from(reach.out) {
                        out.push(reach);
                    }
                }
            }
        }
    }

    /// The ground and water at a column, from its eroded ground `ground`
    /// (the elevation field cut by the erosion stage). `warp(x, z, λ)` is
    /// the meander noise. `x`, `z` on any lap of the world.
    pub fn shape_column(
        &self,
        x: i64,
        z: i64,
        ground: i64,
        warp: impl Fn(i64, i64, f32) -> (f32, f32),
    ) -> (i64, Water) {
        let (fx, fz) = (x as f64, z as f64);
        let ci = (fx / self.cell_x).floor() as i64;
        let cj = (fz / self.cell_z).floor() as i64;
        let mut h = ground as f32;
        let mut best: Option<(f32, f32, Bed)> = None; // (inside, level, bed)
        // The banks: no higher than `ceiling`, no lower than `floor`.
        let (mut ceiling, mut floor) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut nearest: Option<Near> = None;
        // The grid-scale wander is the same for every reach: once a column,
        // and only if some reach is near.
        let mut wander = None;
        NEAR.with_borrow_mut(|near| {
            if (near.0, near.1, near.2) != (self.id, ci, cj) {
                self.reaches_near(ci, cj, &mut near.3);
                (near.0, near.1, near.2) = (self.id, ci, cj);
            }
            for reach in &near.3 {
                let (d0, t0) = segment(fx, fz, reach.from, reach.to);
                if d0 > f64::from(reach.out) {
                    continue;
                }
                // Warp the query by this reach's meander, as it stands at
                // the column's place along it.
                let at = |pair: (f32, f32)| pair.0 + (pair.1 - pair.0) * t0 as f32;
                let width = at(reach.width);
                let wavelength = (MEANDER_PER_WIDTH * width).max(32.0);
                let (mx, mz) = warp(x, z, wavelength);
                let (bx, bz) = *wander.get_or_insert_with(|| warp(x, z, BASE_WANDER_WAVELENGTH));
                let amp = at(reach.wander);
                let wx = fx + f64::from(mx * amp + bx * BASE_WANDER);
                let wz = fz + f64::from(mz * amp + bz * BASE_WANDER);
                let (d, t) = segment(wx, wz, reach.from, reach.to);
                let d = d as f32;
                let t = t as f32;
                let half = 0.5 * (reach.width.0 + (reach.width.1 - reach.width.0) * t);
                let surface = reach.level.0 + (reach.level.1 - reach.level.0) * t;
                let level = surface.floor();
                let out = d - half;
                if nearest.is_none_or(|n: Near| out < n.out) {
                    nearest = Some(Near {
                        out,
                        level: level as i64,
                        slope: reach.slope,
                        width: 2.0 * half,
                        discharge: reach.discharge,
                        mouth: reach.mouth,
                    });
                }
                // Banks: within a 45° rise or fall of a block above the
                // water, easing back to the ground by BANK_REACH (the
                // quartic term outgrows any ground there).
                let e = (out / BANK_REACH).max(0.0);
                let give = BANK_SLOPE * out.max(0.0) + 4_096.0 * e * e * e * e;
                ceiling = ceiling.min(level + 1.0 + give);
                // The levee holds the water beside it, which on a steep reach
                // stands up to a block's fall higher than here.
                let held = (surface + reach.slope).ceil();
                floor = floor.max(held + 1.0 - give);
                if out < 0.0 && best.is_none_or(|b| out < b.0) {
                    let depth = reach.depth.0 + (reach.depth.1 - reach.depth.0) * t;
                    let across = (d / half.max(0.5)).min(1.0);
                    let deep = (depth * (1.0 - across * across)).round().max(1.0);
                    let bed = if reach.slope > 0.02 {
                        Bed::Gravel
                    } else if reach.slope > 0.004 {
                        Bed::Sand
                    } else {
                        Bed::Silt
                    };
                    best = Some((out, level, bed));
                    h = level - deep;
                }
            }
        });
        if let Some((_, level, bed)) = best {
            return (
                h as i64,
                Water {
                    level: level as i64,
                    body: Body::River(bed),
                    near: nearest,
                },
            );
        }
        // Steeper ground cut back, lower ground raised into a levee; where
        // two reaches disagree — a steep one stepping down past its own
        // lower water — the levee wins, so no water is ever held by air.
        let h = h.min(ceiling).max(floor).round() as i64;
        let h = self.alluvium(ci, cj, h);
        let mut water = self.still_water(ci, cj, h);
        water.near = nearest;
        (h, water)
    }

    /// The ground of a column of cell `(ci, cj)` (unwrapped) after the
    /// sediment that fills hollows below a river's grade: raised to the
    /// level of its own cell's fill, or of a neighbour's — the floor's edge
    /// follows the ground's contours, as a lake's shore does.
    fn alluvium(&self, ci: i64, cj: i64, h: i64) -> i64 {
        let here = self.wrap(ci, cj);
        let fill = |c: usize| {
            let level = i64::from(self.lvl(c));
            (self.cover_at(c) == Cover::Land && level > i64::from(self.h(c))).then_some(level)
        };
        let mut level = fill(here);
        if level.is_none() {
            for nb in neighbours(here, self.nx, self.nz) {
                if let Some(l) = fill(nb)
                    && h < l
                    && level.is_none_or(|m| l < m)
                {
                    level = Some(l);
                }
            }
        }
        level.map_or(h, |l| h.max(l))
    }

    /// A lake or salt flat at a column of cell `(ci, cj)` (unwrapped) whose
    /// ground is `h`.
    fn still_water(&self, ci: i64, cj: i64, h: i64) -> Water {
        let here = self.wrap(ci, cj);
        let lake_of = |c: usize| match self.cover_at(c) {
            Cover::Lake => Some((i64::from(self.lvl(c)), Body::Lake)),
            Cover::SaltLake => Some((i64::from(self.lvl(c)), Body::SaltLake)),
            _ => None,
        };
        let mut found = lake_of(here);
        if found.is_none() {
            for nb in neighbours(here, self.nx, self.nz) {
                if let Some((level, body)) = lake_of(nb)
                    && h < level
                    && found.is_none_or(|(l, _)| level < l)
                {
                    found = Some((level, body));
                }
            }
        }
        match found {
            Some((level, body)) if h < level => Water {
                level,
                body,
                near: None,
            },
            _ if self.cover_at(here) == Cover::SaltFlat
                && h <= i64::from(self.h(here)) + SALT_FLAT_RISE =>
            {
                Water {
                    level: h,
                    body: Body::SaltFlat,
                    near: None,
                }
            }
            _ => Water::DRY,
        }
    }

    fn wrap(&self, i: i64, j: i64) -> usize {
        let i = i.rem_euclid(self.nx as i64) as usize;
        let j = j.rem_euclid(self.nz as i64) as usize;
        j * self.nx + i
    }

    /// Where a cell's reach sits, in the unwrapped frame of cell `(i, j)`.
    fn node(&self, i: i64, j: i64) -> (f64, f64) {
        let c = self.wrap(i, j);
        let h = super::mix(c as u64);
        let jx = ((h & 0xFFFF) as f64 / 65_535.0 - 0.5) * 2.0 * JITTER;
        let jz = (((h >> 16) & 0xFFFF) as f64 / 65_535.0 - 0.5) * 2.0 * JITTER;
        (
            (i as f64 + 0.5 + jx) * self.cell_x,
            (j as f64 + 0.5 + jz) * self.cell_z,
        )
    }

    /// The water's velocity in cell `(i, j)`, blocks per second along x and
    /// z (M14, criterion 10): what the water's texture drifts at. A river's
    /// speed from Manning's relation, `v = R^⅔·S^½ / n` with the channel's
    /// depth for R and n = 0.035 (a natural channel), 0.05 to 4 m/s — a
    /// torrent races, a lowland river barely moves — along its reach.
    /// A cell beside a river takes the largest one's flow (its bends wander
    /// into its neighbours); lakes and the sea are still.
    pub fn flow(&self, i: usize, j: usize) -> (f32, f32) {
        let c = j * self.nx + i;
        if matches!(self.cover_at(c), Cover::Sea | Cover::Lake | Cover::SaltLake) {
            return (0.0, 0.0);
        }
        let velocity = |i: i64, j: i64| {
            let r = self.reach(i, j)?;
            let (dx, dz) = (r.to.0 - r.from.0, r.to.1 - r.from.1);
            let len = (dx * dx + dz * dz).sqrt().max(1.0);
            let depth = r.depth.0.max(1.0);
            let v = (depth.cbrt() * depth.cbrt() * r.slope.max(0.0).sqrt() / MANNING_N)
                .clamp(MIN_FLOW, MAX_FLOW);
            Some((r.discharge, (dx / len) as f32 * v, (dz / len) as f32 * v))
        };
        let (i, j) = (i as i64, j as i64);
        if let Some((_, vx, vz)) = velocity(i, j) {
            return (vx, vz);
        }
        let mut best: Option<(f32, f32, f32)> = None;
        for (di, dj) in OFFSETS {
            if let Some(v) = velocity(i + di, j + dj)
                && best.is_none_or(|b| v.0 > b.0)
            {
                best = Some(v);
            }
        }
        best.map_or((0.0, 0.0), |(_, vx, vz)| (vx, vz))
    }

    /// The reach leaving cell `(i, j)` (unwrapped), if it carries a stream.
    fn reach(&self, i: i64, j: i64) -> Option<Reach> {
        let c = self.wrap(i, j);
        let q = self.discharge[c];
        let dir = self.flow[c];
        if q < STREAM_FROM_M3S || dir == HERE || self.sea_at(c) {
            return None;
        }
        let r = neighbours(c, self.nx, self.nz)[dir as usize];
        let lake = |c: usize| matches!(self.cover_at(c), Cover::Lake | Cover::SaltLake);
        if lake(c) && lake(r) {
            return None; // inside a lake: the lake is the water
        }
        let (di, dj) = OFFSETS[dir as usize];
        let (ri, rj) = (i + di, j + dj);
        let from = self.node(i, j);
        let to = self.node(ri, rj);
        let qr = self.discharge[r].max(q);
        let level_of = |c: usize| {
            if self.sea_at(c) {
                0.0
            } else {
                f32::from(self.level[c])
            }
        };
        let (lc, lr) = (level_of(c), level_of(r).min(level_of(c)));
        let (sx, sz) = (to.0 - from.0, to.1 - from.1);
        let step = (sx * sx + sz * sz).sqrt() as f32;
        let slope = (lc - lr) / step.max(1.0);
        let width = |q: f32| (WIDTH_PER_ROOT_Q * q.sqrt()).max(1.0);
        let depth =
            |q: f32| (DEPTH_PER_Q * q.sqrt() / q.sqrt().sqrt().sqrt()).clamp(1.0, MAX_DEPTH);
        let flat = 1.0 - (slope / MEANDER_SLOPE).min(1.0);
        let wander = |w: f32| w * (0.3 + 1.2 * flat);
        let (w0, w1) = (width(q), width(qr));
        let (a0, a1) = (wander(w0), wander(w1));
        Some(Reach {
            out: w0.max(w1) * 0.5 + BANK_REACH + BASE_WANDER + a0.max(a1),
            from,
            to,
            level: (lc, lr),
            width: (width(q), width(qr)),
            depth: (depth(q), depth(qr)),
            wander: (wander(width(q)), wander(width(qr))),
            slope,
            discharge: qr,
            mouth: self.sea_at(r),
        })
    }
}

/// The cell offsets of [`neighbours`]' order.
const OFFSETS: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, 1),
    (1, -1),
    (-1, -1),
];

/// Distance from `(x, z)` to the segment `a`–`b`, and how far along it the
/// nearest point is, `[0, 1]`.
fn segment(x: f64, z: f64, a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (vx, vz) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vz * vz;
    let t = if len2 > 0.0 {
        (((x - a.0) * vx + (z - a.1) * vz) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, pz) = (a.0 + vx * t, a.1 + vz * t);
    let (dx, dz) = (x - px, z - pz);
    ((dx * dx + dz * dz).sqrt(), t)
}
