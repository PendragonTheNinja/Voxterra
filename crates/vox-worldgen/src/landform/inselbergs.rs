//! Inselbergs and tors (M15 task 4, `docs/landforms.md` *Weathering*,
//! ADR-0021).
//!
//! Where tropical rock weathered deep for millions of years and the plain
//! was then stripped, the unweathered cores stand out of the plain:
//! **bornhardts**, domes of bare rock (Uluru, the Sugarloaf, Spitzkoppe),
//! here 40–150 blocks high and 200–600 across, one every few kilometres;
//! and **tors** (kopjes), piles of stacked blocks 6–20 high and 20–60
//! across, more often — also, at low weight, on temperate uplands
//! (Dartmoor; owner's decision 4).
//!
//! Each kind stands on a jittered lattice tiled with the world: one site
//! per cell, kept with a probability set by the process weight *at the
//! site's centre*, so a dome is always whole — never cut off by a weight
//! boundary across it. A dome is steep at its foot and rounded on top
//! (`h = H·√(1 − (r/R)³)`); about half are rounder (`1 − (r/R)²`), with a
//! flank to walk up (decision 3). A tor is the same shape stepped in
//! 3-block courses, its blocks. Bare rock: no soil. Height layers that
//! only raise, before the drainage network (rivers flow round them).

use vox_core::WorldShape;

use crate::elevation::mix64;

/// One kind of rock: lattice cell, radius and height ranges (blocks), how
/// many cells keep a site at full weight, and the course its stacks step in
/// (0: smooth).
struct Kind {
    cell: i64,
    radius: (f64, f64),
    height: (f32, f32),
    density: f32,
    course: f32,
    salt: u64,
}

const DOME: Kind = Kind {
    cell: 2_500,
    radius: (100.0, 300.0),
    height: (40.0, 150.0),
    density: 0.7,
    course: 0.0,
    salt: 0xB0_0001,
};
const TOR: Kind = Kind {
    cell: 500,
    radius: (10.0, 30.0),
    height: (6.0, 20.0),
    density: 0.5,
    course: 3.0,
    salt: 0xB0_0002,
};

/// The short axis's length against the long's, and the outline's wobble
/// (a share of the radius).
const ASPECT: (f64, f64) = (0.55, 1.0);
const OUTLINE: f64 = 0.25;

/// Which rock a column stands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rock {
    Dome,
    Tor,
}

impl Rock {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dome => "inselberg",
            Self::Tor => "tor",
        }
    }
}

/// The layer for one world.
#[derive(Clone, Copy, Debug)]
pub struct Inselbergs {
    seed: u64,
    /// Per kind: lattice cells around each axis, and blocks per cell.
    grids: [(i64, i64, f64, f64); 2],
}

const KINDS: [(&Kind, Rock); 2] = [(&DOME, Rock::Dome), (&TOR, Rock::Tor)];

/// A site: its centre (in the column's lap), radius, height, and whether
/// it is the rounder, climbable kind.
struct Site {
    /// Its canonical cell, which its draws hash.
    cell: (u64, u64),
    x: f64,
    z: f64,
    radius: f64,
    height: f32,
    round: bool,
    /// Its long axis (a unit vector) and how much shorter the short one is.
    axis: (f64, f64),
    aspect: f64,
}

impl Inselbergs {
    pub fn new(seed: u64, shape: WorldShape) -> Self {
        let grid = |kind: &Kind| {
            let nx = i64::from(shape.cells_around_x(kind.cell));
            let nz = i64::from(shape.cells_around_z(kind.cell));
            (
                nx,
                nz,
                shape.size_x() as f64 / nx as f64,
                shape.size_z() as f64 / nz as f64,
            )
        };
        Self {
            seed,
            grids: KINDS.map(|(kind, _)| grid(kind)),
        }
    }

    /// The rock raising a column and how many blocks, if any. `weights(x,
    /// z)` gives the dome and tor weights at a site's centre; it is asked
    /// only for sites whose rock could reach the column.
    /// `wobble` is a noise in `[-1, 1]` that roughens the outlines.
    pub fn at(
        &self,
        x: i64,
        z: i64,
        wobble: f32,
        mut weights: impl FnMut(i64, i64) -> (f32, f32),
    ) -> Option<(f32, Rock)> {
        let mut best: Option<(f32, Rock)> = None;
        for (k, &(kind, rock)) in KINDS.iter().enumerate() {
            self.sites_near(k, x, z, |site| {
                let (dx, dz) = (x as f64 + 0.5 - site.x, z as f64 + 0.5 - site.z);
                // Along and across its axis, the short way stretched; the
                // outline roughened.
                let along = dx * site.axis.0 + dz * site.axis.1;
                let across = (-dx * site.axis.1 + dz * site.axis.0) / site.aspect;
                let r = (along * along + across * across).sqrt()
                    / (site.radius * (1.0 + OUTLINE * f64::from(wobble)));
                if r >= 1.0 {
                    return;
                }
                let (dome, tor) = weights(site.x.floor() as i64, site.z.floor() as i64);
                let w = if rock == Rock::Dome { dome } else { tor };
                if keep(self.seed, kind, &site) >= w * kind.density {
                    return;
                }
                let r = r as f32;
                let mut h = if site.round {
                    site.height * (1.0 - r * r)
                } else {
                    site.height * (1.0 - r * r * r).max(0.0).sqrt()
                };
                if kind.course > 0.0 {
                    h = (h / kind.course).ceil() * kind.course;
                }
                // Under a block rounds to nothing: no rock.
                if h >= 0.5 && best.is_none_or(|b| h > b.0) {
                    best = Some((h, rock));
                }
            });
        }
        best
    }

