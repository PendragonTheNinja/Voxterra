//! The tectonic framework: stage 1's first map (milestone 16 task 1,
//! ADR-0025).
//!
//! Where the world's large features are, and why. Until milestone 16 the
//! terrain's continents, mountain belts and trenches were placed by
//! unrelated noise fields (`elevation.rs`), which gave a world good to walk
//! and gave geology nothing to stand on: no range was where it was for a
//! reason. This module is the reason. It works out, once per world, a
//! mosaic of **terranes** grouped into drifting **plates**; which terranes
//! are continent and how old each is; and what every boundary between two
//! of them is — an old suture, a collision, a subduction zone, a spreading
//! ridge, a rift, a quiet coast.
//!
//! **Nothing reads this yet.** Task 1 builds the framework and draws it
//! beside the existing terrain (`tectonics_map`); task 2 lets it place the
//! continents, belts and trenches, and is the task that changes the world.
//!
//! ## Two scales (ADR-0025 §1)
//!
//! A dozen plates with mountains on their edges would put ranges hours
//! apart, and ADR-0010 tuned them to forty minutes. Earth's continents are
//! not single slabs either: each is a mosaic of terranes welded together
//! over billions of years, with a worn-down range along every weld. So:
//!
//! - **Terranes** are [`TERRANE_BLOCKS`] across — ADR-0010's distance from
//!   one mountain belt to the next — in every world, whatever its size.
//!   Their boundaries are where belts can be.
//! - **Plates** are sets of terranes, about one per [`PLATE_BLOCKS`] square
//!   of world — ADR-0010's continent wavelength. Their boundaries are
//!   today's: the highest ranges, the trenches, the ocean ridges, the rifts.
//!
//! ## How it is computed
//!
//! Terranes are a jittered lattice: one seed point in each cell of a grid
//! that fits the torus a whole number of times, and a column belongs to
//! its nearest seed. That is a pure function of `(x, z)` — a search of the
//! cells round the column — so there is no grid to interpolate and nothing
//! to show through as squares (M14's lesson). Everything else is a table
//! over the terranes, written once at world creation: [`Tectonics::new`].
//!
//! ## Deterministic on every machine
//!
//! Hashes, integer lattice arithmetic, `+ − × ÷`, `floor` and `sqrt` — all
//! bit-exact under IEEE 754 — and no trigonometry: a drift direction is a
//! normalised pair of hashed numbers. The table is built sequentially, in
//! index order, with every tie broken by index.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use vox_core::WorldShape;

use crate::elevation::mix64;

/// The size of a terrane, blocks: ADR-0010's distance to the next mountain
/// belt. Landform scale — the same in every world.
pub const TERRANE_BLOCKS: i64 = 13_000;
/// One plate per square of this side, blocks: ADR-0010's continent
/// wavelength, so a plate is about as large as the continents already are.
pub const PLATE_BLOCKS: i64 = 56_000;
/// The fewest plates a world may have, and the fewest lattice cells per
/// axis (ADR-0012: every periodic field keeps at least three cells).
const MIN_PLATES: usize = 3;
const MIN_CELLS: i64 = 3;
/// Where in its lattice cell a terrane's seed may sit, as a fraction of
/// the cell. Not the whole cell: seeds at opposite corners of neighbouring
/// cells would make slivers.
const JITTER: (f64, f64) = (0.12, 0.88);
/// Cells searched either side of a column's own for its nearest seeds.
/// With seeds anywhere in their cells the nearest two are always within
/// two rings; one ring is not enough.
const SEARCH: i64 = 2;
/// How far a boundary may bend from the straight line between its seeds,
/// in cells, and the wavelengths of the bending, in cells.
const WARP_CELLS: f64 = 0.16;
const WARP_WAVELENGTHS: [f64; 2] = [1.3, 0.45];
const WARP_WEIGHTS: [f64; 2] = [1.0, 0.35];

/// The share of terranes that are continental crust: the land target (42%,
/// ADR-0012) and the shelves beyond the coast.
pub const CONTINENTAL_SHARE: f64 = 0.46;
/// Hotspots per plate, on average: a few in a world, never none.
const HOTSPOTS_PER_PLATE: f64 = 0.4;
/// The share of its plate a continent covers, least and most. Less than
/// all of it, so that one side of a continent is a passive margin.
const CONTINENT_OF_PLATE: (f64, f64) = (0.5, 0.9);
/// Relative drift across a boundary, as a share of the fastest possible,
/// beyond which plates are converging or diverging rather than sliding.
const CLOSING: f64 = 0.15;
/// How much the trend of the whole boundary between two plates counts
/// against one segment's own direction when judging which way the plates
/// meet there. A plate boundary is a smoother line than the mosaic it is
/// drawn along: with no trend, every kink in it would flip a subduction
/// zone to a rift and back within a terrane's width.
const TREND_WEIGHT: f64 = 2.0;

/// Half the width of a mountain belt, blocks: ADR-0010's range is 7 000
/// across.
pub const BELT_HALF_WIDTH: f64 = 3_500.0;
/// How far either side of a coast [`Tectonics::continentalness`] runs
/// before it is clamped, blocks. The coastal profile — lowland, shelf,
/// slope — is a blend about 4 200 blocks wide in today's terrain, two
/// thirds of it seaward, so this covers it with room to spare.
pub const COAST_REACH: f64 = 3_500.0;

/// What a world's creator may one day choose (the owner, 2026-11-10: build
/// the framework so that its tunables can become per-world settings). Today
/// every world uses [`Params::default`] and nothing is recorded in
/// `world.meta`; a setting added later records its value there and passes
/// it here, and this module needs no change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// See [`TERRANE_BLOCKS`].
    pub terrane_blocks: i64,
    /// See [`PLATE_BLOCKS`].
    pub plate_blocks: i64,
    /// See [`CONTINENTAL_SHARE`].
    pub continental_share: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            terrane_blocks: TERRANE_BLOCKS,
            plate_blocks: PLATE_BLOCKS,
            continental_share: CONTINENTAL_SHARE,
        }
    }
}

/// The world's six ages, oldest first (`docs/geology.md` §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Age {
    /// The Dawn Age: the first crust.
    Eoan = 1,
    /// The Age of Iron Seas.
    Ferrian,
    /// The Age of Old Seas: the old ranges were raised.
    Thalassian,
    /// The Sundering: a supercontinent rifted apart.
    Rhexian,
    /// The Age of Mountains: today's collisions.
    Orean,
    /// The Age of Ice, and the present.
    Nivean,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crust {
    Oceanic,
    Continental,
}

/// One piece of the mosaic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Terrane {
    /// The plate it rides on.
    pub plate: u32,
    pub crust: Crust,
    /// The continent it is part of, if it is continental.
    pub continent: Option<u32>,
    /// One of its continent's ancient cores.
    pub craton: bool,
    /// The order in which it was welded onto its continent: 0 for a
    /// craton, 1 for the first belt of terranes added round the cratons,
    /// and so on outward. Continents grow by accretion at their edges.
    pub step: u8,
    /// When it became part of its continent: a craton's own age, or the age
    /// in which it was welded on — or the Orean, whatever its history, if
    /// it is being deformed by a plate closing on it today. `None` for
    /// ocean floor, whose age is its distance from the ridge that made it.
    pub age: Option<Age>,
}

/// One plate: a set of terranes drifting together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plate {
    /// Drift, as a direction scaled by speed in `(0, 1]`.
    pub drift: (f64, f64),
    /// The terrane it was grown from.
    pub centre: u32,
    /// Breaks ties between like crust: where two oceanic plates converge,
    /// the one with the higher rank is the older, denser one, and goes
    /// under.
    pub rank: u32,
}

/// How two plates meet where they converge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convergence {
    /// Continent into continent: the highest ranges, no volcanoes.
    Collision,
    /// Ocean under continent: a coastal range crowned with volcanoes.
    ContinentalArc,
    /// Ocean under ocean: a chain of volcanic islands behind a trench.
    IslandArc,
}

/// What the boundary between two terranes is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// Nothing: two terranes of one craton, or ocean floor within a plate.
    Quiet,
    /// A continent's edge in the middle of a plate: a quiet coast with a
    /// wide shelf (the Atlantic's).
    PassiveMargin,
    /// The weld where younger ground was added to older, of the age it was
    /// made: an old mountain belt, worn down by how long ago that was.
    /// These run round a continent's cores, one belt outside another, as
    /// the Appalachians wrap the old heart of North America.
    Suture(Age),
    /// The joint between two terranes welded on in the same step, side by
    /// side. It runs across the belts, not along them, and was never a
    /// range: a fault zone, low hills at most.
    Seam(Age),
    /// Two plates converging. `first_over`: the first terrane of the pair
    /// asked about is on the overriding plate (the one that keeps its
    /// crust; the other goes under it).
    Convergent { kind: Convergence, first_over: bool },
    /// Two oceanic plates (or an ocean and a continent) parting: a
    /// spreading ridge.
    Ridge,
    /// A continent being pulled apart: a rift valley.
    Rift,
    /// Two plates sliding past each other.
    Transform,
}

impl Boundary {
    /// How much mountain this boundary raises, as a share of the most
    /// (ADR-0025 §4). A collision is the measure; a suture has worn down
    /// with its age, from high ground to hills.
    pub fn belt_strength(self) -> f64 {
        match self {
            Boundary::Convergent { kind, .. } => match kind {
                Convergence::Collision => 1.0,
                Convergence::ContinentalArc => 0.75,
                Convergence::IslandArc => 0.35,
            },
            Boundary::Suture(age) => match age {
                Age::Orean | Age::Nivean => 0.6,
                Age::Rhexian => 0.45,
                Age::Thalassian => 0.35,
                Age::Ferrian => 0.15,
                Age::Eoan => 0.1,
            },
            Boundary::Seam(age) => match age {
                Age::Orean | Age::Nivean => 0.2,
                Age::Rhexian | Age::Thalassian => 0.1,
                Age::Ferrian | Age::Eoan => 0.05,
            },
            Boundary::Quiet
            | Boundary::PassiveMargin
            | Boundary::Ridge
            | Boundary::Rift
            | Boundary::Transform => 0.0,
        }
    }
}

