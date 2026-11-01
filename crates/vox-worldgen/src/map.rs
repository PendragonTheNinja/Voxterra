//! Headless world maps (M13 task 0): the whole planet as images, so climate
//! and biomes can be reviewed in one picture before anyone walks on them.
//!
//! `cargo test -p vox-worldgen --release world_map -- --ignored --nocapture`
//! writes PNGs to `target/world-map/` (or `$WORLD_MAP_DIR`). Env:
//! `WORLD_MAP_SEED` (hex or decimal), `WORLD_MAP_SIZE` (blocks, a multiple of
//! 8 192), `WORLD_MAP_PX` (pixels across, default 800).
//!
//! Test-only: nothing here ships in the game. The PNG writer is a few dozen
//! lines rather than a dependency — images are written uncompressed
//! ("stored" deflate blocks), which every viewer reads.

use std::io::Write;
use std::path::Path;

use crate::biome::Biome;

/// Write an 8-bit RGB image as a PNG.
pub(crate) fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    assert_eq!(rgb.len(), (w * h * 3) as usize, "image size");
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for row in rgb.chunks_exact((w * 3) as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    // zlib stream of stored (uncompressed) deflate blocks.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour, no interlace

    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x89PNG\r\n\x1a\n")?;
    for (kind, data) in [(b"IHDR", &ihdr), (b"IDAT", &z), (b"IEND", &Vec::new())] {
        f.write_all(&(data.len() as u32).to_be_bytes())?;
        let mut chunk = kind.to_vec();
        chunk.extend_from_slice(data);
        f.write_all(&chunk)?;
        f.write_all(&crc32(&chunk).to_be_bytes())?;
    }
    Ok(())
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &d in data {
        a = (a + u32::from(d)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &d in data {
        c ^= u32::from(d);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Linear blend between colour stops `(value, rgb)`, sorted by value.
pub(crate) fn ramp(stops: &[(f32, [u8; 3])], v: f32) -> [u8; 3] {
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if v <= first.0 {
        return first.1;
    }
    if v >= last.0 {
        return last.1;
    }
    let i = stops.iter().position(|s| s.0 > v).unwrap();
    let (a, b) = (stops[i - 1], stops[i]);
    let t = (v - a.0) / (b.0 - a.0);
    std::array::from_fn(|k| (f32::from(a.1[k]) + (f32::from(b.1[k]) - f32::from(a.1[k])) * t) as u8)
}

/// Elevation colours: deep blue to shelf cyan below sea level, lowland green
/// through brown to white above.
pub(crate) fn elevation_colour(h: i64) -> [u8; 3] {
    ramp(
        &[
            (-900.0, [8, 20, 60]),
            (-260.0, [20, 52, 120]),
            (-20.0, [60, 120, 190]),
            (-1.0, [110, 170, 215]),
            (0.0, [70, 140, 70]),
            (120.0, [120, 160, 80]),
            (350.0, [170, 150, 90]),
            (700.0, [140, 110, 80]),
            (1_000.0, [200, 200, 200]),
            (1_300.0, [255, 255, 255]),
        ],
        h as f32,
    )
}

/// Discharge from which the rivers map draws a river, m³/s.
const RIVER_FROM_M3S: f32 = 0.5;

/// A colour per biome, chosen to read like an atlas: greens for forest,
/// yellows and tans for grass and dry land, white for ice, blues for sea.
pub(crate) fn biome_colour(b: Biome) -> [u8; 3] {
    match b {
        Biome::TropicalRainforest => [18, 100, 32],
        Biome::TropicalDryForest => [92, 130, 40],
        Biome::Savanna => [190, 180, 80],
        Biome::HotDesert => [235, 205, 140],
        Biome::HotSteppe => [210, 175, 110],
        Biome::Mediterranean => [150, 160, 90],
        Biome::TemperateGrassland => [175, 200, 110],
        Biome::ColdDesert => [190, 180, 160],
        Biome::TemperateForest => [60, 140, 60],
        Biome::TemperateRainforest => [30, 110, 90],
        Biome::BorealForest => [40, 90, 70],
        Biome::Tundra => [150, 150, 120],
        Biome::IceSheet => [245, 250, 255],
        Biome::AlpineMeadow => [140, 170, 120],
        Biome::AlpineRock => [170, 165, 160],
        Biome::Badlands => [190, 100, 50],
        Biome::SandBeach => [245, 230, 170],
        Biome::ShingleBeach => [150, 145, 135],
        Biome::RockyCoast => [110, 105, 100],
        Biome::WarmShallowSea => [60, 170, 200],
        Biome::TemperateSea => [50, 110, 170],
        Biome::PolarSea => [170, 200, 225],
        Biome::DeepOcean => [20, 40, 100],
        Biome::River => [40, 90, 220],
        Biome::Lake => [90, 200, 230],
        Biome::SaltLake => [250, 245, 235],
        Biome::Floodplain => [140, 190, 70],
        Biome::Marsh => [110, 150, 110],
        Biome::Swamp => [70, 100, 60],
        Biome::Bog => [120, 95, 70],
        Biome::Delta => [200, 170, 120],
        Biome::MangroveCoast => [60, 120, 80],
    }
}

/// Mean annual temperature colours, °C: ice white through blue, green at
/// temperate, to red in the hot tropics.
pub(crate) fn temperature_colour(c: f32) -> [u8; 3] {
    ramp(
        &[
            (-30.0, [255, 255, 255]),
            (-10.0, [150, 180, 255]),
            (0.0, [60, 90, 220]),
            (8.0, [60, 170, 200]),
            (15.0, [90, 190, 90]),
            (21.0, [230, 210, 70]),
            (27.0, [220, 70, 40]),
        ],
        c,
    )
}

/// Annual precipitation colours, mm/yr: desert sand to forest green to deep
/// blue.
pub(crate) fn precipitation_colour(mm: f32) -> [u8; 3] {
    ramp(
        &[
            (0.0, [150, 90, 40]),
            (150.0, [230, 200, 120]),
            (400.0, [210, 220, 120]),
            (800.0, [100, 180, 80]),
            (1_500.0, [30, 120, 60]),
            (2_500.0, [30, 90, 160]),
            (5_000.0, [40, 20, 120]),
        ],
        mm,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Generator;
    use crate::climate::{ClimateMap, temperature_at};
    use crate::drainage::Cover;
    use vox_core::WorldShape;

    #[test]
    fn checksums_match_their_definitions() {
        // Standard check values for "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"123456789"), 0x091E_01DE);
    }

    #[test]
    fn ramps_hit_their_stops_and_clamp() {
        let r = [(0.0, [0, 0, 0]), (10.0, [100, 200, 50])];
        assert_eq!(ramp(&r, -5.0), [0, 0, 0]);
        assert_eq!(ramp(&r, 5.0), [50, 100, 25]);
        assert_eq!(ramp(&r, 99.0), [100, 200, 50]);
    }

    /// Which world to map, from the environment.
    pub(crate) fn map_world() -> (Generator, u32, std::path::PathBuf) {
        let seed = std::env::var("WORLD_MAP_SEED")
            .ok()
            .and_then(|s| {
                let s = s.trim();
                match s.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => s.parse().ok(),
                }
            })
            .unwrap_or(0x7e22ac0de);
        let size = std::env::var("WORLD_MAP_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(WorldShape::DEFAULT.size_x());
        let shape = WorldShape::new(size, size).expect("a legal world size");
        let px = std::env::var("WORLD_MAP_PX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(800);
        let dir = std::env::var("WORLD_MAP_DIR").map_or_else(
            |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/world-map"),
            Into::into,
        );
        std::fs::create_dir_all(&dir).unwrap();
        (Generator::new(seed, shape), px, dir)
    }

    /// The planet's elevation, one pixel per `size / px` blocks, with the
    /// equators, tropics and polar circles marked: the picture every later
    /// map is read against.
    #[test]
    #[ignore]
    fn world_map() {
        let (g, px, dir) = map_world();
        let shape = g.shape();
        let step = shape.size_x() as f64 / f64::from(px);
        let mut rgb = vec![0u8; (px * px * 3) as usize];
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            let lat = shape.latitude_degrees(z).abs();
            // Equator, tropics (23.4°), polar circles (66.6°).
            let line = [0.0, 23.4, 66.6].iter().any(|&l| {
                let next = shape
                    .latitude_degrees(((f64::from(j) + 1.0) * step) as i64)
                    .abs();
                (lat - l) * (next - l) <= 0.0
            });
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let c = if line && i % 4 < 2 {
                    [0, 0, 0]
                } else {
                    elevation_colour(g.surface_height(x, z))
                };
                let at = ((j * px + i) * 3) as usize;
                rgb[at..at + 3].copy_from_slice(&c);
            }
        }
        let path = dir.join(format!("elevation-{:x}-{}.png", g.seed(), shape.size_x()));
        write_png(&path, px, px, &rgb).unwrap();
        println!("wrote {}", path.display());

        // Climate (stage 1), then per pixel: temperature at the column's own
        // height, and precipitation.
        let t = std::time::Instant::now();
        let climate = ClimateMap::build(g.elevation(), shape);
        println!(
            "climate map: {:?} cells of {:.0} blocks, built in {:.0} ms",
            climate.cells(),
            climate.cell_blocks().0,
            t.elapsed().as_secs_f64() * 1000.0
        );
        let (mut temp, mut rain) = (rgb.clone(), rgb.clone());
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let h = g.surface_height(x, z);
                let at = ((j * px + i) * 3) as usize;
                let sea = h <= 0;
                let tc = temperature_at(climate.sea_level_temperature(x, z), h);
                let mut c = temperature_colour(tc);
                let mut r = precipitation_colour(climate.precipitation(x, z));
                if sea {
                    // The sea, dimmed, so the land reads first.
                    c = c.map(|v| v / 2);
                    r = r.map(|v| v / 2);
                }
                if rgb[at..at + 3] == [0, 0, 0] {
                    (c, r) = ([0, 0, 0], [0, 0, 0]); // latitude lines
                }
                temp[at..at + 3].copy_from_slice(&c);
                rain[at..at + 3].copy_from_slice(&r);
            }
        }
        // Land shares by climate, beside Earth's (land excluding Antarctica
        // and Greenland's ice for the rain bins).
        let (mut n, mut p_bins, mut t_bins) = (0u32, [0u32; 5], [0u32; 5]);
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let h = g.surface_height(x, z);
                if h <= 0 {
                    continue;
                }
                n += 1;
                let p = climate.precipitation(x, z);
                p_bins[[250.0, 500.0, 1_000.0, 2_000.0]
                    .iter()
                    .filter(|&&b| p >= b)
                    .count()] += 1;
                let t = temperature_at(climate.sea_level_temperature(x, z), h);
                t_bins[[-3.0, 5.0, 15.0, 22.0].iter().filter(|&&b| t >= b).count()] += 1;
            }
        }
        // Mean land precipitation per 10° of latitude (both hemispheres).
        let mut bands = [(0.0f64, 0u32); 9];
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            let b = ((shape.latitude_degrees(z).abs() / 10.0) as usize).min(8);
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                if g.surface_height(x, z) > 0 {
                    bands[b].0 += f64::from(climate.precipitation(x, z));
                    bands[b].1 += 1;
                }
            }
        }
        let means: Vec<u32> = bands
            .iter()
            .map(|&(sum, k)| {
                if k > 0 {
                    (sum / f64::from(k)) as u32
                } else {
                    0
                }
            })
            .collect();
        println!("mean land mm/yr by 10° band from the equator: {means:?}");
        let pct = |b: &[u32; 5]| b.map(|v| (100.0 * v as f64 / n as f64).round() as u32);
        println!(
            "land precipitation % <250 / 250-500 / 500-1000 / 1000-2000 / >2000: {:?} (Earth ~19/16/27/23/15)",
            pct(&p_bins)
        );
        println!(
            "land temperature % <-3 / -3..5 / 5..15 / 15..22 / >22 C: {:?}",
            pct(&t_bins)
        );
        // Biomes, from the generator's own classification.
        let mut biomes = rgb.clone();
        let mut counts = [0u32; Biome::ALL.len()];
        let mut land = 0u32;
        for j in 0..px {
            let z = (f64::from(j) * step) as i64;
            for i in 0..px {
                let x = (f64::from(i) * step) as i64;
                let col = g.column(x, z);
                counts[col.biome as usize] += 1;
                land += u32::from(!col.biome.is_sea());
                let at = ((j * px + i) * 3) as usize;
                if rgb[at..at + 3] != [0, 0, 0] {
                    biomes[at..at + 3].copy_from_slice(&biome_colour(col.biome));
                }
            }
        }
        let total = px * px;
        for b in Biome::ALL {
            let n = counts[b as usize];
            let of = if b.is_sea() { total } else { land };
            println!(
                "  {:<22} {:>5.1}% of {}",
                b.name(),
                100.0 * f64::from(n) / f64::from(of),
                if b.is_sea() { "the world" } else { "land" }
            );
        }
        // Drainage (M14): rivers by discharge and lakes, at the network's
        // own resolution, over the elevation.
        let t = std::time::Instant::now();
        let drainage = crate::drainage::DrainageMap::build(g.elevation(), &climate, shape);
        println!(
            "drainage: {:?}, built in {:.0} ms",
            drainage,
            t.elapsed().as_secs_f64() * 1000.0
        );
        let (dx, dz) = drainage.cells();
        let endorheic = drainage.endorheic();
        let mut rivers = vec![0u8; dx * dz * 3];
        let (mut land_cells, mut biggest) = (0u32, 0.0f32);
        let (mut lake_cells, mut salt_cells, mut closed_cells) = (0u32, 0u32, 0u32);
        for j in 0..dz {
            for i in 0..dx {
                let (h, _, q, _) = drainage.cell(i, j);
                let sea = drainage.is_sea(i, j);
                // Inland pits below sea level are land: draw them as lowland.
                let mut c = elevation_colour(i64::from(if sea { h } else { h.max(1) }));
                if !sea {
                    land_cells += 1;
                    biggest = biggest.max(q);
                    if endorheic[j * dx + i] {
                        // Land draining to a closed basin: tinted sand.
                        closed_cells += 1;
                        c = ramp(&[(0.0, c), (1.0, [220, 190, 120])], 0.45);
                    }
                    match drainage.cover(i, j) {
                        Cover::Lake => {
                            lake_cells += 1;
                            c = [90, 200, 230];
                        }
                        Cover::SaltLake => {
                            lake_cells += 1;
                            salt_cells += 1;
                            c = [60, 170, 160];
                        }
                        Cover::SaltFlat => {
                            salt_cells += 1;
                            c = [245, 240, 225];
                        }
                        _ if q >= RIVER_FROM_M3S => {
                            // Brighter and bluer with discharge, log scale.
                            let k = ((q / RIVER_FROM_M3S).log10() / 3.0).clamp(0.0, 1.0);
                            c = ramp(&[(0.0, [70, 120, 200]), (1.0, [10, 30, 140])], k);
                        }
                        _ => {}
                    }
                }
                let at = (j * dx + i) * 3;
                rivers[at..at + 3].copy_from_slice(&c);
            }
        }
        // Valleys (task 2): the eroded ground, hill-shaded so valleys read,
        // and how deep erosion cut.
        let mut valleys = vec![0u8; dx * dz * 3];
        let (mut deepest, mut cut10, mut cut50) = (0.0f32, 0u32, 0u32);
        for j in 0..dz {
            for i in 0..dx {
                let h = |di: isize, dj: isize| {
                    let ii = (i as isize + di).rem_euclid(dx as isize) as usize;
                    let jj = (j as isize + dj).rem_euclid(dz as isize) as usize;
                    drainage.cell(ii, jj).0 as f32
                };
                let (here, cut) = (drainage.cell(i, j).0, drainage.erosion(i, j));
                if !drainage.is_sea(i, j) {
                    deepest = deepest.min(cut);
                    cut10 += u32::from(cut <= -10.0);
                    cut50 += u32::from(cut <= -50.0);
                }
                let shade = (1.0 + (h(-1, -1) - h(1, 1)) / 60.0).clamp(0.45, 1.5);
                let c = elevation_colour(i64::from(if drainage.is_sea(i, j) {
                    here
                } else {
                    here.max(1)
                }));
                let at = (j * dx + i) * 3;
                for k in 0..3 {
                    valleys[at + k] = (f32::from(c[k]) * shade).min(255.0) as u8;
                }
            }
        }
        let path = dir.join(format!("valleys-{:x}-{}.png", g.seed(), shape.size_x()));
        write_png(&path, dx as u32, dz as u32, &valleys).unwrap();
        let share = |k: u32| 100.0 * f64::from(k) / f64::from(land_cells);
        println!(
            "erosion: deepest cut {:.0} blocks; {:.1}% of land cut 10+ blocks, {:.1}% 50+",
            -deepest,
            share(cut10),
            share(cut50)
        );
        println!(
            "lakes cover {:.1}% of land, salt lakes and flats {:.1}%; {:.1}% of land drains to closed basins; the largest discharge is {:.1} m³/s",
            share(lake_cells),
            share(salt_cells),
            share(closed_cells),
            biggest
        );
        let path = dir.join(format!("rivers-{:x}-{}.png", g.seed(), shape.size_x()));
        write_png(&path, dx as u32, dz as u32, &rivers).unwrap();
        println!("wrote {}", path.display());

        for (name, img) in [
            ("temperature", &temp),
            ("precipitation", &rain),
            ("biomes", &biomes),
        ] {
            let path = dir.join(format!("{name}-{:x}-{}.png", g.seed(), shape.size_x()));
            write_png(&path, px, px, img).unwrap();
            println!("wrote {}", path.display());
        }
    }
}

