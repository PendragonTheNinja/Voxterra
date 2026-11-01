//! The hollows of the land and the water they hold.
//!
//! The hollows nest — a pit inside a basin inside a broad shallow plain — so
//! they are found as a merge tree (a depression hierarchy; Barnes, Callaghan
//! & Wickert, 2020): cells taken lowest first, each joining the groups of
//! the neighbours already taken. Where two undrained groups meet, both are
//! full to that saddle and fill on together as one hollow, their parent; a
//! group that meets the sea, or anything that drains to it, spills there.
//!
//! Water is then poured in, in the steady state of their Fill-Spill-Merge:
//! each cell's runoff runs downhill to the hollow it ends in; a hollow whose
//! inflow beats the evaporation from a lake filling it to the brim is full,
//! and the rest spills over the saddle — into its sibling, or, when the
//! sibling is full too, into the pool they share. A hollow that cannot fill
//! is a **closed basin**: its lake settles where evaporation from its
//! surface equals what flows in — a salt lake, or a salt flat when almost
//! nothing does (Great Salt Lake, Lop Nur, Salar de Uyuni).
//!
//! Last, from the outermost hollow in: a full hollow kept by the lake rules
//! (`LAKE_MIN_MEAN_DEPTH`, the glaciated tarns) holds a fresh lake to its
//! spill level, everything inside it included; a full one that is not is
//! breached, as rivers breach them over geological time, and each hollow
//! inside it is judged on its own. Closed basins are never breached: no
//! river leaves them to cut the rim.

use super::{
    LAKE_MAX_CELLS_GLACIATED, LAKE_MIN_CELLS, LAKE_MIN_CELLS_GLACIATED, LAKE_MIN_DEPTH_GLACIATED,
    LAKE_MIN_MEAN_DEPTH, neighbours,
};

const NONE: u32 = u32::MAX;

/// The aridity index (precipitation over potential evaporation) below which
/// a hollow that cannot fill stays a closed basin. Earth's endorheic basins
/// lie almost all in arid and semi-arid land — UNEP's index under 0.5;
/// wetter hollows were cut through long ago, in the wetter climates of the
/// past and over geological time, so here they are breached as if full.
const ARID_INDEX: f64 = 0.5;

/// A closed basin's floor this far (blocks) above its lake is salt flat: the
/// playa its lake floods and leaves.
const SALT_FLAT_RISE: i32 = 3;

/// What covers a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    /// Dry land (rivers run on land).
    Land,
    /// The sea.
    Sea,
    /// A fresh lake, with an outlet.
    Lake,
    /// The lake of a closed basin: no outlet, salt.
    SaltLake,
    /// The dry floor of a closed basin, crusted with salt.
    SaltFlat,
}

/// The water held in the hollows.
pub(super) struct Held {
    /// The water surface where a lake covers the cell, else the ground.
    pub surface: Vec<i32>,
    pub cover: Vec<Cover>,
    /// Cells where water ends without reaching the sea: closed basins.
    pub sink: Vec<bool>,
}

struct Hollow {
    /// The level where it spills or joins another; `i32::MAX` if it never
    /// does (only in a world without sea).
    spill: i32,
    parent: u32,
    children: [u32; 2],
    /// A cell inside it, beside the saddle where it joined its sibling:
    /// where its sibling's overflow comes in.
    entry: u32,
    /// For a hollow that spills onto ground that drains: the cell beyond
    /// its saddle its overflow runs on from.
    out: u32,
    /// Over everything inside it, at its spill level.
    cells: i64,
    sum_height: i64,
    deepest: i32,
    bottom: u32,
    cold: i64,
    /// Precipitation and potential evaporation summed over its cells, mm.
    precip: f64,
    pet: f64,
}

/// Per cell of the drainage grid, what the hollows are poured from.
pub(super) struct Ground<'a> {
    pub height: &'a [i32],
    pub sea: &'a [bool],
    /// Ice-scoured: below `GLACIATED_BELOW_C`.
    pub cold: &'a [bool],
    /// Precipitation and potential evaporation, mm/yr.
    pub climate: &'a [(f32, f32)],
    /// The water the cell sheds, m³/s.
    pub runoff: &'a [f64],
    /// What a lake covering the cell would cost: its evaporation less the
    /// runoff the cell no longer sheds, m³/s, never negative.
    pub demand: &'a [f64],
}

