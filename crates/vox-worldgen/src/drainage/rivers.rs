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
/// How far into the cells beside a lake its water may reach, blocks; a
/// bank holds it beyond.
const LAKE_REACH: f64 = 48.0;
/// How steeply a lake's bank, and a fill's edge, fall to the ground: rise
/// per block.
const BANK_FALL: f64 = 1.0;
const ALLUVIUM_SLOPE: f64 = BANK_FALL;
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

/// What kind of river a reach is (M14 criterion 10): from its slope, its
/// discharge and its sediment — which follows its climate — as Earth's
/// rivers differ. (Earth's fifth kind, the slow lake-like reach, is where a
/// river crosses a lake: the network's lakes.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RiverKind {
    /// Steep: narrow, fast, straight-ish, over gravel (Alpine streams).
    Torrent,
    /// Moderately steep, loaded with coarse sediment — below glaciers, in
    /// deserts: wide, shallow, split among gravel bars (the Rakaia, Alaska).
    Braided,
    /// Gentle: winding, deeper, slow, over sand and silt (the Mississippi).
    Meandering,
    /// A desert channel too small to run but after storms: a dry bed.
    Wadi,
}

/// The kind of a reach of slope `slope` carrying `q` m³/s through ground
/// that is `arid` or ice-scoured (`cold`).
pub fn river_kind(slope: f32, q: f32, arid: bool, cold: bool) -> RiverKind {
    if arid && q < WADI_BELOW_M3S {
        RiverKind::Wadi
    } else if (arid || cold)
        && q >= BRAIDED_FROM_M3S
        && (BRAID_SLOPES.0..BRAID_SLOPES.1).contains(&slope)
    {
        RiverKind::Braided
    } else if slope > TORRENT_SLOPE {
        RiverKind::Torrent
    } else {
        RiverKind::Meandering
    }
}

/// A reach that falls this far (blocks), and twice as far as the river
/// below it, does it at once: a waterfall at a step in the profile.
pub(super) const FALL_FROM: f32 = 12.0;
/// The highest a waterfall falls at once, blocks; a reach that drops more
/// runs on down its grade below it.
const FALL_MAX: f32 = 32.0;
/// How far past the lip the falling sheet of water reaches, blocks.
const FALL_SHEET: f32 = 1.5;