/// The drainage network over many worlds (M14 criterion 4), against Earth:
/// closed basins, lakes by ground, the longest river, Hack's law, drainage
/// density by climate.
///
/// `cargo test -p vox-worldgen --release drainage_survey -- --ignored
/// --nocapture`; env `SURVEY_SEEDS` (default 6) and `SURVEY_SIZES`.
#[cfg(test)]
mod drainage_survey {
    use crate::Generator;
    use vox_core::WorldShape;

    #[test]
    #[ignore]
    fn drainage_survey() {
        let seeds: u64 = std::env::var("SURVEY_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        let sizes: Vec<i64> = std::env::var("SURVEY_SIZES").ok().map_or_else(
            || vec![WorldShape::DEFAULT.size_x()],
            |s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect(),
        );
        for size in sizes {
            let shape = WorldShape::new(size, size).expect("a legal size");
            println!("world {} km:", size / 1000);
            println!(
                "  seed           closed  lakes ice/other  longest  Hack  streams arid/sub/humid  largest"
            );
            for seed in 0..seeds {
                let g = Generator::new(seed.wrapping_mul(0x9E37_79B9) ^ 0x7e22ac0de, shape);
                let s = g.drainage().stats(g.climate());
                println!(
                    "  {:#013x}  {:5.1}%  {:5.1}% / {:4.1}%  {:5.0} km  {:.2}  {:4.1}% / {:4.1}% / {:4.1}%  {:5.1} m³/s",
                    g.seed(),
                    100.0 * s.endorheic,
                    100.0 * s.lakes_cold,
                    100.0 * s.lakes_warm,
                    s.longest_km,
                    s.hack,
                    100.0 * s.density[0],
                    100.0 * s.density[1],
                    100.0 * s.density[2],
                    s.largest
                );
            }
        }
    }
}