/// A plume of hot rock rising from deep in the mantle, fixed while the
/// plates drift over it: a volcano above it today, and a line of older,
/// dead ones carried away along its plate's drift (Hawaii and the chain
/// behind it). Milestone 17's volcanoes read these; nothing does yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotspot {
    /// Where the plume is, canonical blocks.
    pub x: i64,
    pub z: i64,
    /// The plate above it, whose drift is the direction of its track.
    pub plate: u32,
}

/// Where a column stands in the mosaic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    /// The terrane the column is in.
    pub terrane: u32,
    /// The terrane across the nearest boundary.
    pub neighbour: u32,
    /// Blocks to that boundary.
    pub edge: f64,
    /// What that boundary is, `terrane` first.
    pub boundary: Boundary,
}

/// Cells along a side of the window searched round a column, and seeds in
/// it.
const SIDE: usize = (2 * SEARCH + 1) as usize;
const WINDOW: usize = SIDE * SIDE;

/// The seeds in the window round one column: for each, in window order,
/// the square of its distance (nothing here needs the root) and its
/// terrane.
struct Window {
    /// Infinite for a seed of the outer ring that was not looked at.
    d2: [f64; WINDOW],
    terrane: [u32; WINDOW],
    /// Which of them is nearest.
    first: usize,
    /// The nearest continental and oceanic seeds: the bits of their
    /// squared distances, all ones where the window holds none.
    land2: u64,
    sea2: u64,
}

/// What a column needs to know about two seeds of its window: the belt
/// strength of the boundary between their terranes, and one over twice
/// the distance between the seeds — which turns the difference of two
/// squared distances into the distance from the line between them.
#[derive(Clone, Copy, PartialEq)]
struct Pair {
    strength: f64,
    half_inverse: f64,
}

/// One octave of the warp that bends boundaries: a lattice of hashed
/// displacements round the torus, written once so that a column reads
/// four of them and hashes nothing.
#[derive(PartialEq)]
struct Warp {
    cells: (usize, usize),
    /// Lattice cells per block, each axis.
    scale: (f64, f64),
    weight: f64,
    values: Vec<(f64, f64)>,
}

/// One column's search of the mosaic, from which terrain reads what it
/// is placed by (ADR-0025 §4): [`Column::continentalness`] always, and
/// [`Column::belt`] — the dearer of the two — only where there is land
/// for mountains to stand on.
pub struct Column<'a> {
    tectonics: &'a Tectonics,
    window: Window,
}

impl Column<'_> {
    /// See [`Tectonics::continentalness`].
    pub fn continentalness(&self) -> f64 {
        self.tectonics.inland(&self.window)
    }

    /// See [`Tectonics::belt`].
    pub fn belt(&self) -> f64 {
        self.tectonics.envelope(&self.window)
    }
}

/// A world's tectonic framework. See the module docs.
#[derive(PartialEq)]
pub struct Tectonics {
    seed: u64,
    shape: WorldShape,
    params: Params,
    size_x: f64,
    size_z: f64,
    /// Lattice cells per axis, and blocks per cell.
    nx: i64,
    nz: i64,
    cell_x: f64,
    cell_z: f64,
    /// Cells per block, each axis.
    per_block: (f64, f64),
    /// Each terrane's seed point, as a fraction of its lattice cell.
    jitter: Vec<(f64, f64)>,
    warp: Vec<Warp>,
    terranes: Vec<Terrane>,
    /// Read by every column, so kept apart from the terrane table. For
    /// each terrane, masks to OR into a squared distance's bits so that
    /// it counts toward the nearest continental seed and toward the
    /// nearest oceanic one: nothing and all ones, or the other way round
    /// — a choice made without a branch. And its [`Pair`] with each seed
    /// of the window centred on it.
    crust: Vec<(u64, u64)>,
    pairs: Vec<Pair>,
    /// A column whose nearest seed is within this (squared) needs only
    /// the window's inner nine cells: see [`Tectonics::window`].
    inner_enough: f64,
    plates: Vec<Plate>,
    /// For each ordered pair of plates that touch, the mean direction from
    /// the first to the second across the boundary they share: its length
    /// is 1 for a straight boundary and falls toward 0 for one that wraps
    /// round.
    trend: BTreeMap<(u32, u32), (f64, f64)>,
    hotspots: Vec<Hotspot>,
}

/// Its size, not its tables.
impl std::fmt::Debug for Tectonics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Tectonics({} terranes of ~{:.0} blocks on {} plates)",
            self.terranes.len(),
            self.cell_x,
            self.plates.len()
        )
    }
}

/// Lattice cells round an axis of `size` blocks: as near `terrane_blocks`
/// each as a whole number allows.
fn lattice_cells(size: i64, terrane_blocks: i64) -> i64 {
    ((size as f64 / terrane_blocks as f64 + 0.5).floor() as i64).max(MIN_CELLS)
}

/// Whether the window searched round a column holds every seed that can
/// matter to a belt or a coast there. The nearest seed is at most a jittered cell's
/// diagonal away; the ones that matter are within two half-widths of
/// that; and the nearest seed *outside* the window is two cells and a
/// bit away. True of every legal world size at the default terrane size
/// (`the_search_window_is_wide_enough`); a world option that shrank
/// terranes far enough would have to widen [`SEARCH`].
fn window_holds_every_belt(cell_x: f64, cell_z: f64) -> bool {
    let diagonal = (cell_x * cell_x + cell_z * cell_z).sqrt();
    (SEARCH as f64 + JITTER.0) * cell_x.min(cell_z)
        > JITTER.1 * diagonal + 2.0 * BELT_HALF_WIDTH.max(COAST_REACH)
}

/// A number in `[0, 1)` from a hash's top bits.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