/// Steeper than this, a river is a torrent.
const TORRENT_SLOPE: f32 = 0.01;
/// Braided rivers: slopes and the least discharge.
const BRAID_SLOPES: (f32, f32) = (0.002, 0.03);
const BRAIDED_FROM_M3S: f32 = 0.05;
/// How much wider a braided river spreads than a single channel.
const BRAIDED_SPREAD: f32 = 2.5;
/// Braid noise below which a braided river's bed is a dry bar.
const BRAID_BARS: f32 = -0.15;
/// An arid stream smaller than this runs only after storms: a wadi.
const WADI_BELOW_M3S: f32 = 0.05;

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
    kind: RiverKind,
    /// Where along the reach (0..1) its whole drop falls at once, if it
    /// falls enough to be a waterfall; and how long the reach is, blocks.
    fall: Option<f32>,
    len: f32,
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
                // A waterfall: the pool above holds its level to the lip and
                // the sheet just past it; the river below runs at its own.
                // Its banks stand at the upper level a little further, a
                // cliff beside the falling water.
                let line = reach.level.0 + (reach.level.1 - reach.level.0) * t;
                // Below a waterfall the river runs at its foot, at most
                // FALL_MAX under the lip, until its grade comes down to it:
                // the gorge a waterfall cuts is as deep as the fall, not as
                // the whole reach's drop (which made gorges of 100+ blocks
                // once the lip moved to where the ground falls away).
                let below = (reach.level.0 - FALL_MAX).max(reach.level.1).min(line);
                let (surface, sheet, cliff) = match reach.fall {
                    Some(lip) => {
                        let past = (t - lip) * reach.len;
                        let upper = past < FALL_SHEET;
                        let level = if upper { reach.level.0 } else { below };
                        (
                            level,
                            (0.0..FALL_SHEET).contains(&past),
                            past < FALL_SHEET + 2.0,
                        )
                    }
                    None => (line, false, false),
                };
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
                // water; they blend back into the ground by BANK_REACH
                // (below).
                let give = BANK_SLOPE * out.max(0.0);
                ceiling = ceiling.min(level + 1.0 + give);
                // The levee holds the water beside it, which on a steep reach
                // stands up to a block's fall higher than here.
                let held = if cliff && reach.fall.is_some() {
                    reach.level.0.floor()
                } else {
                    (surface + reach.slope).ceil()
                };
                floor = floor.max(held + 1.0 - give);
                if out < 0.0 && best.is_none_or(|b| out < b.0) {
                    let depth = reach.depth.0 + (reach.depth.1 - reach.depth.0) * t;
                    let across = (d / half.max(0.5)).min(1.0);
                    let deep = (depth * (1.0 - across * across)).round().max(1.0);
                    let (bed, bar) = match reach.kind {
                        RiverKind::Torrent => (Bed::Gravel, false),
                        // Gravel bars between the braids: the bed at the
                        // water's level, dry.
                        RiverKind::Braided => (Bed::Gravel, warp(x, z, 32.0).0 < BRAID_BARS),
                        RiverKind::Wadi => (Bed::Sand, false),
                        RiverKind::Meandering if reach.slope > 0.002 => (Bed::Sand, false),
                        RiverKind::Meandering => (Bed::Silt, false),
                    };
                    h = if bar { level } else { level - deep };
                    if sheet {
                        // The falling sheet stands on the river below's bed.
                        h = below.floor() - deep;
                    }
                    // A wadi's channel holds no water: its level is its bed.
                    let water = if reach.kind == RiverKind::Wadi {
                        h
                    } else {
                        level
                    };
                    best = Some((out, water, bed));
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
        // The banks blend into the natural ground by BANK_REACH from the
        // nearest channel, smoothly (a quartic term that outgrew the ground
        // there had stood deep cuts' sides up as cliffs).
        let banked = h.min(ceiling).max(floor);
        let e = nearest.map_or(1.0, |n| (n.out / BANK_REACH).clamp(0.0, 1.0));
        let natural = e * e * (3.0 - 2.0 * e);
        let h = (banked + (h - banked) * natural).round() as i64;
        let h = self.alluvium((ci, cj), (x, z), h);
        let (h, mut water) = self.still_water((ci, cj), (x, z), h);
        water.near = nearest;
        (h, water)
    }

    /// How far column `(x, z)` lies outside cell `(i, j)` (both unwrapped),
    /// blocks: 0 inside it.
    fn outside(&self, i: i64, j: i64, x: i64, z: i64) -> f64 {
        let (px, pz) = (x as f64 + 0.5, z as f64 + 0.5);
        let gap = |p: f64, k: i64, size: f64| {
            let (lo, hi) = (k as f64 * size, (k + 1) as f64 * size);
            (lo - p).max(p - hi).max(0.0)
        };
        let (dx, dz) = (gap(px, i, self.cell_x), gap(pz, j, self.cell_z));
        (dx * dx + dz * dz).sqrt()
    }

    /// The ground of column `(x, z)`, of cell `(ci, cj)` (unwrapped), after
    /// the sediment that fills hollows below a river's grade: a fill's cell
    /// is raised flat to its level, and its neighbours' low ground slopes
    /// down from it at 45° until it meets their own. (It had been raised
    /// flat to the fill's level wherever it lay below it, which ended in a
    /// cliff wherever the ground beyond the neighbours was lower — the
    /// owner's walk.) A ramp has fallen a cell's width by the time it leaves
    /// the cells considered, so the ground stays continuous.
    fn alluvium(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), h: i64) -> i64 {
        let fill = |c: usize| {
            let level = i64::from(self.lvl(c));
            (self.cover_at(c) == Cover::Land && level > i64::from(self.h(c))).then_some(level)
        };
        let mut top = h;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                if let Some(l) = fill(self.wrap(i, j)) {
                    let d = self.outside(i, j, x, z);
                    top = top.max(l - (d * ALLUVIUM_SLOPE).floor() as i64);
                }
            }
        }
        top
    }

    /// The ground and the lake or salt flat at column `(x, z)`, of cell
    /// `(ci, cj)` (unwrapped), whose ground is `h`.
    ///
    /// A lake fills its cells' ground below its level, and its neighbours'
    /// as far as [`LAKE_REACH`] into them — the shore follows the ground's
    /// contours. Beyond that a bank holds it, falling at 45° to the ground:
    /// the lake's level is its cells' centres' spill, and the ground at the
    /// far side of its neighbours can be lower still, where its water had
    /// stood over air (17 blocks of it at the bench's waterfall, the owner's
    /// walk). A lake's cells hold a higher neighbouring lake the same way.
    fn still_water(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), h: i64) -> (i64, Water) {
        let here = self.wrap(ci, cj);
        let lake_of = |c: usize| match self.cover_at(c) {
            Cover::Lake => Some((i64::from(self.lvl(c)), Body::Lake)),
            Cover::SaltLake => Some((i64::from(self.lvl(c)), Body::SaltLake)),
            _ => None,
        };
        let own = lake_of(here);
        let mut found = own;
        let mut bank = i64::MIN;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                let Some((level, body)) = lake_of(self.wrap(i, j)) else {
                    continue;
                };
                let d = self.outside(i, j, x, z);
                match own {
                    // In a lake's own cells, only a higher lake beside it
                    // needs holding: from its edge.
                    Some((mine, _)) => {
                        if level > mine && d > 0.0 {
                            bank = bank.max(level + 1 - (d * BANK_FALL).floor() as i64);
                        }
                    }
                    None if d <= LAKE_REACH => {
                        if h < level && found.is_none_or(|(l, _)| level < l) {
                            found = Some((level, body));
                        }
                    }
                    None => {
                        let past = d - LAKE_REACH;
                        bank = bank.max(level + 1 - (past * BANK_FALL).floor() as i64);
                    }
                }
            }
        }
        let h = h.max(bank);
        let water = match found {
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
        };
        (h, water)
    }

    fn wrap(&self, i: i64, j: i64) -> usize {
        let i = i.rem_euclid(self.nx as i64) as usize;
        let j = j.rem_euclid(self.nz as i64) as usize;
        j * self.nx + i
    }

    /// Where a cell's reach sits, in the unwrapped frame of cell `(i, j)`.
    fn node(&self, i: i64, j: i64) -> (f64, f64) {
        node(self.wrap(i, j), i, j, (self.cell_x, self.cell_z))
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

    /// Whether the reach leaving cell `(i, j)` has a waterfall.
    pub fn waterfall_at(&self, i: usize, j: usize) -> bool {
        self.waterfall_lip(i, j).is_some()
    }

    /// How far along the reach leaving cell `(i, j)` its waterfall's lip
    /// is, 0..1 from the cell's node to its receiver's, if it has one.
    pub fn waterfall_lip(&self, i: usize, j: usize) -> Option<f32> {
        self.reach(i as i64, j as i64).and_then(|r| r.fall)
    }

    /// The kind of river leaving cell `(i, j)`, if one does.
    pub fn river_kind_at(&self, i: usize, j: usize) -> Option<RiverKind> {
        self.reach(i as i64, j as i64).map(|r| r.kind)
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
        let kind = river_kind(slope, qr, self.arid_at(c), self.cold_at(c));
        // A braided river spreads wide and shallow over its gravel; a wadi
        // is a dry channel, a block deep.
        let (spread, deep): (f32, Option<f32>) = match kind {
            RiverKind::Braided => (BRAIDED_SPREAD, Some(1.0)),
            RiverKind::Wadi => (1.0, Some(1.0)),
            _ => (1.0, None),
        };
        let width = |q: f32| width(q) * spread;
        let depth = |q: f32| deep.unwrap_or_else(|| depth(q));
        let (w0, w1) = (width(q), width(qr));
        let (a0, a1) = (wander(w0), wander(w1));
        // A waterfall is a step in the long profile, not a steep one: it
        // falls at least FALL_FROM, and twice what the river below does.
        let below = {
            let d = self.flow[r];
            if d == HERE {
                0.0
            } else {
                let rr = neighbours(r, self.nx, self.nz)[d as usize];
                (level_of(r) - level_of(rr).min(level_of(r))).max(0.0)
            }
        };
        let drop = lc - lr;
        // It falls where its ground does (stage 1's `lip`): the pool above
        // ends where the ground stops holding it.
        let fall = (drop >= FALL_FROM && drop >= 2.0 * below && kind != RiverKind::Wadi)
            .then(|| f32::from(self.lip[c]) / 256.0);
        Some(Reach {
            fall,
            len: step,
            out: w0.max(w1) * 0.5 + BANK_REACH + BASE_WANDER + a0.max(a1),
            from,
            to,
            level: (lc, lr),
            width: (w0, w1),
            depth: (depth(q), depth(qr)),
            wander: (a0, a1),
            slope,
            discharge: qr,
            mouth: self.sea_at(r),
            kind,
        })
    }
}

