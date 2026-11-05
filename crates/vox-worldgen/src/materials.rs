//! What each biome is made of, from the surface down (M13 task 3,
//! `docs/biomes.md`).
//!
//! A column's block at world height `wy` is a pure function of its
//! [`Column`] and `wy` — so two chunks stacked vertically agree at their
//! seam, the chunk generator and the LOD sampler agree on the top block, and
//! nothing reads a neighbour.
//!
//! Every profile ends within [`MAX_PROFILE_DEPTH`] blocks of the surface,
//! with stone beneath: that is what lets the chunk generator return a
//! uniform stone chunk, without classifying a single column, for any chunk
//! wholly deeper than that below its lowest surface — the commonest chunk in
//! the stream (CLAUDE.md lesson 6). Geology replaces "stone" later.

use vox_core::registry as r;
use vox_core::{BlockId, SEA_LEVEL_BLOCKS};

use crate::biome::{Biome, Column};
use crate::climate::SNOW_LINE_C;
use crate::drainage::{Bed, Body};

/// No profile reaches deeper than this below the surface: below it every
/// column is stone. Kept shallow on purpose — every block of it turns a
/// layer of free uniform-stone chunks into classified mixed ones (24 cost a
/// tenth of all chunks their fast path, M13 task 3). Deep strata are
/// geology's.
pub const MAX_PROFILE_DEPTH: i64 = 12;

/// A lake's surface freezes below this mean annual temperature, °C (the
/// sea's own threshold, `SEA_ICE_BELOW_C`).
const LAKE_ICE_BELOW_C: f32 = -2.0;

/// Thickness of one badlands stratum, blocks.
const STRATUM_BLOCKS: i64 = 3;
/// How deep a badlands profile's banded rock goes.
const BADLANDS_DEPTH: i64 = MAX_PROFILE_DEPTH;
/// Alpine rock carries snow on top this far below the snowline's temperature.
const SNOW_CAP_BELOW_C: f32 = SNOW_LINE_C - 3.0;

