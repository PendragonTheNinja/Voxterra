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

use super::{Cover, DrainageMap, HERE, PROFILE_STEPS, neighbours};

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
/// How far beside a river ground below its water holds a pond rather than
/// being raised into a bank, blocks.
const POND_REACH: f32 = 24.0;
/// A cell's region (lake, fill, salt flat): a disc about its centre of
/// this radius, in cells, ± REGION_WOBBLE by noise of REGION_WAVELENGTH
/// blocks; beyond it a bank or a fill's slope falls to the ground.
pub(super) const REGION_RADIUS: f64 = 0.9;
pub(super) const REGION_WOBBLE: f64 = 0.15;
pub(super) const REGION_WAVELENGTH: f32 = 128.0;
/// How steeply a lake's bank, and a fill's edge, fall to the ground: rise
/// per block.
const BANK_FALL: f64 = 1.0;
const ALLUVIUM_SLOPE: f64 = BANK_FALL;
/// How near a sea cell's centre a column is coastal, cells.
const COAST_REACH: f64 = 1.2;
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

/// A step in a reach's profile this high (blocks) is a waterfall.
const FALL_FROM: f32 = 12.0;
/// How far upstream a bank looks for the water it holds, blocks: across a
/// step in the profile, a bank beside the upper water must hold it.
const HELD_BACK: f32 = 3.0;

/// Steeper than this, a river is a torrent.
const TORRENT_SLOPE: f32 = 0.01;
/// Braided rivers: slopes and the least discharge.
const BRAID_SLOPES: (f32, f32) = (0.002, 0.03);
const BRAIDED_FROM_M3S: f32 = 0.05;
/// How much wider a braided river spreads than a single channel.
const BRAIDED_SPREAD: f32 = 2.5;
/// How far out from a braided river's middle its bars may reach, of its
/// half-width.
const BAR_ACROSS: f32 = 0.6;
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
    /// Its long profile: the water at each of PROFILE_STEPS stretches
    /// along it (ADR-0020 §10), held level through each, stepping down
    /// between; its receiver's level from the end on.
    steps: [f32; PROFILE_STEPS],
    /// Where along the reach (0..1) a step is high enough to be a
    /// waterfall, the highest if several; and how long the reach is, blocks.
    fall: Option<f32>,
    len: f32,
}

