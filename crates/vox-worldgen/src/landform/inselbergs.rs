//! Inselbergs and tors (M15 task 4, `docs/landforms.md` *Weathering*,
//! ADR-0026).
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
const OUTLINE: f64 = 0.35;
/// A dome is a massif (the owner's first look at version 22: "random bare
/// big spheres-ish of rock"): its main dome and 2–4 lesser lobes leaning on
/// it (Kata Tjuta's domes, the Sugarloaf's shoulders), each this far from
/// the centre, this wide and this high (shares of the main dome's) — on
/// its shoulder, standing as a summit of its own (nearer, the main dome
/// buried it).
const LOBES: (u32, u32) = (2, 4);
const LOBE_OFFSET: (f64, f64) = (0.7, 1.1);
const LOBE_RADIUS: (f64, f64) = (0.35, 0.6);
const LOBE_HEIGHT: (f32, f32) = (0.4, 0.8);
/// Rain's grooves down its flanks: up to this share of its height deep,
/// deepest half-way down, as many around as the groove noise gives at this
/// many blocks from the centre (a noise of direction alone: grooves run
/// straight downhill).
const FLUTE: f32 = 0.15;
const FLUTE_REACH: f64 = 90.0;
/// How far beyond its radius a dome's rock may reach, as a share of it.
const DOME_REACH: f64 = 1.75;

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
    /// A dome's lesser lobes: offset from its centre (x, z), radius,
    /// height; the first `lobe_count` of them.
    lobes: [(f64, f64, f64, f32); 4],
    lobe_count: usize,
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
    /// `groove(x, z)` is a noise in `[-1, 1]` at a point, which the flutes
    /// sample along a circle about each dome's centre.
    pub fn at(
        &self,
        x: i64,
        z: i64,
        wobble: f32,
        mut weights: impl FnMut(i64, i64) -> (f32, f32),
        groove: impl Fn(f64, f64) -> f32,
    ) -> Option<(f32, Rock)> {
        let mut best: Option<(f32, Rock)> = None;
        for (k, &(kind, rock)) in KINDS.iter().enumerate() {
            self.sites_near(k, x, z, |site| {
                let (dx, dz) = (x as f64 + 0.5 - site.x, z as f64 + 0.5 - site.z);
                // Along and across its axis, the short way stretched; the
                // outline roughened.
                let scale = 1.0 + OUTLINE * f64::from(wobble);
                let local = |(dx, dz): (f64, f64)| {
                    let along = dx * site.axis.0 + dz * site.axis.1;
                    let across = (-dx * site.axis.1 + dz * site.axis.0) / site.aspect;
                    (along * along + across * across).sqrt()
                };
                let r = local((dx, dz)) / (site.radius * scale);
                let reach = if rock == Rock::Dome { DOME_REACH } else { 1.0 };
                if r >= reach {
                    return;
                }
                let (dome, tor) = weights(site.x.floor() as i64, site.z.floor() as i64);
                let w = if rock == Rock::Dome { dome } else { tor };
                if keep(self.seed, kind, &site) >= w * kind.density {
                    return;
                }
                let profile = |r: f32, height: f32| {
                    if r >= 1.0 {
                        0.0
                    } else if site.round {
                        height * (1.0 - r * r)
                    } else {
                        height * (1.0 - r * r * r).max(0.0).sqrt()
                    }
                };
                let mut h = profile(r as f32, site.height);
                if rock == Rock::Dome {
                    // Grooves down the main dome's flanks.
                    let d = (dx * dx + dz * dz).sqrt().max(1e-6);
                    let g = groove(site.x + dx / d * FLUTE_REACH, site.z + dz / d * FLUTE_REACH);
                    let flank = (4.0 * r * (1.0 - r)).clamp(0.0, 1.0) as f32;
                    let cut = 0.5 + 0.5 * g;
                    h -= FLUTE * site.height * flank * cut * cut;
                    // Its lobes.
                    for lobe in &site.lobes[..site.lobe_count] {
                        let rl = local((dx - lobe.0, dz - lobe.1)) / (lobe.2 * scale);
                        h = h.max(profile(rl as f32, lobe.3));
                    }
                }
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
                let reach = radius * (1.0 + OUTLINE) * if k == 0 { DOME_REACH } else { 1.0 } + 1.0;
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
                    lobes: core::array::from_fn(|l| {
                        let l = l as u64;
                        let (ax, az) = (h(10 + 5 * l) - 0.5, h(11 + 5 * l) - 0.5);
                        let len = (ax * ax + az * az).sqrt().max(1e-6);
                        let off = radius
                            * (LOBE_OFFSET.0 + (LOBE_OFFSET.1 - LOBE_OFFSET.0) * h(12 + 5 * l));
                        (
                            ax / len * off,
                            az / len * off,
                            radius
                                * (LOBE_RADIUS.0 + (LOBE_RADIUS.1 - LOBE_RADIUS.0) * h(13 + 5 * l)),
                            LOBE_HEIGHT.0 + (LOBE_HEIGHT.1 - LOBE_HEIGHT.0) * h(14 + 5 * l) as f32,
                        )
                    }),
                    lobe_count: if k == 0 {
                        (LOBES.0 + ((LOBES.1 - LOBES.0 + 1) as f64 * h(9)) as u32).min(LOBES.1)
                            as usize
                    } else {
                        0
                    },
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
            let got = l.at(x, z, 0.0, |_, _| (1.0, 0.0), |_, _| 0.0);
            assert_eq!(
                got,
                l.at(
                    x + shape.size_x(),
                    z - shape.size_z(),
                    0.0,
                    |_, _| (1.0, 0.0),
                    |_, _| 0.0
                )
            );
            if let Some((h, Rock::Dome)) = got {
                found += 1;
                assert!((0.0..=150.0).contains(&h));
                // Walk out along x to the foot: the steep kind drops several
                // blocks at once there (above its apron of rubble).
                let mut k = 1;
                let mut last = h;
                let mut drop = 0.0f32;
                while let Some((h2, Rock::Dome)) =
                    l.at(x + k, z, 0.0, |_, _| (1.0, 0.0), |_, _| 0.0)
                {
                    drop = drop.max(last - h2);
                    last = h2;
                    k += 1;
                }
                steep += u32::from(drop >= 5.0);
            }
        }
        assert!(found > 20, "{found}");
        assert!(steep > 0);
        // No weight, no rock.
        assert_eq!(l.at(1_000, 1_000, 0.0, |_, _| (0.0, 0.0), |_, _| 0.0), None);
    }
}