/// The block at height `wy` in `col`. `strata(wy)` gives the badlands rock
/// for a height (per world: [`crate::Generator`] picks the band sequence).
pub fn block_in(col: &Column, wy: i64, strata: impl Fn(i64) -> BlockId) -> BlockId {
    let h = col.surface.height;
    let w = col.water;
    // Rivers and lakes (M14): settled, full water blocks up to their own
    // level — the hybrid's layer 1 (`docs/design/water.md`). A lake freezes
    // at its surface where the sea would.
    if wy > h
        && wy <= w.level
        && matches!(
            w.body,
            Body::River(_) | Body::Lake | Body::SaltLake | Body::Pool
        )
    {
        return if wy == w.level && w.body == Body::Lake && col.temperature < LAKE_ICE_BELOW_C {
            r::ICE
        } else {
            r::WATER
        };
    }
    if wy <= h {
        // What water lays on the ground under it.
        let d = h - wy;
        let bed = match w.body {
            Body::River(Bed::Gravel) => Some((r::RIVER_GRAVEL, 2)),
            Body::River(Bed::Sand) => Some((r::SAND, 2)),
            Body::River(Bed::Silt) | Body::Lake => Some((r::SILT, 2)),
            Body::SaltLake | Body::SaltFlat => Some((if d == 0 { r::SALT } else { r::SILT }, 2)),
            // A wetland pool's floor: peat in a bog, mud elsewhere.
            Body::Pool => Some((
                if col.biome == Biome::Bog {
                    r::PEAT
                } else {
                    r::MUD
                },
                1,
            )),
            Body::Dry | Body::Wetland | Body::Floodplain | Body::Delta => None,
        };
        if let Some((block, thick)) = bed
            && d < thick
        {
            return block;
        }
    }
    // Bare rock (M15): an inselberg's dome, a tor — no soil.
    if wy <= h && col.surface.rock {
        return r::STONE;
    }
    // Dune sand (M15): the dune's own body, and one block under it.
    if wy <= h && h - wy <= col.surface.sand && col.surface.sand > 0 {
        return r::SAND;
    }
    if wy > h {
        // Above the ground: the sea fills up to sea level (ADR-0011), and
        // freezes at the surface in polar seas.
        return if wy > SEA_LEVEL_BLOCKS {
            r::AIR
        } else if wy == SEA_LEVEL_BLOCKS && col.biome == Biome::PolarSea {
            r::ICE
        } else {
            r::WATER
        };
    }
    let d = h - wy; // 0 is the top block
    if d >= MAX_PROFILE_DEPTH {
        return r::STONE;
    }
    let p = col.patch;
    let pick = |cut: f32, above: BlockId, below: BlockId| if p > cut { above } else { below };
    match col.biome {
        // --- Seabeds.
        Biome::WarmShallowSea => layers(d, &[(r::SAND, 3), (r::SANDSTONE, 3)]),
        Biome::TemperateSea => layers(d, &[(pick(0.3, r::GRAVEL, r::SAND), 3)]),
        Biome::PolarSea => layers(d, &[(r::GRAVEL, 2)]),
        Biome::DeepOcean => layers(d, &[(r::MUD, 1), (r::CLAY, 3)]),
        // --- Water (M14): the body lays the top of a bed (river gravel,
        // sand or silt; silt; salt); these are what lies under it, and the
        // ground of the lowlands.
        Biome::River => layers(d, &[(r::GRAVEL, 4)]),
        Biome::Lake => layers(d, &[(r::SILT, 2), (r::CLAY, 3)]),
        Biome::SaltLake => layers(d, &[(r::SALT, 1), (r::SILT, 3)]),
        Biome::Floodplain => layers(d, &[(r::GRASS, 1), (r::SILT, 3), (r::RIVER_GRAVEL, 2)]),
        Biome::Marsh => layers(d, &[(pick(-0.1, r::GRASS, r::MUD), 1), (r::MUD, 3)]),
        Biome::Swamp => layers(d, &[(pick(0.2, r::GRASS, r::MUD), 1), (r::MUD, 3)]),
        Biome::Bog => layers(d, &[(pick(0.4, r::GRASS, r::PEAT), 1), (r::PEAT, 3)]),
        Biome::Delta => layers(d, &[(pick(0.0, r::SAND, r::MUD), 2), (r::SILT, 3)]),
        Biome::MangroveCoast => layers(d, &[(r::MUD, 3), (r::SILT, 2)]),
        // --- Coasts.
        Biome::SandBeach => layers(d, &[(r::SAND, 4), (r::SANDSTONE, 3)]),
        Biome::ShingleBeach => layers(d, &[(r::GRAVEL, 3)]),
        Biome::RockyCoast => layers(d, &[(pick(0.4, r::GRAVEL, r::STONE), 1)]),
        // --- Tropics.
        Biome::TropicalRainforest => layers(d, &[(r::GRASS, 1), (r::DIRT, 3), (r::CLAY, 3)]),
        Biome::TropicalDryForest => layers(d, &[(r::GRASS, 1), (r::DIRT, 3)]),
        Biome::Savanna => layers(
            d,
            &[
                (pick(-0.5, r::GRASS, r::COARSE_DIRT), 1),
                (r::COARSE_DIRT, 3),
            ],
        ),
        // --- Dry lands.
        Biome::HotDesert => {
            if col.surface.relief >= 0.3 {
                // Rocky desert — hamada: bare sandstone, sand in hollows.
                layers(
                    d,
                    &[(pick(0.45, r::SAND, r::SANDSTONE), 1), (r::SANDSTONE, 8)],
                )
            } else {
                layers(d, &[(r::SAND, 4), (r::SANDSTONE, 6)])
            }
        }
        Biome::HotSteppe => {
            let top = if p > 0.45 {
                r::SAND
            } else if p < -0.35 {
                r::COARSE_DIRT
            } else {
                r::GRASS
            };
            layers(d, &[(top, 1), (r::DIRT, 3)])
        }
        Biome::ColdDesert => layers(d, &[(pick(0.2, r::SAND, r::GRAVEL), 1), (r::GRAVEL, 2)]),
        Biome::Badlands => {
            if d < 2 {
                r::RED_SAND
            } else if d < BADLANDS_DEPTH {
                strata(wy)
            } else {
                r::STONE
            }
        }
        // --- Temperate.
        Biome::Mediterranean => layers(
            d,
            &[
                (pick(0.4, r::COARSE_DIRT, r::GRASS), 1),
                (r::COARSE_DIRT, 2),
            ],
        ),
        Biome::TemperateGrassland => layers(d, &[(r::GRASS, 1), (r::DIRT, 5)]),
        Biome::TemperateForest => layers(d, &[(r::GRASS, 1), (r::DIRT, 3)]),
        Biome::TemperateRainforest => layers(d, &[(r::GRASS, 1), (r::DIRT, 4)]),
        // --- Cold.
        Biome::BorealForest => layers(d, &[(pick(0.55, r::GRASS, r::PODZOL), 1), (r::DIRT, 3)]),
        Biome::Tundra => layers(d, &[(pick(0.3, r::GRAVEL, r::GRASS), 1), (r::DIRT, 1)]),
        Biome::IceSheet => layers(d, &[(r::SNOW, 3), (r::ICE, MAX_PROFILE_DEPTH - 3)]),
        // --- Mountains.
        Biome::AlpineMeadow => layers(d, &[(pick(0.55, r::STONE, r::GRASS), 1), (r::DIRT, 1)]),
        Biome::AlpineRock => {
            let top = if col.temperature < SNOW_CAP_BELOW_C || p > 0.1 {
                r::SNOW
            } else if p < -0.4 {
                r::GRAVEL
            } else {
                r::STONE
            };
            layers(d, &[(top, 1)])
        }
    }
}

