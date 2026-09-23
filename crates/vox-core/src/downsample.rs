//! Build a coarse LOD node from the **real world's** surface heights
//! (Milestone 09, ADR-0008 criterion 4).
//!
//! The far LOD rings are sampled from the seed: cheap, and correct for terrain
//! that has never been generated. But the *innermost* ring abuts the
//! full-resolution region, and there an approximation is visible — the coarse
//! silhouette disagrees with the real ground it borders, so the boundary reads
//! as "different terrain" rather than the same hill at lower detail. Seed
//! sampling also cannot see **player edits**: a levelled hilltop or a dug
//! canyon would silently reappear at the LOD ring.
//!
//! So the near ring is built from the world's own column heightmap (which the
//! app already maintains for skylight, and which edits keep current), rather
//! than re-derived from the seed.
//!
//! ## Why heights and not blocks
//!
//! Reducing raw voxels would mean hundreds of thousands of block reads per
//! node — far too slow to sit anywhere near the frame path. The heightmap gives
//! the same surface for a few thousand cheap lookups, and at LOD distance the
//! surface *is* the terrain.
//!
//! ## Conservative, matching the seed path
//!
//! Each coarse cell takes the MINIMUM height over its footprint and is filled
//! by the same round-down rule, so coarse terrain never rises above real
//! terrain. LOD underlaps the full-resolution region; anything that overshoots
//! pokes through real ground and is solid to walk into.

use crate::chunk::Chunk;
use crate::coords::{CHUNK_SIZE, LocalPos, WorldPos};
use crate::registry::{GRASS, STONE};

/// Build a coarse LOD node from real surface heights.
///
/// `height_at(wx, wz)` returns the world's surface height for a block column,
/// or `None` if that column isn't known (not resident). Any unknown column in
/// the node's footprint returns `None` overall, and the caller falls back to
/// seed generation — which is what happens after a teleport, or beyond the
/// loaded region.
///
/// The result matches `vox_worldgen::generate_lod_node`'s layout exactly — a
/// 32³ `Chunk` with `h_stride`-wide, `v_stride`-tall cells and skylight baked
/// (air = 15) — so the two are interchangeable to the mesher and renderer.
pub fn downsample_node_from_heights(
    origin: WorldPos,
    h_stride: i64,
    v_stride: i64,
    height_at: impl Fn(i64, i64) -> Option<i64>,
) -> Option<Chunk> {
    debug_assert!(h_stride >= 1 && v_stride >= 1);
    let mut chunk = Chunk::new_air();
    for cz in 0..CHUNK_SIZE as i64 {
        for cx in 0..CHUNK_SIZE as i64 {
            let bx = origin.x + cx * h_stride;
            let bz = origin.z + cz * h_stride;
            // Minimum surface over the cell footprint: coarse terrain must sit
            // at or below the real surface everywhere in the cell.
            let mut height = i64::MAX;
            for dz in 0..h_stride {
                for dx in 0..h_stride {
                    let h = height_at(bx + dx, bz + dz)?;
                    if h < height {
                        height = h;
                    }
                }
            }
            for cy in 0..CHUNK_SIZE as i64 {
                let cell_top = origin.y + cy * v_stride + v_stride - 1;
                // Round down: solid only if the cube lies entirely at or below
                // the surface.
                let lp = LocalPos::new(cx as u8, cy as u8, cz as u8);
                if cell_top > height {
                    // Sky-exposed at LOD (heightfield, no caves).
                    chunk.set_sky_light(lp, 15);
                } else if cell_top + v_stride > height {
                    chunk.set(lp, GRASS);
                } else {
                    chunk.set(lp, STONE);
                }
            }
        }
    }
    chunk.mark_unmodified();
    Some(chunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Topmost solid cell in a coarse column, as a world Y.
    fn coarse_top(chunk: &Chunk, cx: u8, cz: u8, origin_y: i64, v: i64) -> Option<i64> {
        (0..CHUNK_SIZE as u8)
            .rev()
            .find(|&cy| !chunk.get(LocalPos::new(cx, cy, cz)).is_air())
            .map(|cy| origin_y + cy as i64 * v + v - 1)
    }

    /// An unknown column anywhere in the footprint aborts, so the caller can
    /// fall back to seed generation rather than punch a hole in the world.
    #[test]
    fn missing_height_returns_none() {
        let node = downsample_node_from_heights(WorldPos::new(0, -128, 0), 4, 8, |x, _z| {
            if x == 40 { None } else { Some(10) }
        });
        assert!(node.is_none());
    }

    /// THE criterion-4 contract: the coarse surface matches the real surface,
    /// never above it, never more than one cell below.
    #[test]
    fn surface_matches_the_real_world() {
        let (v, origin_y) = (8i64, -128i64);
        // A tilted plane, so every coarse column sees a different height.
        let real = |x: i64, z: i64| 20 + (x / 7) - (z / 11);
        let node = downsample_node_from_heights(WorldPos::new(0, origin_y, 0), 4, v, |x, z| {
            Some(real(x, z))
        })
        .unwrap();
        for cz in 0..CHUNK_SIZE as u8 {
            for cx in 0..CHUNK_SIZE as u8 {
                let top = coarse_top(&node, cx, cz, origin_y, v).expect("solid column");
                // Against every REAL column under this cell.
                for dz in 0..4 {
                    for dx in 0..4 {
                        let h = real(cx as i64 * 4 + dx, cz as i64 * 4 + dz);
                        assert!(top <= h, "coarse {top} above real {h}");
                        assert!(h - top < v + 4, "coarse {top} far below real {h}");
                    }
                }
            }
        }
    }

    /// Edits are reflected — the whole reason the near ring reduces real data
    /// instead of re-deriving terrain from the seed.
    #[test]
    fn reflects_edits() {
        let (v, origin_y) = (8i64, -128i64);
        let flat = 40i64;
        // A dug pit in one coarse cell's footprint.
        let with_pit = |x: i64, z: i64| {
            if (0..4).contains(&x) && (0..4).contains(&z) {
                8
            } else {
                flat
            }
        };
        let node = downsample_node_from_heights(WorldPos::new(0, origin_y, 0), 4, v, |x, z| {
            Some(with_pit(x, z))
        })
        .unwrap();
        let pit = coarse_top(&node, 0, 0, origin_y, v).unwrap();
        let plain = coarse_top(&node, 5, 5, origin_y, v).unwrap();
        assert!(
            pit < plain,
            "edit not reflected: pit {pit} vs plain {plain}"
        );
    }

    /// Air cells carry full skylight, so a downsampled node lights itself the
    /// same way a seed-generated one does (no separate lighting path).
    #[test]
    fn air_cells_get_full_skylight() {
        let node =
            downsample_node_from_heights(WorldPos::new(0, 0, 0), 2, 4, |_, _| Some(-1000)).unwrap();
        for p in LocalPos::iter() {
            assert!(node.get(p).is_air());
            assert_eq!(node.sky_light(p), 15);
        }
    }

    /// Terrain far above the node fills it solid, and the node is canonical
    /// state rather than a user edit.
    #[test]
    fn fully_buried_node_is_solid_and_unmodified() {
        let node = downsample_node_from_heights(WorldPos::new(0, 0, 0), 2, 4, |_, _| Some(100_000))
            .unwrap();
        assert!(LocalPos::iter().all(|p| !node.get(p).is_air()));
        assert!(!node.is_modified());
    }
}