/// Find the hollows, pour in the runoff, and decide what each holds.
pub(super) fn settle(nx: usize, nz: usize, ground: &Ground) -> Held {
    let Ground {
        height,
        sea,
        cold,
        climate,
        runoff,
        demand,
    } = *ground;
    let n = nx * nz;
    let mut land: Vec<u32> = (0..n as u32).filter(|&c| !sea[c as usize]).collect();
    land.sort_unstable_by_key(|&c| (height[c as usize], c));
    let (hollows, joined) = merge_tree(nx, nz, height, sea, cold, climate, &land);
    let m = hollows.len();

    // Each hollow's own cells — those that joined it directly, between its
    // children's spill and its own — lowest first, as they were taken.
    let mut own_start = vec![0u32; m + 1];
    for &c in &land {
        let x = joined[c as usize];
        if x != NONE {
            own_start[x as usize + 1] += 1;
        }
    }
    for x in 0..m {
        own_start[x + 1] += own_start[x];
    }
    let mut own = vec![0u32; own_start[m] as usize];
    let mut fill = own_start.clone();
    for &c in &land {
        let x = joined[c as usize];
        if x != NONE {
            own[fill[x as usize] as usize] = c;
            fill[x as usize] += 1;
        }
    }
    let own_of = |x: usize| &own[own_start[x] as usize..own_start[x + 1] as usize];
    let need: Vec<f64> = (0..m)
        .map(|x| own_of(x).iter().map(|&c| demand[c as usize]).sum())
        .collect();

    // Runoff runs down the steepest slope to the hollow it ends in; a cell
    // with no lower neighbour is in the hollow it joined.
    let mut term = vec![NONE; n];
    for &c in &land {
        let c = c as usize;
        let mut low = c;
        for nb in neighbours(c, nx, nz) {
            if (height[nb], nb) < (height[low], low) {
                low = nb;
            }
        }
        term[c] = if low == c {
            joined[c]
        } else if sea[low] {
            NONE
        } else {
            term[low]
        };
    }
    let mut inflow = vec![0.0f64; m];
    for &c in &land {
        let x = term[c as usize];
        if x != NONE {
            inflow[x as usize] += runoff[c as usize];
        }
    }

    // A hollow on ground that is not arid never closes: if it cannot pay
    // its lake's evaporation, it is breached and its water passes on.
    let breached: Vec<bool> = hollows
        .iter()
        .map(|h| h.precip >= ARID_INDEX * h.pet)
        .collect();
    // Pour, from the innermost hollows out (children are made before
    // their parents).
    let mut pour = Pour {
        hollows: &hollows,
        need: &need,
        breached: &breached,
        term: &term,
        water: vec![0.0; m],
        full: vec![false; m],
        lake: vec![false; m],
        pool: vec![false; m],
        excess: vec![0.0; m],
    };
    for x in 0..m {
        let [a, b] = hollows[x].children;
        if a == NONE {
            pour.pool[x] = true;
            pour.water[x] = inflow[x];
            pour.fill(x);
            continue;
        }
        let (a, b) = (a as usize, b as usize);
        let mut pending = inflow[x];
        if !pour.full[a] && !pour.full[b] && pending > 0.0 {
            pour.deliver(pending, hollows[a].entry, a);
            pending = 0.0;
        }
        if pour.full[a] && !pour.full[b] {
            let w = std::mem::take(&mut pour.excess[a]) + std::mem::take(&mut pending);
            pour.deliver(w, hollows[b].entry, b);
        }
        if pour.full[b] && !pour.full[a] {
            let w = std::mem::take(&mut pour.excess[b]) + std::mem::take(&mut pending);
            pour.deliver(w, hollows[a].entry, a);
        }
        if pour.full[a] && pour.full[b] {
            pour.pool[x] = true;
            pour.water[x] =
                std::mem::take(&mut pour.excess[a]) + std::mem::take(&mut pour.excess[b]) + pending;
            pour.fill(x);
        }
    }

    // What overflows a whole tree runs on downhill from its saddle — to the
    // sea, or into another, lower hollow, which may fill in turn. Highest
    // spill first, so a hollow has all it will receive before it passes
    // its own overflow on.
    let root = |mut x: usize| {
        while hollows[x].parent != NONE {
            x = hollows[x].parent as usize;
        }
        x
    };
    let mut queue: std::collections::BinaryHeap<(i32, std::cmp::Reverse<usize>)> = (0..m)
        .filter(|&x| hollows[x].parent == NONE && hollows[x].spill != i32::MAX)
        .map(|x| (hollows[x].spill, std::cmp::Reverse(x)))
        .collect();
    let mut passes = vec![0u8; m];
    while let Some((_, std::cmp::Reverse(x))) = queue.pop() {
        let out = hollows[x].out;
        if !pour.full[x] || pour.excess[x] <= 0.0 || out == NONE {
            continue;
        }
        let t = term[out as usize];
        if t == NONE {
            continue; // to the sea
        }
        let target = root(t as usize);
        // Downhill only; at an equal spill, a pair could pass water back
        // and forth — let it go to the sea instead after a few rounds.
        passes[x] += 1;
        if target == x || hollows[target].spill > hollows[x].spill || passes[x] > 4 {
            continue;
        }
        let w = std::mem::take(&mut pour.excess[x]);
        pour.deliver(w, out, target);
        if pour.full[target] && pour.excess[target] > 0.0 {
            queue.push((hollows[target].spill, std::cmp::Reverse(target)));
        }
    }

    // Decide, from the outermost in: (level, salt) per hollow.
    let mut held: Vec<Option<(i32, bool)>> = vec![None; m];
    for x in (0..m).rev() {
        let h = &hollows[x];
        if h.parent != NONE
            && let Some(p) = held[h.parent as usize]
        {
            held[x] = Some(p);
            continue;
        }
        if h.spill == i32::MAX {
            continue;
        }
        if pour.lake[x] {
            let volume = h.cells * i64::from(h.spill) - h.sum_height;
            let basin = volume >= LAKE_MIN_MEAN_DEPTH * h.cells && h.cells >= LAKE_MIN_CELLS as i64;
            let glaciated = h.cold * 2 >= h.cells;
            let tarn = glaciated
                && h.spill - h.deepest >= LAKE_MIN_DEPTH_GLACIATED
                && (LAKE_MIN_CELLS_GLACIATED as i64..=LAKE_MAX_CELLS_GLACIATED as i64)
                    .contains(&h.cells);
            if basin || tarn {
                held[x] = Some((h.spill, false));
            }
        } else if pour.pool[x] && !pour.full[x] {
            // The level where evaporation from the lake meets the inflow:
            // the lake covers the lowest of its own cells it can pay for.
            let mut left = pour.water[x];
            let mut level = h.spill;
            for &c in own_of(x) {
                left -= demand[c as usize];
                if left < 0.0 {
                    level = height[c as usize];
                    break;
                }
            }
            held[x] = Some((level, true));
        }
    }

    let mut surface = height.to_vec();
    let mut cover: Vec<Cover> = sea
        .iter()
        .map(|&s| if s { Cover::Sea } else { Cover::Land })
        .collect();
    let mut sink = vec![false; n];
    for &c in &land {
        let c = c as usize;
        let x = joined[c];
        let Some((level, salt)) = (if x == NONE { None } else { held[x as usize] }) else {
            continue;
        };
        if level > height[c] {
            surface[c] = level;
            cover[c] = if salt { Cover::SaltLake } else { Cover::Lake };
            sink[c] = salt;
        } else if salt && height[c] < level + SALT_FLAT_RISE {
            cover[c] = Cover::SaltFlat;
        }
    }
    // Every closed basin ends somewhere, even one too dry to hold a lake.
    for (x, h) in hollows.iter().enumerate() {
        if let Some((_, true)) = held[x] {
            let b = h.bottom as usize;
            sink[b] = true;
            if cover[b] == Cover::Land {
                cover[b] = Cover::SaltFlat;
            }
        }
    }
    Held {
        surface,
        cover,
        sink,
    }
}