/// The block at depth `d` in a stack of `(block, thickness)` layers over
/// stone.
fn layers(d: i64, stack: &[(BlockId, i64)]) -> BlockId {
    let mut top = 0;
    for &(block, thick) in stack {
        top += thick;
        if d < top {
            return block;
        }
    }
    r::STONE
}

/// The badlands rock for a stratum index: four bands in a per-world order,
/// so a cliff reads as layered rock (Monument Valley, the Painted Desert).
pub fn stratum_block(seed: u64, wy: i64) -> BlockId {
    const BANDS: [BlockId; 4] = [
        r::BADLANDS_ORANGE,
        r::BADLANDS_RED,
        r::BADLANDS_TAN,
        r::RED_SANDSTONE,
    ];
    let band = wy.div_euclid(STRATUM_BLOCKS) as u64;
    let mut h = seed ^ band.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    BANDS[(h % 4) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevation::Surface;

    fn col(biome: Biome, height: i64, patch: f32) -> Column {
        Column {
            surface: Surface {
                height,
                relief: 0.1,
                uplift: 0.0,
                sand: 0,
                rock: false,
            },
            temperature: 10.0,
            sea_level_temperature: 10.0,
            precipitation: 800.0,
            biome,
            patch,
            water: crate::drainage::Water::DRY,
        }
    }

    /// The fast path's promise: every biome, every patch, is stone from
    /// `MAX_PROFILE_DEPTH` down — and has its surface block on top.
    #[test]
    fn every_profile_ends_in_stone_within_the_maximum_depth() {
        for biome in Biome::ALL {
            for patch in [-1.0, -0.45, 0.0, 0.35, 0.6, 1.0] {
                let h = if biome.is_sea() { -50 } else { 80 };
                let c = col(biome, h, patch);
                for d in MAX_PROFILE_DEPTH..MAX_PROFILE_DEPTH + 40 {
                    assert_eq!(
                        block_in(&c, h - d, |y| stratum_block(1, y)),
                        r::STONE,
                        "{biome:?}"
                    );
                }
                assert_ne!(
                    block_in(&c, h, |y| stratum_block(1, y)),
                    r::AIR,
                    "{biome:?}"
                );
            }
        }
    }

    #[test]
    fn the_sea_fills_to_sea_level_and_freezes_in_polar_seas() {
        let temperate = col(Biome::TemperateSea, -30, 0.0);
        assert_eq!(block_in(&temperate, -10, |_| r::STONE), r::WATER);
        assert_eq!(
            block_in(&temperate, SEA_LEVEL_BLOCKS, |_| r::STONE),
            r::WATER
        );
        assert_eq!(
            block_in(&temperate, SEA_LEVEL_BLOCKS + 1, |_| r::STONE),
            r::AIR
        );
        let polar = col(Biome::PolarSea, -30, 0.0);
        assert_eq!(block_in(&polar, SEA_LEVEL_BLOCKS, |_| r::STONE), r::ICE);
        assert_eq!(
            block_in(&polar, SEA_LEVEL_BLOCKS - 1, |_| r::STONE),
            r::WATER
        );
        assert_eq!(block_in(&polar, -30, |_| r::STONE), r::GRAVEL);
    }

    #[test]
    fn surfaces_read_as_their_biomes() {
        let top = |b, p| block_in(&col(b, 50, p), 50, |y| stratum_block(7, y));
        assert_eq!(top(Biome::HotDesert, 0.0), r::SAND);
        assert_eq!(top(Biome::IceSheet, 0.0), r::SNOW);
        assert_eq!(top(Biome::BorealForest, 0.0), r::PODZOL);
        assert_eq!(top(Biome::TemperateForest, 0.0), r::GRASS);
        assert_eq!(top(Biome::Badlands, 0.0), r::RED_SAND);
        assert_eq!(top(Biome::ShingleBeach, 0.0), r::GRAVEL);
        // Patches: sand through a steppe, grass through it otherwise.
        assert_eq!(top(Biome::HotSteppe, 0.6), r::SAND);
        assert_eq!(top(Biome::HotSteppe, 0.0), r::GRASS);
        // Under the ice sheet's snow, ice.
        assert_eq!(
            block_in(&col(Biome::IceSheet, 50, 0.0), 40, |_| r::STONE),
            r::ICE
        );
    }

    /// Badlands strata are horizontal: the same height, the same band, in
    /// every badlands column of a world; and the bands vary with height.
    #[test]
    fn badlands_rock_is_banded_by_height() {
        let a = col(Biome::Badlands, 120, 0.0);
        let b = col(Biome::Badlands, 117, 0.9);
        for wy in 109..=115 {
            let s = |y| stratum_block(42, y);
            assert_eq!(block_in(&a, wy, s), block_in(&b, wy, s));
        }
        let bands: std::collections::HashSet<BlockId> =
            (0..60).map(|y| stratum_block(42, y)).collect();
        assert!(bands.len() >= 3, "{bands:?}");
    }
}