/// Biome shares over many worlds (M13 criteria 3 and 5): every biome in
/// every world, in Earth-like proportions.
///
/// `cargo test -p vox-worldgen --release biome_survey -- --ignored
/// --nocapture`; env `SURVEY_SEEDS` (default 8) and `SURVEY_SIZES`
/// (comma-separated blocks, default the default size).
#[cfg(test)]
mod survey {
    use crate::Generator;
    use crate::biome::Biome;
    use vox_core::WorldShape;

    /// Per-biome share of a world: land biomes of its land, sea biomes of
    /// the world. Sampled on a grid, plus a finer pass along the shores,
    /// where coasts are narrower than any coarse grid.
    pub(crate) fn shares(g: &Generator, px: i64) -> ([f64; Biome::ALL.len()], Vec<i64>, Vec<i64>) {
        let shape = g.shape();
        let step = shape.size_x() / px;
        let (mut counts, mut land, mut total) = ([0u64; Biome::ALL.len()], 0u64, 0u64);
        for j in 0..px {
            for i in 0..px {
                let b = g.column(i * step, j * step).biome;
                counts[b as usize] += 1;
                total += 1;
                land += u64::from(!b.is_sea());
            }
        }
        let mut out = [0.0; Biome::ALL.len()];
        for b in Biome::ALL {
            let of = if b.is_sea() { total } else { land.max(1) };
            out[b as usize] = counts[b as usize] as f64 / of as f64;
        }
        // Coasts are narrower than any coarse grid: measure them exactly,
        // on a few whole rows walked block by block.
        let (mut coast, mut row_land) = ([0u64; Biome::ALL.len()], 0u64);
        // Lengths of unbroken land-biome stretches along those rows (coasts
        // left out: they are shore, not a place to cross).
        let mut runs = Vec::new();
        // As many rows as ~6 M columns allow, at most 64.
        let rows = (6_000_000 / shape.size_x()).clamp(4, 64);
        for j in (0..px).step_by((px / rows).max(1) as usize) {
            let z = j * step;
            let (mut current, mut len) = (None, 0i64);
            for x in 0..shape.size_x() {
                let b = g.column(x, z).biome;
                if !b.is_sea() {
                    row_land += 1;
                    coast[b as usize] += u64::from(b.is_coast());
                }
                let place = (!b.is_sea() && !b.is_coast()).then_some(b);
                if place == current {
                    len += 1;
                } else {
                    if current.is_some() {
                        runs.push(len);
                    }
                    (current, len) = (place, 1);
                }
            }
        }
        for b in Biome::ALL.into_iter().filter(|b| b.is_coast()) {
            out[b as usize] = coast[b as usize] as f64 / row_land.max(1) as f64;
        }
        // River mouths (M14): deltas and mangrove coasts are narrower still,
        // and where they are is known — beside the reaches that reach the
        // sea. Looked for there, finely; found ones count as present.
        let d = g.drainage();
        let (dnx, dnz) = d.cells();
        let (cw, ch) = d.cell_blocks();
        // The 400 largest mouths, whatever the world's size.
        let mut mouths = Vec::new();
        for j in 0..dnz {
            for i in 0..dnx {
                let (_, _, q, (ri, rj)) = d.cell(i, j);
                if !d.is_sea(i, j) && d.is_sea(ri, rj) && q > 0.0 {
                    mouths.push((q, i, j));
                }
            }
        }
        mouths.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(_, i, j) in mouths.iter().take(400) {
            let (x0, z0) = ((i as f64 * cw) as i64, (j as f64 * ch) as i64);
            for dz in (-64..192).step_by(8) {
                for dx in (-64..192).step_by(8) {
                    let b = g.column(x0 + dx, z0 + dz).biome;
                    if matches!(b, Biome::Delta | Biome::MangroveCoast) && out[b as usize] == 0.0 {
                        out[b as usize] = 1e-6;
                    }
                }
            }
        }
        // And north–south, across the climate belts rather than along them.
        let mut ns_runs = Vec::new();
        for i in (0..px).step_by((px / rows).max(1) as usize) {
            let x = i * step;
            let (mut current, mut len) = (None, 0i64);
            for z in 0..shape.size_z() {
                let b = g.column(x, z).biome;
                let place = (!b.is_sea() && !b.is_coast()).then_some(b);
                if place == current {
                    len += 1;
                } else {
                    if current.is_some() {
                        ns_runs.push(len);
                    }
                    (current, len) = (place, 1);
                }
            }
        }
        (out, runs, ns_runs)
    }