/// The merge tree: the hollows, and for each land cell the hollow it joined
/// (`NONE` for ground that drains).
fn merge_tree(
    nx: usize,
    nz: usize,
    height: &[i32],
    sea: &[bool],
    cold: &[bool],
    climate: &[(f32, f32)],
    land: &[u32],
) -> (Vec<Hollow>, Vec<u32>) {
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
    let mut joined = vec![NONE; n];
    let mut hollows: Vec<Hollow> = Vec::new();
    let mut taken = vec![false; n];
    for c in 0..n {
        if sea[c] {
            up[c] = n as u32;
            taken[c] = true;
        }
    }
    for &c in land {
        let c = c as usize;
        let h = height[c];
        taken[c] = true;
        // This cell's group root once it has one, and the neighbour it
        // joined through (a cell of that group beside this saddle).
        let (mut mine, mut via) = (NONE, NONE);
        for nb in neighbours(c, nx, nz) {
            if !taken[nb] || nb == c {
                continue;
            }
            let theirs = root(&mut up, nb as u32);
            if mine == NONE {
                up[c] = theirs;
                (mine, via) = (theirs, nb as u32);
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
                // Either drains: both now drain, the undrained one over
                // this saddle onto the other's ground.
                if a != NONE {
                    hollows[a as usize].out = nb as u32;
                } else if b != NONE {
                    hollows[b as usize].out = via;
                }
                NONE
            } else {
                let id = hollows.len() as u32;
                hollows[a as usize].entry = via;
                hollows[b as usize].entry = nb as u32;
                let (ha, hb) = (&hollows[a as usize], &hollows[b as usize]);
                let parent = Hollow {
                    spill: i32::MAX,
                    parent: NONE,
                    children: [a, b],
                    entry: NONE,
                    out: NONE,
                    cells: ha.cells + hb.cells,
                    sum_height: ha.sum_height + hb.sum_height,
                    deepest: ha.deepest.min(hb.deepest),
                    bottom: if ha.deepest <= hb.deepest {
                        ha.bottom
                    } else {
                        hb.bottom
                    },
                    cold: ha.cold + hb.cold,
                    precip: ha.precip + hb.precip,
                    pet: ha.pet + hb.pet,
                };
                hollows[a as usize].parent = id;
                hollows[b as usize].parent = id;
                hollows.push(parent);
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
                parent: NONE,
                children: [NONE; 2],
                entry: NONE,
                out: NONE,
                cells: 0,
                sum_height: 0,
                deepest: h,
                bottom: c as u32,
                cold: 0,
                precip: 0.0,
                pet: 0.0,
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
            hl.precip += f64::from(climate[c].0);
            hl.pet += f64::from(climate[c].1);
        }
    }
    (hollows, joined)
}

/// The state of the pour.
struct Pour<'a> {
    hollows: &'a [Hollow],
    need: &'a [f64],
    breached: &'a [bool],
    term: &'a [u32],
    /// Water reaching each hollow's pool, m³/s.
    water: Vec<f64>,
    /// Whether water leaves it: it filled, or it is breached.
    full: Vec<bool>,
    /// Whether it filled to its spill with a lake.
    lake: Vec<bool>,
    /// Whether the hollow holds a pool of its own: always for the
    /// innermost, and for a parent once both its children are full.
    pool: Vec<bool>,
    /// Overflow of a full hollow not yet passed to its parent.
    excess: Vec<f64>,
}