fn smoothstep(lo: f64, hi: f64, v: f64) -> f64 {
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Tectonics {
    /// Work out a world's framework from its seed and size.
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        Self::with_params(seed, shape, Params::default())
    }

    /// [`Tectonics::new`] with other than the default tunables.
    pub fn with_params(seed: u64, shape: WorldShape, params: Params) -> Self {
        let (nx, nz) = (
            lattice_cells(shape.size_x(), params.terrane_blocks),
            lattice_cells(shape.size_z(), params.terrane_blocks),
        );
        let n = (nx * nz) as usize;
        let mut t = Self {
            seed,
            shape,
            params,
            size_x: shape.size_x() as f64,
            size_z: shape.size_z() as f64,
            nx,
            nz,
            cell_x: shape.size_x() as f64 / nx as f64,
            cell_z: shape.size_z() as f64 / nz as f64,
            per_block: (
                nx as f64 / shape.size_x() as f64,
                nz as f64 / shape.size_z() as f64,
            ),
            jitter: Vec::with_capacity(n),
            warp: Vec::new(),
            terranes: Vec::new(),
            crust: vec![(u64::MAX, 0); n],
            pairs: Vec::new(),
            inner_enough: 0.0,
            plates: Vec::new(),
            trend: BTreeMap::new(),
            hotspots: Vec::new(),
        };
        assert!(
            window_holds_every_belt(t.cell_x, t.cell_z),
            "terranes of {} x {} blocks are too small for belts {} wide",
            t.cell_x,
            t.cell_z,
            2.0 * BELT_HALF_WIDTH
        );
        // A seed outside the inner nine cells is more than a cell away
        // along one axis.
        let inner =
            (1.0 + JITTER.0) * t.cell_x.min(t.cell_z) - 2.0 * BELT_HALF_WIDTH.max(COAST_REACH);
        t.inner_enough = inner.max(0.0) * inner.max(0.0);
        for c in 0..n as u64 {
            let at = |salt: u64| JITTER.0 + (JITTER.1 - JITTER.0) * unit(t.hash(salt, c, 0));
            t.jitter.push((at(0x7EC7_0001), at(0x7EC7_0002)));
        }
        let mut norm = 0.0;
        for (k, (&wavelength, &weight)) in WARP_WAVELENGTHS.iter().zip(&WARP_WEIGHTS).enumerate() {
            let cells = |n: i64| ((n as f64 / wavelength + 0.5).floor() as i64).max(MIN_CELLS);
            let cells = (cells(nx) as usize, cells(nz) as usize);
            let at = |salt: u64, i: usize, j: usize| {
                2.0 * unit(t.hash(salt + 2 * k as u64, i as u64, j as u64)) - 1.0
            };
            let values = (0..cells.0 * cells.1)
                .map(|c| {
                    let (i, j) = (c % cells.0, c / cells.0);
                    (at(0x7EC7_0060, i, j), at(0x7EC7_0061, i, j))
                })
                .collect();
            norm += weight;
            t.warp.push(Warp {
                cells,
                scale: (cells.0 as f64 / t.size_x, cells.1 as f64 / t.size_z),
                weight,
                values,
            });
        }
        for octave in &mut t.warp {
            octave.weight *= WARP_CELLS / norm;
        }

        // Plates: well-spread centres, grown outward unevenly.
        let area = shape.size_x() as f64 * shape.size_z() as f64;
        let plate_side = params.plate_blocks as f64;
        let wanted = (area / (plate_side * plate_side) + 0.5).floor() as usize;
        let plate_count = wanted.clamp(MIN_PLATES, (n / 3).max(MIN_PLATES));
        let centres = t.spread_centres(plate_count);
        let plate_of = t.grow(&centres, &vec![true; n], usize::MAX, 0x7EC7_0010);
        for (p, &centre) in centres.iter().enumerate() {
            let (a, b) = (
                2.0 * unit(t.hash(0x7EC7_0020, p as u64, 0)) - 1.0,
                2.0 * unit(t.hash(0x7EC7_0021, p as u64, 0)) - 1.0,
            );
            let len = (a * a + b * b).sqrt();
            let (dx, dz) = if len > 1e-6 {
                (a / len, b / len)
            } else {
                (1.0, 0.0)
            };
            let speed = 0.4 + 0.6 * unit(t.hash(0x7EC7_0022, p as u64, 0));
            t.plates.push(Plate {
                drift: (dx * speed, dz * speed),
                centre: centre as u32,
                rank: (t.hash(0x7EC7_0023, p as u64, 0) >> 40) as u32,
            });
        }
        t.terranes = plate_of
            .iter()
            .map(|&p| Terrane {
                plate: p.expect("every terrane is reached from a plate centre") as u32,
                crust: Crust::Oceanic,
                continent: None,
                craton: false,
                step: 0,
                age: None,
            })
            .collect();
        t.trend = t.boundary_trends();

        t.raise_continents();
        t.date_continents();
        t.crust = t
            .terranes
            .iter()
            .map(|terrane| match terrane.crust {
                Crust::Continental => (0, u64::MAX),
                Crust::Oceanic => (u64::MAX, 0),
            })
            .collect();
        t.pairs = (0..n * WINDOW)
            .map(|at| {
                let (c, k) = (at / WINDOW, at % WINDOW);
                let (di, dj) = ((k % SIDE) as i64 - SEARCH, (k / SIDE) as i64 - SEARCH);
                let (i, j) = (
                    (c as i64 % nx + di).rem_euclid(nx),
                    (c as i64 / nx + dj).rem_euclid(nz),
                );
                let other = (j * nx + i) as usize;
                Pair {
                    strength: t.boundary(c as u32, other as u32).belt_strength(),
                    half_inverse: t.half_inverse(c, other, di, dj),
                }
            })
            .collect();
        t.place_hotspots();
        t
    }

    fn hash(&self, salt: u64, a: u64, b: u64) -> u64 {
        mix64(
            self.seed
                ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ a.wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ b.wrapping_mul(0xA076_1D64_78BD_642F),
        )
    }

    /// The eight lattice neighbours of a terrane, with whether each is
    /// diagonal. On a 3-cell axis a neighbour can repeat; callers do not
    /// mind.
    fn neighbours(&self, c: usize) -> [(usize, bool); 8] {
        let (i, j) = (c as i64 % self.nx, c as i64 / self.nx);
        let mut out = [(0, false); 8];
        let mut k = 0;
        for dj in -1..=1 {
            for di in -1..=1 {
                if di == 0 && dj == 0 {
                    continue;
                }
                let (ni, nj) = ((i + di).rem_euclid(self.nx), (j + dj).rem_euclid(self.nz));
                out[k] = ((nj * self.nx + ni) as usize, di != 0 && dj != 0);
                k += 1;
            }
        }
        out
    }

    /// `count` terranes spread as far from one another as the lattice
    /// allows: each next one is the terrane farthest from all chosen so
    /// far (ties to the lowest index).
    fn spread_centres(&self, count: usize) -> Vec<usize> {
        let n = (self.nx * self.nz) as usize;
        let lattice_distance = |a: usize, b: usize| {
            let wrap = |d: i64, period: i64| {
                let d = d.rem_euclid(period);
                d.min(period - d)
            };
            let dx = wrap(a as i64 % self.nx - b as i64 % self.nx, self.nx);
            let dz = wrap(a as i64 / self.nx - b as i64 / self.nx, self.nz);
            dx * dx + dz * dz
        };
        let first = (self.hash(0x7EC7_0030, 0, 0) % n as u64) as usize;
        let mut centres = vec![first];
        let mut nearest: Vec<i64> = (0..n).map(|c| lattice_distance(c, first)).collect();
        while centres.len() < count {
            let (mut best, mut best_d) = (0, -1);
            for (c, &d) in nearest.iter().enumerate() {
                if d > best_d {
                    (best, best_d) = (c, d);
                }
            }
            centres.push(best);
            for (c, d) in nearest.iter_mut().enumerate() {
                *d = (*d).min(lattice_distance(c, best));
            }
        }
        centres
    }

    /// Grow regions outward from `sources` over the terranes marked
    /// `allowed`, each step costing a hashed, uneven amount so the regions
    /// come out irregular: which source reached each terrane first, or
    /// `None` if none could. `limit` stops the growth after that many
    /// terranes have been claimed (the sources included).
    fn grow(
        &self,
        sources: &[usize],
        allowed: &[bool],
        limit: usize,
        salt: u64,
    ) -> Vec<Option<usize>> {
        let n = allowed.len();
        let mut owner: Vec<Option<usize>> = vec![None; n];
        let mut heap = BinaryHeap::new();
        for (s, &c) in sources.iter().enumerate() {
            heap.push(Reverse((0u64, c, s)));
        }
        let mut claimed = 0;
        while let Some(Reverse((cost, c, s))) = heap.pop() {
            if owner[c].is_some() || !allowed[c] {
                continue;
            }
            if claimed >= limit {
                break;
            }
            owner[c] = Some(s);
            claimed += 1;
            for (next, diagonal) in self.neighbours(c) {
                if owner[next].is_none() && allowed[next] {
                    let (lo, hi) = (c.min(next) as u64, c.max(next) as u64);
                    let step = 1_000 + self.hash(salt, lo, hi) % 1_600;
                    let step = if diagonal { step * 141 / 100 } else { step };
                    heap.push(Reverse((cost + step, next, s)));
                }
            }
        }
        owner
    }

    /// The unit direction from one terrane's seed to another's, the short
    /// way round the torus.
    fn direction(&self, a: u32, b: u32) -> (f64, f64) {
        let (ax, az) = self.seed_point(a);
        let (bx, bz) = self.seed_point(b);
        let short = |d: f64, period: f64| d - (d / period + 0.5).floor() * period;
        let (nx, nz) = (short(bx - ax, self.size_x), short(bz - az, self.size_z));
        let len = (nx * nx + nz * nz).sqrt();
        if len < 1e-9 {
            (0.0, 0.0)
        } else {
            (nx / len, nz / len)
        }
    }

    /// See the `trend` field.
    fn boundary_trends(&self) -> BTreeMap<(u32, u32), (f64, f64)> {
        let mut sum: BTreeMap<(u32, u32), (f64, f64, u32)> = BTreeMap::new();
        for c in 0..self.terranes.len() {
            for (m, _) in self.neighbours(c) {
                let (pc, pm) = (self.terranes[c].plate, self.terranes[m].plate);
                if pc != pm {
                    let (nx, nz) = self.direction(c as u32, m as u32);
                    let entry = sum.entry((pc, pm)).or_insert((0.0, 0.0, 0));
                    *entry = (entry.0 + nx, entry.1 + nz, entry.2 + 1);
                }
            }
        }
        sum.into_iter()
            .map(|(pair, (x, z, count))| (pair, (x / f64::from(count), z / f64::from(count))))
            .collect()
    }

    /// Decide which terranes are continent: plates taken in a hashed order
    /// each get one, grown from a core over part of the plate, until the
    /// world has its share of continental crust.
    fn raise_continents(&mut self) {
        let n = self.terranes.len();
        let share = self.params.continental_share;
        let target = ((n as f64 * share + 0.5).floor() as usize).clamp(1, n - 1);
        let mut order: Vec<usize> = (0..self.plates.len()).collect();
        order.sort_by_key(|&p| (self.hash(0x7EC7_0040, p as u64, 0), p));
        let mut raised = 0;
        for (continent, p) in order.into_iter().enumerate() {
            if raised >= target {
                break;
            }
            let members: Vec<usize> = (0..n)
                .filter(|&c| self.terranes[c].plate as usize == p)
                .collect();
            let share = CONTINENT_OF_PLATE.0
                + (CONTINENT_OF_PLATE.1 - CONTINENT_OF_PLATE.0)
                    * unit(self.hash(0x7EC7_0041, p as u64, 0));
            let quota = ((members.len() as f64 * share + 0.5).floor() as usize)
                .clamp(1, members.len())
                .min(target - raised);
            // The core is any terrane of the plate, by hash — usually off
            // its centre, so the continent reaches one edge of the plate
            // (an active margin) and not the other (a passive one).
            let core = *members
                .iter()
                .max_by_key(|&&c| (self.hash(0x7EC7_0042, c as u64, 0), Reverse(c)))
                .expect("a plate has at least its centre");
            let allowed: Vec<bool> = (0..n)
                .map(|c| self.terranes[c].plate as usize == p)
                .collect();
            let grown = self.grow(&[core], &allowed, quota, 0x7EC7_0043);
            for (c, owner) in grown.iter().enumerate() {
                if owner.is_some() {
                    self.terranes[c].crust = Crust::Continental;
                    self.terranes[c].continent = Some(continent as u32);
                    raised += 1;
                }
            }
        }
    }

    /// Give each continent its past: one to three cratons at its core, and
    /// every other terrane the age it was welded on — older near the
    /// cratons, and of the present age of mountain-building where it meets
    /// a plate that is closing on it today.
    fn date_continents(&mut self) {
        let n = self.terranes.len();
        let continents = self
            .terranes
            .iter()
            .filter_map(|t| t.continent)
            .max()
            .map_or(0, |c| c + 1);
        for continent in 0..continents {
            let members: Vec<usize> = (0..n)
                .filter(|&c| self.terranes[c].continent == Some(continent))
                .collect();
            let allowed: Vec<bool> = (0..n)
                .map(|c| self.terranes[c].continent == Some(continent))
                .collect();
            let core = *members
                .iter()
                .max_by_key(|&&c| (self.hash(0x7EC7_0042, c as u64, 0), Reverse(c)))
                .expect("a continent has a core");
            // Cratons: the core and its nearest one or two, in a small
            // continent just the core.
            let wanted = 1 + (self.hash(0x7EC7_0050, u64::from(continent), 0) % 3) as usize;
            let cratons = wanted.min((members.len() / 3).max(1));
            let nucleus = self.grow(&[core], &allowed, cratons, 0x7EC7_0051);
            let craton_cells: Vec<usize> = (0..n).filter(|&c| nucleus[c].is_some()).collect();
            let craton_age = if unit(self.hash(0x7EC7_0052, u64::from(continent), 0)) < 0.6 {
                Age::Eoan
            } else {
                Age::Ferrian
            };
            // Rings outward from the cratons, by lattice steps.
            let mut ring: Vec<Option<u32>> = vec![None; n];
            let mut frontier = craton_cells.clone();
            for &c in &craton_cells {
                ring[c] = Some(0);
            }
            let mut depth = 0;
            while !frontier.is_empty() {
                depth += 1;
                let mut next = Vec::new();
                for &c in &frontier {
                    for (m, _) in self.neighbours(c) {
                        if allowed[m] && ring[m].is_none() {
                            ring[m] = Some(depth);
                            next.push(m);
                        }
                    }
                }
                frontier = next;
            }
            // The first belt round an Eoan core was welded on in the
            // Ferrian (the old mobile belts); everything since, in the
            // Thalassian — the age the old ranges were raised.
            for &c in &members {
                let step = ring[c].expect("a continent is one piece, so every terrane is reached");
                let t = &mut self.terranes[c];
                t.step = step.min(255) as u8;
                t.craton = step == 0;
                t.age = Some(match step {
                    0 => craton_age,
                    1 if craton_age == Age::Eoan => Age::Ferrian,
                    _ => Age::Thalassian,
                });
            }
        }
        // A continental terrane on a closing plate boundary is being
        // deformed now, whatever its history (cratons excepted: they are
        // what the mountains are pushed against).
        for c in 0..n {
            if self.terranes[c].crust != Crust::Continental || self.terranes[c].craton {
                continue;
            }
            let active = self.neighbours(c).iter().any(|&(m, _)| {
                matches!(
                    self.boundary(c as u32, m as u32),
                    Boundary::Convergent { .. }
                )
            });
            if active {
                self.terranes[c].age = Some(Age::Orean);
            }
        }
    }

    /// Hotspots are where the mantle put them, which has nothing to do
    /// with the plates above: anywhere, by hash.
    fn place_hotspots(&mut self) {
        let count = ((self.plates.len() as f64 * HOTSPOTS_PER_PLATE + 0.5).floor() as u64).max(1);
        for k in 0..count {
            let x = (unit(self.hash(0x7EC7_0070, k, 0)) * self.size_x) as i64;
            let z = (unit(self.hash(0x7EC7_0071, k, 0)) * self.size_z) as i64;
            let plate = self.terranes[self.site(x, z).terrane as usize].plate;
            self.hotspots.push(Hotspot { x, z, plate });
        }
    }

    // --- The table, read ---------------------------------------------------

    pub fn params(&self) -> Params {
        self.params
    }

    pub fn hotspots(&self) -> &[Hotspot] {
        &self.hotspots
    }

    pub fn terranes(&self) -> &[Terrane] {
        &self.terranes
    }

    pub fn plates(&self) -> &[Plate] {
        &self.plates
    }

    /// Lattice cells per axis.
    pub fn cells(&self) -> (i64, i64) {
        (self.nx, self.nz)
    }

    /// A terrane's seed point, in canonical blocks.
    pub fn seed_point(&self, terrane: u32) -> (f64, f64) {
        let c = terrane as usize;
        let (i, j) = (c as i64 % self.nx, c as i64 / self.nx);
        (
            (i as f64 + self.jitter[c].0) * self.cell_x,
            (j as f64 + self.jitter[c].1) * self.cell_z,
        )
    }

    /// How fast two terranes' plates are closing across the line between
    /// them, in `[-2, 2]`: positive converging, negative parting.
    ///
    /// Measured across this pair's own line bent toward the trend of the
    /// whole boundary ([`TREND_WEIGHT`]), so a boundary keeps its
    /// character along its length and changes it only where it truly
    /// turns — as the Pacific's edge slides past California and dives
    /// under Alaska.
    pub fn closing(&self, a: u32, b: u32) -> f64 {
        let (ia, ib) = (
            self.terranes[a as usize].plate,
            self.terranes[b as usize].plate,
        );
        let (pa, pb) = (self.plates[ia as usize], self.plates[ib as usize]);
        let own = self.direction(a, b);
        // Terranes that are near each other without being lattice
        // neighbours can be on plates with no recorded trend.
        let trend = self.trend.get(&(ia, ib)).copied().unwrap_or((0.0, 0.0));
        let (nx, nz) = (
            own.0 + TREND_WEIGHT * trend.0,
            own.1 + TREND_WEIGHT * trend.1,
        );
        let len = (nx * nx + nz * nz).sqrt();
        if len < 1e-9 {
            return 0.0;
        }
        ((pa.drift.0 - pb.drift.0) * nx + (pa.drift.1 - pb.drift.1) * nz) / len
    }

    /// What the boundary between two terranes is, `a` first
    /// (`docs/geology.md` §4's table).
    pub fn boundary(&self, a: u32, b: u32) -> Boundary {
        let (ta, tb) = (self.terranes[a as usize], self.terranes[b as usize]);
        if a == b {
            return Boundary::Quiet;
        }
        let both_continent = ta.crust == Crust::Continental && tb.crust == Crust::Continental;
        let both_ocean = ta.crust == Crust::Oceanic && tb.crust == Crust::Oceanic;
        if ta.plate == tb.plate {
            return if both_ocean || (ta.craton && tb.craton) {
                Boundary::Quiet
            } else if both_continent {
                // Welded when the younger of the two arrived.
                let age = ta.age.max(tb.age).expect("continental terranes are dated");
                if ta.step == tb.step {
                    Boundary::Seam(age)
                } else {
                    Boundary::Suture(age)
                }
            } else {
                Boundary::PassiveMargin
            };
        }
        let closing = self.closing(a, b);
        if closing > CLOSING {
            let (ra, rb) = (
                self.plates[ta.plate as usize].rank,
                self.plates[tb.plate as usize].rank,
            );
            // Continental crust is too light to go under; between like
            // crust the lower-ranked (younger, lighter) plate stays up.
            let (kind, first_over) = if both_continent {
                (Convergence::Collision, (ra, ta.plate) < (rb, tb.plate))
            } else if both_ocean {
                (Convergence::IslandArc, (ra, ta.plate) < (rb, tb.plate))
            } else {
                (Convergence::ContinentalArc, ta.crust == Crust::Continental)
            };
            Boundary::Convergent { kind, first_over }
        } else if closing < -CLOSING {
            // Ocean against continent parting is a young ocean, its ridge
            // still close to the coast it split from (the Red Sea).
            if both_continent {
                Boundary::Rift
            } else {
                Boundary::Ridge
            }
        } else {
            Boundary::Transform
        }
    }

    // --- Per column --------------------------------------------------------

    /// A column on the world's first lap. This is where the framework
    /// meets the torus (CLAUDE.md: terrain generation canonicalises), so
    /// that every lap gives the same bits.
    fn canonical(&self, x: i64, z: i64) -> (i64, i64) {
        (self.shape.canonical_x(x), self.shape.canonical_z(z))
    }

    /// A canonical column, displaced so that boundaries bend.
    fn warped(&self, x: i64, z: i64) -> (f64, f64) {
        let (x, z) = (x as f64, z as f64);
        let (mut dx, mut dz) = (0.0, 0.0);
        for octave in &self.warp {
            let (cx, cz) = octave.cells;
            let (u, v) = (x * octave.scale.0, z * octave.scale.1);
            // Not negative, so truncating is `floor`.
            let (i0, j0) = ((u as usize).min(cx - 1), (v as usize).min(cz - 1));
            let (i1, j1) = (
                if i0 + 1 == cx { 0 } else { i0 + 1 },
                if j0 + 1 == cz { 0 } else { j0 + 1 },
            );
            let ease = |t: f64| t * t * (3.0 - 2.0 * t);
            let (su, sv) = (ease(u - i0 as f64), ease(v - j0 as f64));
            let (a, b) = (octave.values[j0 * cx + i0], octave.values[j0 * cx + i1]);
            let (c, d) = (octave.values[j1 * cx + i0], octave.values[j1 * cx + i1]);
            let mix = |a: f64, b: f64, c: f64, d: f64| {
                let (top, bottom) = (a + (b - a) * su, c + (d - c) * su);
                top + (bottom - top) * sv
            };
            dx += octave.weight * mix(a.0, b.0, c.0, d.0);
            dz += octave.weight * mix(a.1, b.1, c.1, d.1);
        }
        (x + dx * self.cell_x, z + dz * self.cell_z)
    }

    /// The seeds in the window round a canonical column.
    ///
    /// On an axis of three or four cells the window laps the world and
    /// meets a terrane twice. Both images are kept: on a torus that small
    /// a terrane does border its neighbour on both sides.
    ///
    /// This is the framework's inner loop — it runs for every column of
    /// terrain — so it divides nothing, wraps the lattice once per axis
    /// and not once per seed, takes no square roots, branches on nothing
    /// it cannot predict, and stops early when it can. A seed of the
    /// outer ring is more than a cell away; so where the nearest seed is
    /// close enough ([`Tectonics::inner_enough`]), no outer seed can be
    /// within two belt half-widths of nearest, nor near enough to bring
    /// the coast within [`COAST_REACH`], and unless `whole` is asked for
    /// the outer ring is left unread. Nothing the fields return changes
    /// (`the_short_search_changes_nothing`); most columns take it.
    fn window(&self, x: i64, z: i64, whole: bool) -> Window {
        let (wx, wz) = self.warped(x, z);
        // In cells. The warp can carry a column a little below zero; one
        // cell's offset keeps the truncation a `floor`.
        let (ux, uz) = (wx * self.per_block.0 + 1.0, wz * self.per_block.1 + 1.0);
        let (ci, cj) = (ux as i64 - 1 - SEARCH, uz as i64 - 1 - SEARCH);
        // Where the column is from the window's corner, less each seed's
        // cell: all that is left to subtract is the seed's own jitter.
        let from = |u: f64, c: i64| {
            let mut k = -1.0;
            [(); SIDE].map(|()| {
                k += 1.0;
                u - 1.0 - c as f64 - k
            })
        };
        let (fx, fz) = (from(ux, ci), from(uz, cj));
        // The window's first cell is never more than one lap off the
        // lattice, either way.
        let wrapped = |first: i64, cells: i64| {
            let mut at = if first < 0 { first + cells } else { first };
            [(); SIDE].map(|()| {
                let here = if at >= cells { at - cells } else { at };
                at = here + 1;
                here as usize
            })
        };
        let (columns, rows) = (wrapped(ci, self.nx), wrapped(cj, self.nz));
        let (sx, sz) = (self.cell_x * self.cell_x, self.cell_z * self.cell_z);
        let mut d2s = [f64::INFINITY; WINDOW];
        let mut terranes = [0u32; WINDOW];
        // Three running minima, kept as integers: a distance is not
        // negative, so its bits order as an integer does, and an integer
        // minimum is one cheap step where a floating one is a chain of
        // them. The nearest carries its place in the window in its low
        // five bits.
        let mut least = [u64::MAX; 3];
        let mut look = |di: usize, dj: usize, least: &mut [u64; 3]| {
            let c = rows[dj] * self.nx as usize + columns[di];
            let k = dj * SIDE + di;
            let (jx, jz) = self.jitter[c];
            let (ax, az) = (fx[di] - jx, fz[dj] - jz);
            let d2 = ax * ax * sx + az * az * sz;
            d2s[k] = d2;
            terranes[k] = c as u32;
            let bits = d2.to_bits();
            least[0] = least[0].min((bits & !31) | k as u64);
            least[1] = least[1].min(bits | self.crust[c].0);
            least[2] = least[2].min(bits | self.crust[c].1);
        };
        let inner = 1..SIDE - 1;
        for dj in inner.clone() {
            for di in inner.clone() {
                look(di, dj, &mut least);
            }
        }
        if whole || f64::from_bits(least[0] & !31) > self.inner_enough {
            for dj in 0..SIDE {
                for di in 0..SIDE {
                    if !(inner.contains(&di) && inner.contains(&dj)) {
                        look(di, dj, &mut least);
                    }
                }
            }
        }
        Window {
            d2: d2s,
            terrane: terranes,
            first: (least[0] & 31) as usize,
            land2: least[1],
            sea2: least[2],
        }
    }

    /// One over twice the distance between two seeds, the second `di`
    /// and `dj` cells from the first on the unwrapped lattice. Nothing
    /// for a seed and itself.
    fn half_inverse(&self, a: usize, b: usize, di: i64, dj: i64) -> f64 {
        let dx = (di as f64 + self.jitter[b].0 - self.jitter[a].0) * self.cell_x;
        let dz = (dj as f64 + self.jitter[b].1 - self.jitter[a].1) * self.cell_z;
        let apart = (dx * dx + dz * dz).sqrt();
        if apart > 0.0 { 0.5 / apart } else { 0.0 }
    }

    /// The [`Pair`] of two seeds of one window: from the table where the
    /// second is within a window's reach of the first on the lattice
    /// (every pair that shares a boundary is), and worked out where it is
    /// not.
    fn pair(&self, window: &Window, a: usize, b: usize) -> Pair {
        let di = (b % SIDE) as i64 - (a % SIDE) as i64;
        let dj = (b / SIDE) as i64 - (a / SIDE) as i64;
        let (ta, tb) = (window.terrane[a] as usize, window.terrane[b] as usize);
        if di.abs() <= SEARCH && dj.abs() <= SEARCH {
            self.pairs[ta * WINDOW + ((dj + SEARCH) * SIDE as i64 + di + SEARCH) as usize]
        } else {
            Pair {
                strength: self.boundary(ta as u32, tb as u32).belt_strength(),
                half_inverse: self.half_inverse(ta, tb, di, dj),
            }
        }
    }

    /// Where a column stands: its terrane, the nearest boundary and what
    /// that boundary is. The column may be on any lap of the world.
    pub fn site(&self, x: i64, z: i64) -> Site {
        let (x, z) = self.canonical(x, z);
        let window = self.window(x, z, true);
        let own = window.first;
        let (mut edge, mut neighbour) = (f64::MAX, u32::MAX);
        for k in (0..WINDOW).filter(|&k| window.terrane[k] != window.terrane[own]) {
            // How far the column is from the line between the two seeds.
            let d = (window.d2[k] - window.d2[own]) * self.pair(&window, own, k).half_inverse;
            if (d, window.terrane[k]) < (edge, neighbour) {
                (edge, neighbour) = (d, window.terrane[k]);
            }
        }
        Site {
            terrane: window.terrane[own],
            neighbour,
            edge,
            boundary: self.boundary(window.terrane[own], neighbour),
        }
    }

    /// How far inside a continent a column is, blocks: positive on
    /// continental crust, negative on oceanic, zero on the line between,
    /// and clamped at [`COAST_REACH`] either way — what the continent
    /// field is made from (ADR-0025 §4).
    ///
    /// Half the difference of the distances to the nearest seed of each
    /// crust. That is exact on the straight line between the two seeds and
    /// reads up to ~30% short toward the ends of the boundary between
    /// them, so a shelf's width varies that much along a coast — and it is
    /// continuous everywhere, which the exact distance to one bisector is
    /// not where the nearest terrane changes.
    pub fn continentalness(&self, x: i64, z: i64) -> f64 {
        let (x, z) = self.canonical(x, z);
        self.inland(&self.window(x, z, false))
    }

    fn inland(&self, window: &Window) -> f64 {
        // A seed the window does not hold is farther than its reach, and
        // counts as that far, so the value cannot jump as one comes in.
        let far = (SEARCH as f64 + JITTER.0) * self.cell_x.min(self.cell_z);
        let far = (far * far).to_bits();
        let (land, sea) = (
            f64::from_bits(window.land2.min(far)),
            f64::from_bits(window.sea2.min(far)),
        );
        ((sea.sqrt() - land.sqrt()) * 0.5).clamp(-COAST_REACH, COAST_REACH)
    }

    /// How strongly mountains are raised at a column, in `[0, 1]`: the
    /// strongest of the boundaries near it, each falling away across
    /// [`BELT_HALF_WIDTH`] (ADR-0025 §4 — the envelope that replaces the
    /// belt-gate noise).
    ///
    /// Every pair of terranes near the column is a candidate boundary,
    /// and its line is the two seeds' bisector — which runs on for ever,
    /// while the boundary itself stops where a third terrane comes
    /// between the two. So each pair is also weighted by how far past
    /// that end the column is: the most that any third terrane is nearer
    /// than one of the pair, measured across *their* bisector. Without
    /// that, belts would cross the insides of terranes; with it a belt
    /// ends in a rounded cap at each junction.
    ///
    /// The field is continuous everywhere, and that takes care. Every
    /// term is a property of a set of terranes and the column, never of
    /// the order the terranes come in; and the terranes considered are
    /// all those within two half-widths of being nearest — a terrane
    /// farther than that can neither be half of a boundary that reaches
    /// the column nor come between two that are (the continuity test
    /// found both mistakes in simpler versions of this function).
    pub fn belt(&self, x: i64, z: i64) -> f64 {
        let (x, z) = self.canonical(x, z);
        self.envelope(&self.window(x, z, false))
    }

    /// One search of the mosaic for a column, for both
    /// [`Tectonics::continentalness`] and [`Tectonics::belt`].
    pub fn column(&self, x: i64, z: i64) -> Column<'_> {
        let (x, z) = self.canonical(x, z);
        self.column_canonical(x, z)
    }

    /// [`Tectonics::column`] for a column already on the world's first
    /// lap, as the terrain's own are when it asks.
    pub(crate) fn column_canonical(&self, x: i64, z: i64) -> Column<'_> {
        Column {
            tectonics: self,
            window: self.window(x, z, false),
        }
    }

    fn envelope(&self, window: &Window) -> f64 {
        // The seeds within two half-widths of being nearest.
        let limit = window.d2[window.first].sqrt() + 2.0 * BELT_HALF_WIDTH;
        let limit = limit * limit;
        let mut near = [0usize; WINDOW];
        let mut count = 0;
        for k in 0..WINDOW {
            // Written every time and kept only if wanted: no branch.
            near[count] = k;
            count += usize::from(window.d2[k] < limit);
        }
        let near = &near[..count];
        let mut strongest = 0.0f64;
        for (at, &a) in near.iter().enumerate() {
            for &b in &near[at + 1..] {
                let pair = self.pair(window, a, b);
                if pair.strength <= strongest {
                    continue;
                }
                // How far the column is from the line between the two.
                let across = ((window.d2[b] - window.d2[a]) * pair.half_inverse).abs();
                if across >= BELT_HALF_WIDTH {
                    continue;
                }
                // And how far past the end of their boundary: the most
                // that any third seed is nearer than either.
                let mut beyond = 0.0f64;
                for &j in near.iter().filter(|&&j| j != a && j != b) {
                    for of in [a, b] {
                        let past =
                            (window.d2[of] - window.d2[j]) * self.pair(window, j, of).half_inverse;
                        if past > beyond {
                            beyond = past;
                        }
                    }
                }
                let here = (1.0 - smoothstep(0.0, BELT_HALF_WIDTH, across))
                    * (1.0 - smoothstep(0.0, BELT_HALF_WIDTH, beyond));
                strongest = strongest.max(pair.strength * here);
            }
        }
        strongest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{elevation_colour, write_png};
    use vox_core::{MAX_WORLD_SIZE_BLOCKS, MIN_WORLD_SIZE_BLOCKS};

    const SEEDS: u64 = 16;

    fn world(seed: u64) -> Tectonics {
        Tectonics::new(seed, WorldShape::DEFAULT)
    }

    fn square(size: i64) -> WorldShape {
        WorldShape::new(size, size).unwrap()
    }

    /// Every pair of terranes that are lattice neighbours, both ways round.
    fn adjacent(t: &Tectonics) -> Vec<(u32, u32)> {
        (0..t.terranes().len())
            .flat_map(|c| t.neighbours(c).map(move |(m, _)| (c as u32, m as u32)))
            .filter(|&(a, b)| a != b)
            .collect()
    }

    /// Whether the terranes passing `member` are one piece on the lattice.
    fn one_piece(t: &Tectonics, member: impl Fn(usize) -> bool) -> bool {
        let n = t.terranes().len();
        let Some(start) = (0..n).find(|&c| member(c)) else {
            return true;
        };
        let mut seen = vec![false; n];
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            for (m, _) in t.neighbours(c) {
                if member(m) && !seen[m] {
                    seen[m] = true;
                    stack.push(m);
                }
            }
        }
        (0..n).all(|c| !member(c) || seen[c])
    }

    #[test]
    fn the_same_seed_and_size_give_the_same_framework() {
        assert!(world(7) == world(7));
        assert!(world(7) != world(8));
        assert!(world(7) != Tectonics::new(7, square(MIN_WORLD_SIZE_BLOCKS)));
        assert!(world(7) == Tectonics::with_params(7, WorldShape::DEFAULT, Params::default()));
    }

    /// The lattice fits the torus a whole number of times, at least three
    /// (ADR-0012), with cells near a terrane's size; plates follow the
    /// world's area and never outnumber a third of the terranes.
    #[test]
    fn the_mosaic_fits_every_world_size() {
        let small = Tectonics::new(1, square(MIN_WORLD_SIZE_BLOCKS));
        assert_eq!(small.cells(), (3, 3));
        assert_eq!(small.plates().len(), 3);
        let default = world(1);
        assert_eq!(default.cells(), (16, 16));
        assert_eq!(default.plates().len(), 13);
        let long = Tectonics::new(1, WorldShape::new(MIN_WORLD_SIZE_BLOCKS, 204_800).unwrap());
        assert_eq!(long.cells(), (3, 16));
        assert!((3..=16).contains(&long.plates().len()));
        for size in [MIN_WORLD_SIZE_BLOCKS, 40_960, 98_304, 204_800, 409_600] {
            let t = Tectonics::new(3, square(size));
            let (nx, nz) = t.cells();
            assert_eq!(t.terranes().len() as i64, nx * nz);
            assert!(nx >= 3 && nz >= 3);
            let cell = size as f64 / nx as f64;
            assert!(
                (cell / TERRANE_BLOCKS as f64 - 1.0).abs() < 0.2,
                "{size}: a terrane is {cell} blocks"
            );
            assert!(t.plates().len() >= 3 && t.plates().len() * 3 <= t.terranes().len());
        }
    }

    /// Every pair of legal axis lengths, so every legal world.
    #[test]
    fn the_search_window_is_wide_enough() {
        let quantum = vox_core::WORLD_SIZE_QUANTUM_BLOCKS;
        let cells: Vec<f64> = (MIN_WORLD_SIZE_BLOCKS / quantum..=MAX_WORLD_SIZE_BLOCKS / quantum)
            .map(|q| (q * quantum) as f64 / lattice_cells(q * quantum, TERRANE_BLOCKS) as f64)
            .collect();
        for &x in &cells {
            for &z in &cells {
                assert!(window_holds_every_belt(x, z), "cells of {x} x {z} blocks");
            }
        }
    }

    /// The largest legal world is 26 000 terranes on 1 400 plates; building
    /// it must stay a matter of seconds and megabytes.
    #[test]
    fn the_largest_world_builds() {
        let t = Tectonics::new(5, square(MAX_WORLD_SIZE_BLOCKS));
        assert_eq!(t.cells(), (161, 161));
        assert!(t.plates().len() > 1_000);
        assert!(t.belt(123_456, 654_321) >= 0.0);
    }

    /// A world made with other tunables is a different, coarser or finer
    /// mosaic — the path a future world-creation setting takes.
    #[test]
    fn the_tunables_are_per_world() {
        let coarse = Tectonics::with_params(
            2,
            WorldShape::DEFAULT,
            Params {
                terrane_blocks: 2 * TERRANE_BLOCKS,
                plate_blocks: 2 * PLATE_BLOCKS,
                continental_share: 0.7,
            },
        );
        assert_eq!(coarse.cells(), (8, 8));
        assert_eq!(coarse.plates().len(), 3);
        let land = coarse
            .terranes()
            .iter()
            .filter(|t| t.crust == Crust::Continental)
            .count();
        assert_eq!(land, 45, "0.7 of 64 terranes");
    }

    #[test]
    fn every_plate_is_one_piece_and_has_its_centre() {
        for seed in 0..SEEDS {
            let t = world(seed);
            for (p, plate) in t.plates().iter().enumerate() {
                assert_eq!(t.terranes()[plate.centre as usize].plate as usize, p);
                assert!(
                    one_piece(&t, |c| t.terranes()[c].plate as usize == p),
                    "seed {seed}: plate {p} is in pieces"
                );
                let speed = (plate.drift.0 * plate.drift.0 + plate.drift.1 * plate.drift.1).sqrt();
                assert!((0.4..=1.0 + 1e-9).contains(&speed));
            }
        }
    }

    /// Continents: the world's share of continental crust; each one piece,
    /// on one plate, grown outward from one to three cratons.
    #[test]
    fn continents_are_grown_from_cratons() {
        for seed in 0..SEEDS {
            let t = world(seed);
            let n = t.terranes().len();
            let land = t
                .terranes()
                .iter()
                .filter(|t| t.crust == Crust::Continental)
                .count();
            assert_eq!(
                land,
                (n as f64 * CONTINENTAL_SHARE + 0.5) as usize,
                "seed {seed}"
            );
            let continents = t
                .terranes()
                .iter()
                .filter_map(|t| t.continent)
                .max()
                .unwrap()
                + 1;
            assert!(
                (4..=13).contains(&continents),
                "seed {seed}: {continents} continents"
            );
            for k in 0..continents {
                let members: Vec<usize> = (0..n)
                    .filter(|&c| t.terranes()[c].continent == Some(k))
                    .collect();
                assert!(!members.is_empty());
                assert!(one_piece(&t, |c| t.terranes()[c].continent == Some(k)));
                let plate = t.terranes()[members[0]].plate;
                let cratons = members.iter().filter(|&&c| t.terranes()[c].craton).count();
                assert!((1..=3).contains(&cratons), "seed {seed}: {cratons} cratons");
                for &c in &members {
                    let terrane = t.terranes()[c];
                    assert_eq!(terrane.plate, plate, "a continent rides one plate");
                    assert_eq!(terrane.crust, Crust::Continental);
                    assert_eq!(terrane.craton, terrane.step == 0);
                    let age = terrane.age.expect("continental crust is dated");
                    if terrane.craton {
                        assert!(age <= Age::Ferrian, "a craton is of the first two ages");
                    } else {
                        // Accretion: it was welded onto something already
                        // there.
                        assert!(
                            t.neighbours(c).iter().any(|&(m, _)| {
                                t.terranes()[m].continent == Some(k)
                                    && t.terranes()[m].step + 1 == terrane.step
                            }),
                            "seed {seed}: terrane {c} was welded onto nothing"
                        );
                        assert!(age >= Age::Ferrian);
                    }
                }
            }
            for terrane in t.terranes().iter().filter(|t| t.crust == Crust::Oceanic) {
                assert_eq!(
                    (terrane.continent, terrane.craton, terrane.age),
                    (None, false, None)
                );
            }
        }
    }

    /// `docs/geology.md` §4's table, pair by pair.
    #[test]
    fn every_boundary_follows_from_crust_and_drift() {
        for seed in 0..SEEDS {
            let t = world(seed);
            for (a, b) in adjacent(&t) {
                let (ta, tb) = (t.terranes()[a as usize], t.terranes()[b as usize]);
                let land = |t: Terrane| t.crust == Crust::Continental;
                let boundary = t.boundary(a, b);
                // The same boundary from the other side.
                let mirrored = match t.boundary(b, a) {
                    Boundary::Convergent { kind, first_over } => Boundary::Convergent {
                        kind,
                        first_over: !first_over,
                    },
                    other => other,
                };
                assert_eq!(boundary, mirrored, "seed {seed}: {a}|{b}");
                assert!((t.closing(a, b) - t.closing(b, a)).abs() < 1e-12);
                if ta.plate == tb.plate {
                    match boundary {
                        Boundary::Quiet => {
                            assert!((!land(ta) && !land(tb)) || (ta.craton && tb.craton));
                        }
                        Boundary::PassiveMargin => assert!(land(ta) != land(tb)),
                        Boundary::Suture(age) => {
                            assert!(land(ta) && land(tb) && ta.step != tb.step);
                            assert_eq!(Some(age), ta.age.max(tb.age));
                        }
                        Boundary::Seam(age) => {
                            assert!(land(ta) && land(tb) && ta.step == tb.step && !ta.craton);
                            assert_eq!(Some(age), ta.age.max(tb.age));
                        }
                        other => panic!("seed {seed}: {other:?} inside a plate"),
                    }
                    continue;
                }
                let closing = t.closing(a, b);
                match boundary {
                    Boundary::Convergent { kind, first_over } => {
                        assert!(closing > CLOSING);
                        match kind {
                            Convergence::Collision => assert!(land(ta) && land(tb)),
                            Convergence::IslandArc => assert!(!land(ta) && !land(tb)),
                            // Continental crust never goes under.
                            Convergence::ContinentalArc => {
                                assert!(land(ta) != land(tb));
                                assert_eq!(first_over, land(ta));
                            }
                        }
                    }
                    Boundary::Rift => assert!(closing < -CLOSING && land(ta) && land(tb)),
                    Boundary::Ridge => assert!(closing < -CLOSING && !(land(ta) && land(tb))),
                    Boundary::Transform => assert!(closing.abs() <= CLOSING),
                    other => panic!("seed {seed}: {other:?} between plates"),
                }
            }
        }
    }

    /// Ground on a closing plate boundary is being deformed today,
    /// whatever its history — cratons excepted.
    #[test]
    fn active_margins_are_of_the_age_of_mountains() {
        let mut active = 0;
        for seed in 0..SEEDS {
            let t = world(seed);
            for (a, b) in adjacent(&t) {
                let terrane = t.terranes()[a as usize];
                if matches!(t.boundary(a, b), Boundary::Convergent { .. })
                    && terrane.crust == Crust::Continental
                    && !terrane.craton
                {
                    assert_eq!(terrane.age, Some(Age::Orean), "seed {seed}: terrane {a}");
                    active += 1;
                }
            }
        }
        assert!(
            active > 100,
            "only {active} active margins in {SEEDS} worlds"
        );
    }

    /// A default world has coasts of both kinds, mountains being built,
    /// sea floor being made and old welds; the rarer kinds turn up across
    /// a handful of worlds.
    #[test]
    fn a_world_has_every_kind_of_boundary() {
        let mut ever = [false; 10];
        for seed in 0..SEEDS {
            let t = world(seed);
            let mut here = [false; 10];
            for (a, b) in adjacent(&t) {
                here[kind_index(t.boundary(a, b))] = true;
            }
            for kind in [
                "passive",
                "suture",
                "seam",
                "cont.arc",
                "ridge",
                "transform",
            ] {
                let k = KINDS.iter().position(|&name| name == kind).unwrap();
                assert!(here[k], "seed {seed} has no {kind} boundary");
            }
            for k in 0..10 {
                ever[k] |= here[k];
            }
            assert!(!t.hotspots().is_empty());
            for spot in t.hotspots() {
                assert!((0..204_800).contains(&spot.x) && (0..204_800).contains(&spot.z));
                let under = t.terranes()[t.site(spot.x, spot.z).terrane as usize].plate;
                assert_eq!(spot.plate, under);
            }
        }
        assert_eq!(ever, [true; 10], "kinds never seen: {KINDS:?}");
    }

    /// The smallest world is nine terranes on three plates: it still has
    /// land and sea, and a boundary between plates.
    #[test]
    fn the_smallest_world_has_land_sea_and_a_plate_boundary() {
        for seed in 0..SEEDS {
            let t = Tectonics::new(seed, square(MIN_WORLD_SIZE_BLOCKS));
            let land = t
                .terranes()
                .iter()
                .filter(|t| t.crust == Crust::Continental)
                .count();
            assert_eq!(land, 4, "seed {seed}");
            assert!(t.terranes().iter().any(|t| t.craton));
            assert!(
                adjacent(&t)
                    .iter()
                    .any(|&(a, b)| kind_index(t.boundary(a, b)) >= 4)
            );
        }
    }

    /// The framework is a function of where a column is on the torus, not
    /// of which lap it is on — to the bit.
    #[test]
    fn the_framework_is_periodic() {
        for (seed, shape) in [
            (3, WorldShape::DEFAULT),
            (4, square(MIN_WORLD_SIZE_BLOCKS)),
            (5, WorldShape::new(MIN_WORLD_SIZE_BLOCKS, 98_304).unwrap()),
        ] {
            let t = Tectonics::new(seed, shape);
            let (sx, sz) = (shape.size_x(), shape.size_z());
            for k in 0..400i64 {
                let (x, z) = (k * 7_919 % sx, k * 104_729 % sz);
                for (lx, lz) in [(1, 0), (0, 1), (-1, -1), (3, -2), (-40, 17)] {
                    let (fx, fz) = (x + lx * sx, z + lz * sz);
                    assert_eq!(t.site(fx, fz), t.site(x, z));
                    assert_eq!(t.belt(fx, fz).to_bits(), t.belt(x, z).to_bits());
                    assert_eq!(
                        t.continentalness(fx, fz).to_bits(),
                        t.continentalness(x, z).to_bits()
                    );
                }
            }
        }
    }

    /// Walk lines across whole worlds, seams included. A terrane changes
    /// only where the distance to its edge has run down to nothing; the
    /// belt envelope is full on a boundary, nothing beyond its half-width
    /// and never jumps; continentalness has its crust's sign and never
    /// jumps either.
    #[test]
    fn the_fields_are_continuous_across_every_boundary() {
        const STEP: i64 = 8;
        for (seed, shape) in [
            (0, WorldShape::DEFAULT),
            (1, WorldShape::DEFAULT),
            (2, square(MIN_WORLD_SIZE_BLOCKS)),
            (3, WorldShape::new(MIN_WORLD_SIZE_BLOCKS, 98_304).unwrap()),
        ] {
            let t = Tectonics::new(seed, shape);
            let (sx, sz) = (shape.size_x(), shape.size_z());
            // Three lines: along x, along z, and a diagonal that laps the
            // world in both.
            for (dx, dz, z0) in [(STEP, 0, sz / 3), (0, STEP, sx / 5), (STEP, STEP, 11)] {
                let count = (sx.max(sz) + 4_096) / STEP;
                let at = |k: i64| {
                    let (x, z) = if dx == 0 {
                        (z0, k * dz)
                    } else {
                        (k * dx, z0 + k * dz)
                    };
                    (x - 2_048, z - 2_048)
                };
                let sample = |k: i64| {
                    let (x, z) = at(k);
                    (t.site(x, z), t.belt(x, z), t.continentalness(x, z))
                };
                let mut last = sample(0);
                for k in 1..count {
                    let now = sample(k);
                    let (site, belt, inland) = now;
                    let where_ = format!("seed {seed} at {:?}", at(k));
                    assert!(
                        site.edge >= 0.0 && site.terrane != site.neighbour,
                        "{where_}"
                    );
                    if site.terrane != last.0.terrane {
                        assert!(
                            site.edge < 2.0 * STEP as f64 && last.0.edge < 2.0 * STEP as f64,
                            "{where_}: the terrane changed {} from its edge",
                            site.edge.max(last.0.edge)
                        );
                    }
                    assert!((0.0..=1.0).contains(&belt), "{where_}: belt {belt}");
                    if site.edge >= BELT_HALF_WIDTH {
                        assert_eq!(belt, 0.0, "{where_}: a belt inside a terrane");
                    }
                    if site.edge < 2.0 {
                        let strength = site.boundary.belt_strength();
                        assert!(
                            belt >= 0.999 * strength,
                            "{where_}: {belt} on a {strength} line"
                        );
                    }
                    assert!(
                        (belt - last.1).abs() < 0.012,
                        "{where_}: the belt jumps {} to {belt}",
                        last.1
                    );
                    let land = t.terranes()[site.terrane as usize].crust == Crust::Continental;
                    assert!(
                        if land { inland >= 0.0 } else { inland <= 0.0 },
                        "{where_}: continentalness {inland} on {:?}",
                        t.terranes()[site.terrane as usize].crust
                    );
                    assert!(
                        (inland - last.2).abs() < 2.0 * STEP as f64,
                        "{where_}: continentalness jumps {} to {inland}",
                        last.2
                    );
                    assert!(inland.abs() <= COAST_REACH);
                    last = now;
                }
            }
        }
    }

    /// Reading only the inner nine cells where that is provably enough
    /// gives the same fields, to the bit, as reading all twenty-five —
    /// and is taken for most columns, or it would not be worth having.
    #[test]
    fn the_short_search_changes_nothing() {
        for (seed, shape) in [
            (9, WorldShape::DEFAULT),
            (10, square(MIN_WORLD_SIZE_BLOCKS)),
            (11, WorldShape::new(MIN_WORLD_SIZE_BLOCKS, 57_344).unwrap()),
        ] {
            let t = Tectonics::new(seed, shape);
            let (sx, sz) = (shape.size_x(), shape.size_z());
            let mut short = 0;
            const N: i64 = 60_000;
            for k in 0..N {
                let (x, z) = (k * 7_919 % sx, k * 104_729 % sz);
                let (some, all) = (t.window(x, z, false), t.window(x, z, true));
                short += i64::from(some.d2.iter().any(|d| d.is_infinite()));
                assert_eq!(some.first, all.first);
                assert_eq!(t.inland(&some).to_bits(), t.inland(&all).to_bits());
                assert_eq!(t.envelope(&some).to_bits(), t.envelope(&all).to_bits());
            }
            assert!(
                short * 2 > N,
                "{shape:?}: the short search ran for {short} of {N}"
            );
        }
    }

    #[test]
    fn one_search_gives_the_same_fields_as_two() {
        let t = world(9);
        for k in 0..20_000i64 {
            let (x, z) = (k * 7_919 % 204_800, k * 104_729 % 204_800);
            let column = t.column(x + 204_800, z - 409_600);
            assert_eq!(column.belt().to_bits(), t.belt(x, z).to_bits());
            assert_eq!(
                column.continentalness().to_bits(),
                t.continentalness(x, z).to_bits()
            );
        }
    }

    /// One colour per plate, told apart at a glance.
    const PLATE_HUES: [[f32; 3]; 12] = [
        [86.0, 130.0, 184.0],
        [120.0, 168.0, 96.0],
        [190.0, 132.0, 96.0],
        [150.0, 116.0, 180.0],
        [96.0, 170.0, 168.0],
        [196.0, 170.0, 92.0],
        [176.0, 104.0, 130.0],
        [110.0, 140.0, 110.0],
        [140.0, 150.0, 196.0],
        [184.0, 150.0, 140.0],
        [104.0, 120.0, 150.0],
        [164.0, 184.0, 120.0],
    ];

    fn boundary_colour(b: Boundary) -> Option<[u8; 3]> {
        Some(match b {
            Boundary::Quiet => return None,
            Boundary::PassiveMargin => [236, 236, 214],
            Boundary::Suture(age) => match age {
                Age::Eoan | Age::Ferrian => [92, 74, 60],
                Age::Thalassian => [128, 92, 60],
                Age::Rhexian => [150, 104, 60],
                Age::Orean | Age::Nivean => [176, 112, 52],
            },
            Boundary::Seam(_) => [170, 160, 150],
            Boundary::Convergent { kind, .. } => match kind {
                Convergence::Collision => [208, 28, 28],
                Convergence::ContinentalArc => [240, 120, 20],
                Convergence::IslandArc => [248, 208, 40],
            },
            Boundary::Ridge => [40, 220, 232],
            Boundary::Rift => [226, 60, 214],
            Boundary::Transform => [20, 20, 20],
        })
    }

    fn line(rgb: &mut [u8], px: u32, from: (f64, f64), to: (f64, f64), colour: [u8; 3]) {
        let steps = ((to.0 - from.0).abs().max((to.1 - from.1).abs()) as i64).max(1);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let i = (from.0 + (to.0 - from.0) * t).rem_euclid(f64::from(px)) as u32;
            let j = (from.1 + (to.1 - from.1) * t).rem_euclid(f64::from(px)) as u32;
            let at = ((j * px + i) * 3) as usize;
            rgb[at..at + 3].copy_from_slice(&colour);
        }
    }

    fn dot(rgb: &mut [u8], px: u32, at: (f64, f64), radius: i64, colour: [u8; 3]) {
        for dj in -radius..=radius {
            for di in -radius..=radius {
                if di * di + dj * dj <= radius * radius {
                    let i = (at.0 as i64 + di).rem_euclid(i64::from(px)) as u32;
                    let j = (at.1 as i64 + dj).rem_euclid(i64::from(px)) as u32;
                    let o = ((j * px + i) * 3) as usize;
                    rgb[o..o + 3].copy_from_slice(&colour);
                }
            }
        }
    }

    /// Tool: `cargo test -p vox-worldgen --release tectonics_map -- --ignored`
    /// (the `WORLD_MAP_*` variables as for `world_map`). Three maps of one
    /// world, side by side:
    ///
    /// 1. **The framework.** Each plate its own colour — pale where it
    ///    carries continent, palest on cratons, dark where it is ocean
    ///    floor. Boundaries by kind: red collision, orange continental
    ///    arc, yellow island arc, cyan ridge, magenta rift, black
    ///    transform, cream passive margin, brown sutures (darker with
    ///    age). A white line from each plate's centre shows its drift; a
    ///    white dot with a tail is a hotspot and its track.
    /// 2. **What task 2 will read**: continental crust tan and ocean blue
    ///    by `continentalness`, with the belt envelope in white over it.
    /// 3. **Today's terrain**, which the framework does not yet place.
    #[test]
    #[ignore]
    fn tectonics_map() {
        let (g, px, dir) = crate::map::tests::map_world();
        let shape = g.shape();
        let t = g.tectonics();
        let step = shape.size_x() as f64 / f64::from(px);
        let to_px = |x: f64, z: f64| (x / step, z / step);
        let mut panels = [(); 3].map(|()| vec![0u8; (px * px * 3) as usize]);
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let at = ((j * px + i) * 3) as usize;
                let site = t.site(x, z);
                let terrane = t.terranes()[site.terrane as usize];
                let hue = PLATE_HUES[terrane.plate as usize % PLATE_HUES.len()];
                let shade = match (terrane.crust, terrane.craton) {
                    (Crust::Oceanic, _) => hue.map(|c| c * 0.42),
                    (Crust::Continental, false) => hue.map(|c| c + (255.0 - c) * 0.35),
                    (Crust::Continental, true) => hue.map(|c| c + (255.0 - c) * 0.7),
                };
                let mut colour = shade.map(|c| c as u8);
                let width = match site.boundary {
                    Boundary::Seam(_) => 0.5,
                    Boundary::Suture(_) | Boundary::PassiveMargin => 0.9,
                    _ => 1.8,
                };
                if site.edge < width * step
                    && let Some(c) = boundary_colour(site.boundary)
                {
                    colour = c;
                }
                panels[0][at..at + 3].copy_from_slice(&colour);

                let inland = t.continentalness(x, z) as f32;
                let base = if inland >= 0.0 {
                    crate::map::ramp(
                        &[(0.0, [196, 178, 140]), (20_000.0, [150, 132, 100])],
                        inland,
                    )
                } else {
                    crate::map::ramp(&[(0.0, [70, 120, 170]), (20_000.0, [20, 40, 84])], -inland)
                };
                let belt = t.belt(x, z) as f32;
                let colour = base.map(|c| (f32::from(c) + (255.0 - f32::from(c)) * belt) as u8);
                panels[1][at..at + 3].copy_from_slice(&colour);

                panels[2][at..at + 3]
                    .copy_from_slice(&elevation_colour(g.elevation().height(x, z)));
            }
        }
        for plate in t.plates() {
            let (x, z) = t.seed_point(plate.centre);
            let from = to_px(x, z);
            let to = (from.0 + plate.drift.0 * 34.0, from.1 + plate.drift.1 * 34.0);
            line(&mut panels[0], px, from, to, [255, 255, 255]);
            dot(&mut panels[0], px, from, 3, [255, 255, 255]);
        }
        for spot in t.hotspots() {
            let from = to_px(spot.x as f64, spot.z as f64);
            let drift = t.plates()[spot.plate as usize].drift;
            let to = (from.0 + drift.0 * 26.0, from.1 + drift.1 * 26.0);
            line(&mut panels[0], px, from, to, [255, 255, 255]);
            dot(&mut panels[0], px, from, 4, [0, 0, 0]);
            dot(&mut panels[0], px, from, 2, [255, 255, 255]);
        }
        let gap = 6u32;
        let width = px * 3 + gap * 2;
        let mut rgb = vec![255u8; (width * px * 3) as usize];
        for (k, panel) in panels.iter().enumerate() {
            let left = k as u32 * (px + gap);
            for j in 0..px {
                let from = (j * px * 3) as usize;
                let to = ((j * width + left) * 3) as usize;
                rgb[to..to + (px * 3) as usize]
                    .copy_from_slice(&panel[from..from + (px * 3) as usize]);
            }
        }
        let path = dir.join(format!("tectonics-{:x}-{}.png", g.seed(), shape.size_x()));
        write_png(&path, width, px, &rgb).unwrap();
        println!("wrote {}  {:?}", path.display(), t);
    }

    fn kind_index(b: Boundary) -> usize {
        match b {
            Boundary::Quiet => 0,
            Boundary::PassiveMargin => 1,
            Boundary::Suture(_) => 2,
            Boundary::Seam(_) => 3,
            Boundary::Convergent { kind, .. } => match kind {
                Convergence::Collision => 4,
                Convergence::ContinentalArc => 5,
                Convergence::IslandArc => 6,
            },
            Boundary::Ridge => 7,
            Boundary::Rift => 8,
            Boundary::Transform => 9,
        }
    }
    const KINDS: [&str; 10] = [
        "quiet",
        "passive",
        "suture",
        "seam",
        "collision",
        "cont.arc",
        "isl.arc",
        "ridge",
        "rift",
        "transform",
    ];

    /// Tool: `cargo test -p vox-worldgen --release tectonics_survey --
    /// --ignored --nocapture` (`SURVEY_SEEDS`, `WORLD_MAP_SIZE`). What the
    /// framework holds, world by world: plates, the share of continental
    /// crust, each kind of boundary as a share of all boundary length, and
    /// the share of continental ground under the belt envelope.
    #[test]
    #[ignore]
    fn tectonics_survey() {
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<i64>().ok());
        let seeds = env("SURVEY_SEEDS").unwrap_or(12) as u64;
        let size = env("WORLD_MAP_SIZE").unwrap_or(WorldShape::DEFAULT.size_x());
        let shape = WorldShape::new(size, size).unwrap();
        const GRID: i64 = 500;
        println!(
            "seed plates conts crust% | {} | belt>.25 >.5 >.75 (% of continental ground)",
            KINDS[1..].join(" ")
        );
        for seed in 0..seeds {
            let t = Tectonics::new(seed, shape);
            let mut length = [0u32; 10];
            let (mut crust, mut belts) = (0u32, [0u32; 3]);
            for j in 0..GRID {
                for i in 0..GRID {
                    let (x, z) = (i * size / GRID, j * size / GRID);
                    let site = t.site(x, z);
                    if site.edge < 400.0 {
                        length[kind_index(site.boundary)] += 1;
                    }
                    if t.terranes()[site.terrane as usize].crust == Crust::Continental {
                        crust += 1;
                        let belt = t.belt(x, z);
                        for (k, bar) in [0.25, 0.5, 0.75].iter().enumerate() {
                            belts[k] += u32::from(belt > *bar);
                        }
                    }
                }
            }
            let total: u32 = length[1..].iter().sum();
            let continents = t
                .terranes()
                .iter()
                .filter_map(|t| t.continent)
                .max()
                .map_or(0, |c| c + 1);
            let shares: Vec<String> = length[1..]
                .iter()
                .map(|&n| format!("{:4.1}", 100.0 * f64::from(n) / f64::from(total.max(1))))
                .collect();
            println!(
                "{seed:4} {:6} {continents:5} {:5.1} | {} | {:5.1} {:5.1} {:5.1}",
                t.plates().len(),
                100.0 * f64::from(crust) / (GRID * GRID) as f64,
                shares.join(" "),
                100.0 * f64::from(belts[0]) / f64::from(crust.max(1)),
                100.0 * f64::from(belts[1]) / f64::from(crust.max(1)),
                100.0 * f64::from(belts[2]) / f64::from(crust.max(1)),
            );
        }
    }

    /// Tool: the largest step in the belt envelope between columns eight
    /// blocks apart, over many lines in many worlds — the continuity test
    /// at a size too slow to run every time.
    #[test]
    #[ignore]
    fn belt_jump_scan() {
        let mut worst = (0.0f64, 0u64, 0i64, 0i64);
        for seed in 0..24u64 {
            let t = world(seed);
            for line in 0..40i64 {
                let z = line * 5_119 + 7;
                let mut last = t.belt(0, z);
                for x in (8..204_800).step_by(8) {
                    let belt = t.belt(x, z);
                    if (belt - last).abs() > worst.0 {
                        worst = ((belt - last).abs(), seed, x, z);
                    }
                    last = belt;
                }
            }
        }
        println!(
            "largest step {:.4} in seed {} at ({}, {})",
            worst.0, worst.1, worst.2, worst.3
        );
    }

    /// Tool: what the framework costs per column, beside the terrain's
    /// height (`cargo test -p vox-worldgen --release tectonics_cost --
    /// --ignored --nocapture`).
    #[test]
    #[ignore]
    fn tectonics_cost() {
        let g = crate::Generator::new(0x7e22ac0de, WorldShape::DEFAULT);
        let t = g.tectonics();
        const N: i64 = 400_000;
        let columns = |k: i64| (k * 7_919 % 204_800, k * 104_729 % 204_800);
        let time = |name: &str, f: &dyn Fn(i64, i64) -> f64| {
            let mut best = f64::MAX;
            let mut sum = 0.0;
            for _ in 0..7 {
                let start = std::time::Instant::now();
                sum = 0.0;
                for k in 0..N {
                    let (x, z) = columns(k);
                    sum += f(x, z);
                }
                best = best.min(start.elapsed().as_nanos() as f64 / N as f64);
            }
            println!("{name:16} {best:7.0} ns/column   (checksum {sum:.3})");
        };
        time("warp only", &|x, z| t.warped(x, z).0);
        time("window only", &|x, z| t.window(x, z, false).d2[12]);
        time("window, whole", &|x, z| t.window(x, z, true).d2[12]);
        time("column, both", &|x, z| {
            let column = t.column_canonical(x, z);
            column.continentalness() + column.belt()
        });
        time("column, as used", &|x, z| {
            let column = t.column_canonical(x, z);
            let inland = column.continentalness();
            if inland > -1_500.0 {
                inland + column.belt()
            } else {
                inland
            }
        });
        time("belt", &|x, z| t.belt(x, z));
        time("continentalness", &|x, z| t.continentalness(x, z));
        time("site", &|x, z| t.site(x, z).edge);
        time("terrain height", &|x, z| g.elevation().height(x, z) as f64);
    }
}