/// How far either side of its straight line a reach's channel can lie,
/// blocks: half its width, its meanders' amplitude and the grid-scale
/// wander — the ground the river may be found on. `q` is its discharge,
/// `slope` its fall per block (as [`DrainageMap::reach`] shapes it).
pub(super) fn belt(q: f32, slope: f32) -> f32 {
    let width = (WIDTH_PER_ROOT_Q * q.sqrt()).max(1.0);
    let flat = 1.0 - (slope / MEANDER_SLOPE).min(1.0);
    0.5 * width + width * (0.3 + 1.2 * flat) + BASE_WANDER
}

/// Where the reach of cell `c` — `(i, j)` on some lap — sits: its centre,
/// jittered so rivers do not run in grid lines.
pub(super) fn node(c: usize, i: i64, j: i64, cell: (f64, f64)) -> (f64, f64) {
    let h = super::mix(c as u64);
    let jx = ((h & 0xFFFF) as f64 / 65_535.0 - 0.5) * 2.0 * JITTER;
    let jz = (((h >> 16) & 0xFFFF) as f64 / 65_535.0 - 0.5) * 2.0 * JITTER;
    (
        (i as f64 + 0.5 + jx) * cell.0,
        (j as f64 + 0.5 + jz) * cell.1,
    )
}

/// The cell offsets of [`neighbours`]' order.
pub(super) const OFFSETS: [(i64, i64); 8] = [
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