impl Pour<'_> {
    /// Settle a pool's state from the water reaching it: full with a lake
    /// if it pays its evaporation, else open and dry if breached, else
    /// closed.
    fn fill(&mut self, x: usize) {
        if self.water[x] >= self.need[x] {
            self.full[x] = true;
            self.lake[x] = true;
            self.excess[x] = self.water[x] - self.need[x];
        } else if self.breached[x] {
            self.full[x] = true;
            self.excess[x] = self.water[x];
        }
    }

    /// Pour `w` in at `cell`, inside hollow `stop` (already poured): it runs
    /// to the hollow it ends in, fills it, and what spills moves on — to a
    /// sibling, or up to a shared pool — but never out of `stop`, whose
    /// parent is still being poured.
    fn deliver(&mut self, mut w: f64, cell: u32, stop: usize) {
        let mut x = self.term[cell as usize] as usize;
        loop {
            if !self.pool[x] {
                // A parent whose children are not both full has no pool:
                // the water runs down into a child.
                let a = self.hollows[x].children[0] as usize;
                x = self.term[self.hollows[a].entry as usize] as usize;
                continue;
            }
            if !self.full[x] {
                self.water[x] += w;
                self.fill(x);
                if !self.full[x] {
                    return;
                }
                w = std::mem::take(&mut self.excess[x]);
            }
            if x == stop {
                self.excess[x] += w;
                return;
            }
            let p = self.hollows[x].parent as usize;
            let [a, b] = self.hollows[p].children;
            let sibling = if a as usize == x { b } else { a } as usize;
            if self.full[sibling] {
                // Both full: they pool together in the parent.
                self.pool[p] = true;
                x = p;
            } else {
                x = self.term[self.hollows[sibling].entry as usize] as usize;
            }
        }
    }
}
