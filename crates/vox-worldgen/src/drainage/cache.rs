//! The drainage network's cache in the world folder (ADR-0019 §6).
//!
//! The network is a pure function of the seed, the world's size and the
//! generator version, so the file is only ever a shortcut: read when every
//! one of those matches and its checksum holds, rebuilt and rewritten when
//! anything does not. It is never the source of truth — a corrupt or stale
//! cache costs a rebuild, never wrong terrain.
//!
//! Layout, little-endian: the magic `VXDR`; [`FORMAT_VERSION`]; the generator
//! version, seed and size it was built for; the grid's and the erosion
//! grid's dimensions; the arrays; and last an FNV-1a checksum of everything
//! before it.

use vox_core::WorldShape;

use super::{DrainageMap, erosion::Erosion};

const MAGIC: &[u8; 4] = b"VXDR";
/// Bump when the layout changes. A file of another version is rebuilt.
pub const FORMAT_VERSION: u32 = 1;

/// What a cache must have been built for to be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheKey {
    pub seed: u64,
    pub shape: WorldShape,
    pub generator_version: u32,
}

impl DrainageMap {
    /// The network as cache-file bytes, for `key`.
    pub fn to_cache_bytes(&self, key: CacheKey) -> Vec<u8> {
        let n = self.nx * self.nz;
        let mut out = Vec::with_capacity(64 + n * 11 + self.erosion.delta.len() * 4);
        out.extend_from_slice(MAGIC);
        put_u32(&mut out, FORMAT_VERSION);
        put_u32(&mut out, key.generator_version);
        out.extend_from_slice(&key.seed.to_le_bytes());
        out.extend_from_slice(&key.shape.size_x().to_le_bytes());
        out.extend_from_slice(&key.shape.size_z().to_le_bytes());
        put_u32(&mut out, self.nx as u32);
        put_u32(&mut out, self.nz as u32);
        out.extend_from_slice(&self.cell_x.to_le_bytes());
        out.extend_from_slice(&self.cell_z.to_le_bytes());
        let e = &self.erosion;
        put_u32(&mut out, e.nx as u32);
        put_u32(&mut out, e.nz as u32);
        out.extend_from_slice(&e.cell_x.to_le_bytes());
        out.extend_from_slice(&e.cell_z.to_le_bytes());
        for &v in &self.height {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &v in &self.level {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.cover);
        out.extend_from_slice(&self.flow);
        for &v in &self.discharge {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &v in &e.delta {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let sum = fnv1a(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// The network from cache-file bytes, if they are whole, of this format,
    /// and built for `key`; `None` otherwise — rebuild.
    pub fn from_cache_bytes(bytes: &[u8], key: CacheKey) -> Option<Self> {
        let (body, sum) = bytes.split_at_checked(bytes.len().checked_sub(8)?)?;
        if fnv1a(body) != u64::from_le_bytes(sum.try_into().ok()?) {
            return None;
        }
        let mut r = Reader(body);
        if r.take(4)? != MAGIC
            || r.u32()? != FORMAT_VERSION
            || r.u32()? != key.generator_version
            || r.u64()? != key.seed
            || r.i64()? != key.shape.size_x()
            || r.i64()? != key.shape.size_z()
        {
            return None;
        }
        let (nx, nz) = (r.u32()? as usize, r.u32()? as usize);
        let (cell_x, cell_z) = (r.f64()?, r.f64()?);
        let (enx, enz) = (r.u32()? as usize, r.u32()? as usize);
        let (ecell_x, ecell_z) = (r.f64()?, r.f64()?);
        // The grid must be the one this world's network has: anything else
        // would index out of bounds, so it is a stale or foreign file.
        if (nx, nz) != super::grid(key.shape) || (enx, enz) != (nx / 2, nz / 2) {
            return None;
        }
        let n = nx * nz;
        let en = enx * enz;
        let height = r.array(n, |b| i16::from_le_bytes([b[0], b[1]]), 2)?;
        let level = r.array(n, |b| i16::from_le_bytes([b[0], b[1]]), 2)?;
        let cover = r.take(n)?.to_vec();
        let flow = r.take(n)?.to_vec();
        let discharge = r.array(n, |b| f32::from_le_bytes(b.try_into().unwrap()), 4)?;
        let delta = r.array(en, |b| f32::from_le_bytes(b.try_into().unwrap()), 4)?;
        if !r.0.is_empty() || flow.iter().any(|&d| d > super::HERE) {
            return None;
        }
        Some(Self {
            id: super::next_id(),
            nx,
            nz,
            cell_x,
            cell_z,
            height,
            level,
            cover,
            flow,
            discharge,
            erosion: Erosion {
                nx: enx,
                nz: enz,
                cell_x: ecell_x,
                cell_z: ecell_z,
                delta,
            },
            #[cfg(test)]
            budget: (Vec::new(), Vec::new()),
        })
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// FNV-1a, 64-bit: enough to tell a truncated or damaged file.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, k: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(k)?;
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn array<T>(&mut self, n: usize, f: impl Fn(&[u8]) -> T, width: usize) -> Option<Vec<T>> {
        let bytes = self.take(n.checked_mul(width)?)?;
        Some(bytes.chunks_exact(width).map(f).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::ClimateMap;
    use crate::elevation::Elevation;

    fn world() -> (DrainageMap, CacheKey) {
        let shape = WorldShape::new(32_768, 32_768).unwrap();
        let e = Elevation::new(77, shape);
        let c = ClimateMap::build(&e, shape);
        let key = CacheKey {
            seed: 77,
            shape,
            generator_version: crate::GENERATOR_VERSION,
        };
        (DrainageMap::build(&e, &c, shape), key)
    }

    /// The cache reads back exactly what was written.
    #[test]
    fn the_cache_round_trips() {
        let (d, key) = world();
        let back = DrainageMap::from_cache_bytes(&d.to_cache_bytes(key), key).unwrap();
        assert_eq!((back.nx, back.nz), (d.nx, d.nz));
        assert_eq!(back.height, d.height);
        assert_eq!(back.level, d.level);
        assert_eq!(back.cover, d.cover);
        assert_eq!(back.flow, d.flow);
        assert!(
            back.discharge
                .iter()
                .zip(&d.discharge)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert!(
            back.erosion
                .delta
                .iter()
                .zip(&d.erosion.delta)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        for (x, z) in [(0.0, 0.0), (1_234.5, 9_876.0), (-50_000.0, 70_000.0)] {
            assert_eq!(
                back.erosion_at(x, z).to_bits(),
                d.erosion_at(x, z).to_bits()
            );
        }
    }

    /// Anything else is rebuilt: another world, another generator, another
    /// format, a damaged or truncated file.
    #[test]
    fn a_stale_or_damaged_cache_is_refused() {
        let (d, key) = world();
        let bytes = d.to_cache_bytes(key);
        let other = |f: fn(&mut CacheKey)| {
            let mut k = key;
            f(&mut k);
            DrainageMap::from_cache_bytes(&bytes, k).is_none()
        };
        assert!(other(|k| k.seed += 1), "another seed");
        assert!(other(|k| k.generator_version += 1), "another generator");
        assert!(
            other(|k| k.shape = WorldShape::new(32_768, 40_960).unwrap()),
            "another size"
        );
        let mut damaged = bytes.clone();
        damaged[bytes.len() / 2] ^= 0x10;
        assert!(
            DrainageMap::from_cache_bytes(&damaged, key).is_none(),
            "damaged"
        );
        for cut in [0, 7, 40, bytes.len() - 1] {
            assert!(
                DrainageMap::from_cache_bytes(&bytes[..cut], key).is_none(),
                "cut at {cut}"
            );
        }
        let mut old = bytes.clone();
        old[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(
            DrainageMap::from_cache_bytes(&old, key).is_none(),
            "another format"
        );
    }
}
