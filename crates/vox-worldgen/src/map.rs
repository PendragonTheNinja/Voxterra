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
        for (name, img) in [("temperature", &temp), ("precipitation", &rain)] {
            let path = dir.join(format!("{name}-{:x}-{}.png", g.seed(), shape.size_x()));
            write_png(&path, px, px, img).unwrap();
            println!("wrote {}", path.display());
        }
    }
}