    /// M13 criterion 3: every biome in the catalogue, in every world, for
    /// every seed tried, at every size from the smallest. Minutes in a
    /// release build:
    /// `cargo test -p vox-worldgen --release every_biome -- --ignored`.
    #[test]
    #[ignore]
    fn every_biome_in_every_world() {
        for size in [32_768, 65_536, 204_800, 524_288] {
            let shape = WorldShape::new(size, size).unwrap();
            for seed in 0..8u64 {
                let g = Generator::new(seed.wrapping_mul(0x0051_7CC1_B727) ^ 0x000B_103E, shape);
                let (s, _, _) = shares(&g, 300);
                let missing: Vec<&str> = Biome::ALL
                    .iter()
                    .filter(|b| s[**b as usize] == 0.0)
                    .map(|b| b.name())
                    .collect();
                assert!(
                    missing.is_empty(),
                    "size {size}, seed {:#x}: no {missing:?}",
                    g.seed()
                );
            }
        }
    }

    #[test]
    #[ignore]
    fn biome_survey() {
        let seeds: u64 = std::env::var("SURVEY_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let sizes: Vec<i64> = std::env::var("SURVEY_SIZES").ok().map_or_else(
            || vec![WorldShape::DEFAULT.size_x()],
            |s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect(),
        );
        for size in sizes {
            let shape = WorldShape::new(size, size).expect("a legal size");
            let mut sum = [0.0f64; Biome::ALL.len()];
            let mut min = [f64::MAX; Biome::ALL.len()];
            let mut missing = [0u32; Biome::ALL.len()];
            let (mut runs, mut ns_runs) = (Vec::new(), Vec::new());
            for seed in 0..seeds {
                let g = Generator::new(seed.wrapping_mul(0x9E37_79B9) ^ 0x7e22ac0de, shape);
                let (s, r, ns) = shares(&g, 300);
                runs.extend(r);
                ns_runs.extend(ns);
                for k in 0..Biome::ALL.len() {
                    sum[k] += s[k];
                    min[k] = min[k].min(s[k]);
                    missing[k] += u32::from(s[k] == 0.0);
                }
            }
            // Walking scale (criterion 6): stretches weighted by length — the
            // stretch a walker is standing in — at ground sprint.
            for (dir, mut runs) in [("east-west", runs), ("north-south", ns_runs)] {
                let total: i64 = runs.iter().sum();
                runs.sort_unstable();
                let (mut acc, mut median) = (0i64, 0i64);
                for &r in &runs {
                    acc += r;
                    if acc * 2 >= total {
                        median = r;
                        break;
                    }
                }
                const SPRINT: f64 = 5.612;
                println!(
                    "size {size}, walking {dir}: the biome stretch a walker stands in is {median} blocks \
                     (median by length), {:.1} min at sprint; {:.1}% of land lies in stretches under 100 blocks",
                    median as f64 / SPRINT / 60.0,
                    100.0 * runs.iter().filter(|&&r| r < 100).sum::<i64>() as f64 / total as f64
                );
            }
            println!("size {size}, {seeds} seeds: mean% (min%) [seeds without it]");
            for b in Biome::ALL {
                let k = b as usize;
                println!(
                    "  {:<22} {:>5.1} ({:>4.2}) [{}]",
                    b.name(),
                    100.0 * sum[k] / seeds as f64,
                    100.0 * min[k],
                    missing[k]
                );
            }
        }
    }
}