impl Reach {
    /// The water's level at `t` (0..1) along the reach: down each stretch
    /// from its point to the next — a step of FALL_FROM or more held level
    /// to the end and dropped at once, a waterfall. (Every stretch held
    /// level stood a wide river's water in terraces.) A stretch's point is
    /// under the ground to the next one, so the slope down to it is too.
    fn level_at(&self, t: f32) -> f32 {
        if t >= 1.0 {
            return self.level.1;
        }
        let at = t.max(0.0) * PROFILE_STEPS as f32;
        let k = (at as usize).min(PROFILE_STEPS - 1);
        let here = self.steps[k];
        let next = self.steps.get(k + 1).copied().unwrap_or(self.level.1);
        if here - next >= FALL_FROM {
            here
        } else {
            here + (next - here) * (at - k as f32)
        }
    }
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
        ground_at: impl Fn(i64, i64) -> i64,
    ) -> (i64, Water) {
        let (fx, fz) = (x as f64, z as f64);
        let ci = (fx / self.cell_x).floor() as i64;
        let cj = (fz / self.cell_z).floor() as i64;
        // Sediment fills first, so a river's banks are cut into them: a
        // fill sloping down from a higher river's grade, applied after, had
        // raised a lower channel's banks back up into a slot one block wide
        // and ten deep (the owner's walk, a waterfall's gorge).
        let wobble = if self.regions_near(ci, cj) {
            f64::from(warp(x, z, REGION_WAVELENGTH).0)
        } else {
            0.0
        };
        let mut h = self.alluvium((ci, cj), (x, z), ground, wobble) as f32;
        let mut best: Option<(f32, f32, Bed)> = None; // (inside, level, bed)
        // Within POND_REACH of a river, ground below its water holds it as a
        // pond instead of being raised into a causeway: the nearest's level.
        let mut pond: Option<(f32, f32)> = None; // (out, level)
        // The banks: no higher than `ceiling`, no lower than `floor`.
        let (mut ceiling, mut floor) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut nearest: Option<Near> = None;
        // The grid-scale wander is the same for every reach: once a column,
        // and only if some reach is near.
        let mut wander = None;
        // Under a lake or the sea, the water is the lake's or the sea's. A
        // reach runs on to its lake's or the sea's cell centre, and its
        // levees had been built out across the open water — a dike (the
        // owner's walk): there only its channel is cut.
        let mut drowned = None;
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
                // Under a lake or the sea the channel goes on — its bed and
                // its sides cut on into the lake's floor (stopping dead, they
                // left an underwater ledge jutting out where a river meets a
                // lake: the owner's walk) — but nothing is built there.
                let under =
                    *drowned.get_or_insert_with(|| self.drowned((ci, cj), (x, z), ground, wobble));
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
                // The water here, from the reach's long profile: level
                // through each stretch, stepping down between them.
                let surface = reach.level_at(t);
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
                // The bank holds the water beside it — the highest within a
                // few blocks upstream, where the profile steps down past it.
                let held = reach.level_at(t - HELD_BACK / reach.len.max(1.0)).floor();
                // Water is held by ground level with its surface; a bank a
                // block above it raised flat ground beside every river into
                // a little levee (the owner's walk).
                // Where a river is held at its receiver's level over ground
                // lower still — a hollow it crosses, below the grade
                // downstream — ground within POND_REACH below the water is
                // not raised: it holds a pond (below) rather than carrying
                // the river on a causeway, and the bank falls at 45° beyond.
                // Elsewhere the profile keeps the river under the ground
                // beside it, and what dips below it gets a bank, level with
                // the water for a block, then 45°. (Ponds beside every step
                // of a profile stacked into terraces of water, and the
                // ledges holding them stood up through lakes: the owner's
                // walk.)
                let pooled = !under
                    && reach.kind != RiverKind::Wadi
                    && held <= level
                    && (level - reach.level.1.floor()).abs() < 0.5;
                if under {
                    // Nothing built under still water.
                } else if !pooled {
                    floor = floor.max(held - (give - BANK_SLOPE).max(0.0));
                } else if out >= POND_REACH {
                    let past = out - POND_REACH;
                    floor = floor.max(held - (past - 1.0).max(0.0) * BANK_SLOPE);
                } else if out >= 0.0 && pond.is_none_or(|p| level > p.1) {
                    pond = Some((out, level));
                }
                if out < 0.0 && best.is_none_or(|b| out < b.0) {
                    let depth = reach.depth.0 + (reach.depth.1 - reach.depth.0) * t;
                    let across = (d / half.max(0.5)).min(1.0);
                    let deep = (depth * (1.0 - across * across)).round().max(1.0);
                    let (bed, bar) = match reach.kind {
                        RiverKind::Torrent => (Bed::Gravel, false),
                        // Gravel bars between the braids: the bed at the
                        // water's level, dry.
                        // Bars only between the braids, never across the
                        // whole river: water runs along both banks (bars
                        // spanning it had cut it into pools, the owner's
                        // walk).
                        RiverKind::Braided => (
                            Bed::Gravel,
                            across < BAR_ACROSS && warp(x, z, 32.0).0 < BRAID_BARS,
                        ),
                        RiverKind::Wadi => (Bed::Sand, false),
                        RiverKind::Meandering if reach.slope > 0.002 => (Bed::Sand, false),
                        RiverKind::Meandering => (Bed::Silt, false),
                    };
                    h = if bar { level } else { level - deep };
                    if under {
                        h = h.min(ground as f32);
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
        if let Some((_, level, bed)) = best
            && drowned != Some(true)
        {
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
        // Within a lake's region, a river's banks keep its rim: cut below
        // the lake beside it, they let the lake out over air.
        if let Some(rim) = self.lake_level_at((ci, cj), (x, z), wobble)
            && ground >= rim
        {
            floor = floor.max(rim as f32);
        }
        let banked = h.min(ceiling).max(floor);
        let e = nearest.map_or(1.0, |n| (n.out / BANK_REACH).clamp(0.0, 1.0));
        let natural = e * e * (3.0 - 2.0 * e);
        // The blend eases cuts back into the land; it never lowers a bank
        // that holds water (a bank falls at 45° to the ground already).
        let h = (banked + (h - banked) * natural).max(floor).round() as i64;
        if let Some((_, level)) = pond
            && h < level as i64
        {
            return (
                h,
                Water {
                    level: level as i64,
                    body: Body::Lake,
                    near: nearest,
                },
            );
        }
        let (h, mut water) = self.still_water((ci, cj), (x, z), (h, ground), wobble, &ground_at);
        water.near = nearest;
        (h, water)
    }

    /// How far column `(x, z)` lies outside the region of cell `(i, j)`
    /// (both unwrapped), blocks: 0 inside it. A cell's region — where its
    /// lake, fill or salt flat reaches — is a disc about its centre, its
    /// radius wandering with the noise `wobble` (−1..1), not the cell's
    /// square: square regions drew lakes, salt flats and fills with straight
    /// edges and square terraces on the cell grid (the owner's walk). The
    /// discs of a block of cells cover it whole (REGION_RADIUS > √½).
    fn outside(&self, i: i64, j: i64, x: i64, z: i64, wobble: f64) -> f64 {
        let size = self.cell_x.min(self.cell_z);
        let (cx, cz) = (
            (i as f64 + 0.5) * self.cell_x,
            (j as f64 + 0.5) * self.cell_z,
        );
        let (dx, dz) = (x as f64 + 0.5 - cx, z as f64 + 0.5 - cz);
        let radius = size * (REGION_RADIUS + REGION_WOBBLE * wobble);
        ((dx * dx + dz * dz).sqrt() - radius).max(0.0)
    }

    /// Whether column `(x, z)`, of cell `(ci, cj)` (unwrapped), lies by the
    /// sea: within COAST_REACH cells of a sea cell's centre. Coasts and sea
    /// biomes are only here; land below sea level farther inland holds a
    /// lake.
    fn coastal(&self, (ci, cj): (i64, i64), (x, z): (i64, i64)) -> bool {
        let size = self.cell_x.min(self.cell_z);
        (-1..=1).any(|dj| {
            (-1..=1).any(|di| {
                let (i, j) = (ci + di, cj + dj);
                if !self.sea_at(self.wrap(i, j)) {
                    return false;
                }
                let (cx, cz) = (
                    (i as f64 + 0.5) * self.cell_x,
                    (j as f64 + 0.5) * self.cell_z,
                );
                let (dx, dz) = (x as f64 + 0.5 - cx, z as f64 + 0.5 - cz);
                (dx * dx + dz * dz).sqrt() <= COAST_REACH * size
            })
        })
    }

    /// Whether column `(x, z)` (any lap) lies by the sea ([`Self::coastal`]).
    pub fn is_coastal(&self, x: i64, z: i64) -> bool {
        let ci = (x as f64 / self.cell_x).floor() as i64;
        let cj = (z as f64 / self.cell_z).floor() as i64;
        self.coastal((ci, cj), (x, z))
    }

    /// The highest lake whose region holds column `(x, z)`, of cell
    /// `(ci, cj)` (unwrapped), if any.
    fn lake_level_at(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), wobble: f64) -> Option<i64> {
        let mut best = None;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                let c = self.wrap(i, j);
                if matches!(self.cover_at(c), Cover::Lake | Cover::SaltLake)
                    && self.outside(i, j, x, z, wobble) <= 0.0
                {
                    let level = i64::from(self.lvl(c));
                    best = Some(best.map_or(level, |b: i64| b.max(level)));
                }
            }
        }
        best
    }

    /// The natural ground at the edge of cell `(i, j)`'s region nearest
    /// column `(x, z)` (just inside it).
    fn edge_ground(
        &self,
        i: i64,
        j: i64,
        (x, z): (i64, i64),
        wobble: f64,
        ground_at: &dyn Fn(i64, i64) -> i64,
    ) -> i64 {
        let size = self.cell_x.min(self.cell_z);
        let (cx, cz) = (
            (i as f64 + 0.5) * self.cell_x,
            (j as f64 + 0.5) * self.cell_z,
        );
        let (dx, dz) = (x as f64 + 0.5 - cx, z as f64 + 0.5 - cz);
        let dist = (dx * dx + dz * dz).sqrt().max(1.0);
        let radius = size * (REGION_RADIUS + REGION_WOBBLE * wobble) - 1.0;
        let f = (radius / dist).min(1.0);
        ground_at((cx + dx * f).floor() as i64, (cz + dz * f).floor() as i64)
    }

    /// Whether any cell around `(ci, cj)` (unwrapped) holds still water,
    /// a fill or a salt flat — whether a column there needs its region
    /// noise.
    fn regions_near(&self, ci: i64, cj: i64) -> bool {
        (-1..=1).any(|dj| {
            (-1..=1).any(|di| {
                let c = self.wrap(ci + di, cj + dj);
                self.cover_at(c) != Cover::Land || self.lvl(c) > self.h(c)
            })
        })
    }

    /// The ground of column `(x, z)`, of cell `(ci, cj)` (unwrapped), after
    /// the sediment that fills hollows below a river's grade: a fill's cell
    /// is raised flat to its level, and its neighbours' low ground slopes
    /// down from it at 45° until it meets their own. (It had been raised
    /// flat to the fill's level wherever it lay below it, which ended in a
    /// cliff wherever the ground beyond the neighbours was lower — the
    /// owner's walk.) A ramp has fallen a cell's width by the time it leaves
    /// the cells considered, so the ground stays continuous.
    fn alluvium(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), h: i64, wobble: f64) -> i64 {
        let fill = |c: usize| {
            let level = i64::from(self.lvl(c));
            (self.cover_at(c) == Cover::Land && level > i64::from(self.h(c))).then_some(level)
        };
        let mut top = h;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                if let Some(l) = fill(self.wrap(i, j)) {
                    let d = self.outside(i, j, x, z, wobble);
                    top = top.max(l - (d * ALLUVIUM_SLOPE).floor() as i64);
                }
            }
        }
        // A fill's slope runs on under a lake or the sea beside it, but
        // never up out of the water: that would make the lake land.
        // Anywhere a lake's region reaches, not only its cells: fill slopes
        // had stood up through a salt lake as straight-edged rims.
        let here = self.wrap(ci, cj);
        let still = match self.cover_at(here) {
            Cover::Sea => Some(0),
            _ => self.lake_level_at((ci, cj), (x, z), wobble),
        };
        match still {
            Some(level) if h < level => top.min(level - 1).max(h),
            _ => top,
        }
    }

    /// Whether ground `h` at column `(x, z)`, of cell `(ci, cj)`
    /// (unwrapped), is a salt flat's crust: within SALT_FLAT_RISE of a
    /// closed basin's floor, in the basin's cell or as far as a lake
    /// reaches into its neighbours'. The crust follows the ground's
    /// contours (it had covered its cell's columns only, squares on the
    /// cell grid: the owner's "white patches").
    fn salt_flat(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), h: i64, wobble: f64) -> bool {
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                let c = self.wrap(i, j);
                if self.cover_at(c) == Cover::SaltFlat
                    && h <= i64::from(self.h(c)) + SALT_FLAT_RISE
                    && self.outside(i, j, x, z, wobble) <= 0.0
                {
                    return true;
                }
            }
        }
        false
    }

    /// Whether ground `h` at column `(x, z)`, of cell `(ci, cj)`
    /// (unwrapped), lies under a lake or the sea: below a lake's level in
    /// its cells or as far as it reaches into theirs neighbours', or below
    /// sea level by the sea.
    fn drowned(&self, (ci, cj): (i64, i64), (x, z): (i64, i64), h: i64, wobble: f64) -> bool {
        // Ground below sea level is under water wherever it is — the sea's,
        // or inland a lake at sea level (an inland pit, too small to be
        // sea): a river had been carried across one on a causeway.
        if h < 0 {
            return true;
        }
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                let c = self.wrap(i, j);
                let level = match self.cover_at(c) {
                    Cover::Lake | Cover::SaltLake => i64::from(self.lvl(c)),
                    Cover::Sea => 0,
                    _ => continue,
                };
                if h < level && self.outside(i, j, x, z, wobble) <= 0.0 {
                    return true;
                }
            }
        }
        false
    }

    /// The ground and the lake or salt flat at column `(x, z)`, of cell
    /// `(ci, cj)` (unwrapped), whose ground is `h`.
    ///
    /// A lake fills the natural ground below its level across its cells'
    /// regions (wavy discs, [`Self::outside`]) — the shore follows the
    /// ground's contours. Beyond, a bank holds it where it reaches the
    /// region's edge, level with it a block and then falling at 45°:
    /// the lake's level is its cells' centres' spill, and the ground at the
    /// far side of its neighbours can be lower still, where its water had
    /// stood over air (17 blocks of it at the bench's waterfall, the owner's
    /// walk). A lake's cells hold a higher neighbouring lake the same way.
    fn still_water(
        &self,
        (ci, cj): (i64, i64),
        (x, z): (i64, i64),
        (h, natural): (i64, i64),
        wobble: f64,
        ground_at: &dyn Fn(i64, i64) -> i64,
    ) -> (i64, Water) {
        let here = self.wrap(ci, cj);
        let lake_of = |c: usize| match self.cover_at(c) {
            Cover::Lake => Some((i64::from(self.lvl(c)), Body::Lake)),
            Cover::SaltLake => Some((i64::from(self.lvl(c)), Body::SaltLake)),
            _ => None,
        };
        let own = lake_of(here);
        let mut found = own;
        // Banks, by the level of the lake each holds: a bank only stands
        // where no lake as high already covers the column (a lake's own
        // cells' banks had stood up as ridges in its water).
        let mut banks: [(i64, i64); 9] = [(i64::MIN, i64::MIN); 9];
        let mut k = 0;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                let Some((level, body)) = lake_of(self.wrap(i, j)) else {
                    continue;
                };
                let d = self.outside(i, j, x, z, wobble);
                // A bank only where it would raise this ground and the lake
                // reaches its region's edge on this side — where the natural
                // ground there is below its level. (A bank standing over a
                // dry rim, where an outlet river cut its channel, made a
                // slot ten blocks deep, the owner's "insanely deep groove".)
                let mut bank = |level: i64| {
                    // Level with the water for a block, then falling at 45°.
                    let b = level - ((d - 1.0).max(0.0) * BANK_FALL).floor() as i64;
                    if b > h && self.edge_ground(i, j, (x, z), wobble, ground_at) < level {
                        banks[k] = (level, b);
                        k += 1;
                    }
                };
                match own {
                    // In a lake's own cells, only a higher lake beside it
                    // needs holding: from its edge.
                    Some((mine, _)) => {
                        if level > mine && d > 0.0 {
                            bank(level);
                        }
                    }
                    // Water where the natural ground lies below the lake —
                    // not where a river's cut lowered it, beside a lake it
                    // flows out of.
                    None if d <= 0.0 => {
                        if h < level && natural < level && found.is_none_or(|(l, _)| level < l) {
                            found = Some((level, body));
                        }
                    }
                    None => bank(level),
                }
            }
        }
        // The sea covers all ground below sea level (0), wherever it is.
        let sea = if h < 0 { 0 } else { i64::MIN };
        let wet = found
            .map_or(
                i64::MIN,
                |(level, _)| if h < level { level } else { i64::MIN },
            )
            .max(sea);
        let bank = banks[..k]
            .iter()
            .filter(|&&(level, _)| level > wet)
            .map(|&(_, b)| b)
            .max()
            .unwrap_or(i64::MIN);
        let h = h.max(bank);
        let water = match found {
            Some((level, body)) if h < level && natural < level => Water {
                level,
                body,
                near: None,
            },
            _ if self.salt_flat((ci, cj), (x, z), h, wobble) => Water {
                level: h,
                body: Body::SaltFlat,
                near: None,
            },
            // Inland, water below sea level is a lake at sea level, not the
            // sea (its shores had been classed as sea coast).
            _ if h < 0 && !self.coastal((ci, cj), (x, z)) => Water {
                level: 0,
                body: Body::Lake,
                near: None,
            },
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
        // The long profile, from stage 1; a reach that does not fall is
        // level throughout.
        let steps: [f32; PROFILE_STEPS] =
            match self.profile.binary_search_by_key(&(c as u32), |p| p.0) {
                Ok(k) => self.profile[k]
                    .1
                    .map(|e| lr + (lc - lr) * f32::from(e) / 255.0),
                Err(_) => [lc; PROFILE_STEPS],
            };
        // A waterfall: a step in the profile FALL_FROM blocks or more (the
        // ground stepping down), the highest; a wadi has no water to fall.
        let fall = (kind != RiverKind::Wadi)
            .then(|| {
                (0..PROFILE_STEPS)
                    .map(|k| {
                        let next = if k + 1 < PROFILE_STEPS {
                            steps[k + 1]
                        } else {
                            lr
                        };
                        (
                            steps[k].floor() - next.floor(),
                            (k + 1) as f32 / PROFILE_STEPS as f32,
                        )
                    })
                    .filter(|&(drop, _)| drop >= FALL_FROM)
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, t)| t)
            })
            .flatten();
        Some(Reach {
            steps,
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