    /// The sites of the 3×3 lattice cells around a column whose rock
    /// could reach it, in its lap: each site's other draws are made only
    /// once its position and radius say it might.
    fn sites_near(&self, k: usize, x: i64, z: i64, mut each: impl FnMut(Site)) {
        let kind = KINDS[k].0;
        let (nx, nz, cw, ch) = self.grids[k];
        let (ci, cj) = (
            (x as f64 / cw).floor() as i64,
            (z as f64 / ch).floor() as i64,
        );
        let (px, pz) = (x as f64 + 0.5, z as f64 + 0.5);
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di, cj + dj);
                // The site belongs to the canonical cell; its position is
                // in this lap.
                let (li, lj) = (i.rem_euclid(nx) as u64, j.rem_euclid(nz) as u64);
                let h = |k: u64| unit(mix64(self.seed ^ kind.salt ^ (li << 32) ^ lj ^ (k << 56)));
                let radius = kind.radius.0 + (kind.radius.1 - kind.radius.0) * h(1);
                // Inside its cell, away from the edges far enough that a
                // site reaches no further than the next cell.
                let margin = (radius / cw.min(ch)).min(0.45);
                let sx = (i as f64 + margin + (1.0 - 2.0 * margin) * h(2)) * cw;
                let sz = (j as f64 + margin + (1.0 - 2.0 * margin) * h(3)) * ch;
                let reach = radius * (1.0 + OUTLINE) + 1.0;
                if (px - sx).abs() > reach || (pz - sz).abs() > reach {
                    continue;
                }
                each(Site {
                    cell: (li, lj),
                    x: sx,
                    z: sz,
                    radius,
                    height: kind.height.0 + (kind.height.1 - kind.height.0) * h(4) as f32,
                    round: h(5) < 0.5,
                    axis: {
                        let (ax, az) = (h(6) - 0.5, h(7) - 0.5);
                        let len = (ax * ax + az * az).sqrt().max(1e-6);
                        (ax / len, az / len)
                    },
                    aspect: ASPECT.0 + (ASPECT.1 - ASPECT.0) * h(8),
                });
            }
        }
    }
}

/// A site's draw against its keep probability, in `[0, 1)`.
fn keep(seed: u64, kind: &Kind, site: &Site) -> f32 {
    let (i, j) = site.cell;
    unit(mix64(seed ^ kind.salt ^ 0x6B ^ (i << 32) ^ j ^ (6 << 56))) as f32
}

/// A hash's top bits as a value in `[0, 1)`.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Domes are whole and steep at the foot; the layer tiles with the
    /// world (a site's draw is made where it is, on any lap).
    #[test]
    fn domes_are_whole_steep_and_tiled() {
        let shape = WorldShape::DEFAULT;
        let l = Inselbergs::new(7, shape);
        let (mut found, mut steep) = (0u32, 0u32);
        for i in 0..2_000i64 {
            let (x, z) = (i * 97, i * 61 + 13);
            let got = l.at(x, z, 0.0, |_, _| (1.0, 0.0));
            assert_eq!(
                got,
                l.at(x + shape.size_x(), z - shape.size_z(), 0.0, |_, _| (
                    1.0, 0.0
                ))
            );
            if let Some((h, Rock::Dome)) = got {
                found += 1;
                assert!((0.0..=150.0).contains(&h));
                // Walk out along x to the foot: the steep kind ends in a
                // drop of several blocks.
                let mut k = 1;
                let mut last = h;
                while let Some((h2, Rock::Dome)) = l.at(x + k, z, 0.0, |_, _| (1.0, 0.0)) {
                    last = h2;
                    k += 1;
                }
                steep += u32::from(last >= 5.0);
            }
        }
        assert!(found > 20, "{found}");
        assert!(steep > 0);
        // No weight, no rock.
        assert_eq!(l.at(1_000, 1_000, 0.0, |_, _| (0.0, 0.0)), None);
    }
}
