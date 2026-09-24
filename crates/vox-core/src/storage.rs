//! On-disk world storage (Milestone 02 task 5).
//!
//! A world lives in a directory:
//!
//! ```text
//! <world>/
//!   world.meta            versioned metadata (magic, version, seed, world size,
//!                         generator version)
//!   chunks/
//!     c.<x>.<y>.<z>.vxc    one file per modified chunk
//! ```
//!
//! Only **modified** chunks are written; unmodified chunks regenerate from
//! the seed for free (see [`Chunk::is_modified`](crate::Chunk::is_modified)),
//! so the directory stays small. Chunks read from disk are marked modified
//! (their presence on disk means they were edited), so they keep persisting.
//!
//! This is a deliberately simple one-file-per-chunk scheme. Region-file
//! packing and compression are deferred (Milestone 02 spec non-goals); if
//! this grows, it can graduate to a dedicated `vox-io` crate. [`WorldStore`]
//! is `Clone` and its methods take `&self`, so it can be used from worker
//! threads (e.g. async generate-or-load).
//!
//! ## Chunks are filed under their CANONICAL position (ADR-0012)
//!
//! The world is a torus, and the player's coordinates never wrap: on a second
//! lap of the world a chunk that was saved at x = 5 is requested at x = 5 plus
//! one world width. The save layer is one of the three places that seam
//! exists, so every chunk path is built from the canonical position. Callers
//! pass unwrapped positions freely and never canonicalise themselves.
//!
//! ## A world is its seed, its size AND its generator
//!
//! `world.meta` records all three. Without the size, the world's period is
//! unknown. Without the generator version, a save made by an older terrain
//! algorithm loads its edited chunks back as islands of the old landscape in
//! the middle of the new one — which is what happened to every world saved
//! before M10.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::{Chunk, ChunkDecodeError};
use crate::coords::ChunkPos;
use crate::planet::{WorldShape, WorldShapeError};

const META_MAGIC: [u8; 4] = *b"VXTW";
/// World-metadata format version. Independent of the chunk format version.
///
/// Version 1 held only the seed. Version 2 adds the world's size and the
/// version of the generator that made it (M10, ADR-0012).
pub const WORLD_META_VERSION: u8 = 2;

/// Byte length of a version-2 metadata file: magic, version, seed, two sizes,
/// generator version.
const META_V2_LEN: usize = 4 + 1 + 8 + 8 + 8 + 4;

/// Handle to a world directory on disk. Cheap to clone (just a path).
#[derive(Clone, Debug)]
pub struct WorldStore {
    root: PathBuf,
    chunks_dir: PathBuf,
    meta: WorldMeta,
}

/// What `world.meta` records about a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldMeta {
    pub seed: u64,
    pub shape: WorldShape,
    /// The terrain generator's version when the world was created. See
    /// `vox_worldgen::GENERATOR_VERSION`.
    pub generator_version: u32,
}

/// Why opening or using a world store failed.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// Metadata file present but not a Voxterra world.
    BadMetaMagic,
    /// Metadata version not understood by this build.
    UnsupportedMetaVersion(u8),
    /// Metadata file truncated/corrupt.
    BadMeta,
    /// A version-1 world, from before worlds recorded their size and
    /// generator. Its edited chunks were made by a terrain algorithm that no
    /// longer exists, so it cannot be opened faithfully.
    LegacyWorld,
    /// The world was made by a different terrain generator than this build's.
    /// Opening it would stitch old edited chunks into new terrain.
    GeneratorMismatch {
        saved: u32,
        current: u32,
    },
    /// The recorded world size is not a legal size.
    BadShape(WorldShapeError),
    /// A chunk file failed to decode.
    Chunk(ChunkDecodeError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::BadMetaMagic => write!(f, "not a Voxterra world (bad metadata magic)"),
            Self::UnsupportedMetaVersion(v) => write!(f, "unsupported world metadata version {v}"),
            Self::BadMeta => write!(f, "corrupt world metadata"),
            Self::LegacyWorld => write!(
                f,
                "this world was created before worlds recorded their size and terrain \
                 generator, and cannot be opened by this build; move or delete it to \
                 start a new one"
            ),
            Self::GeneratorMismatch { saved, current } => write!(
                f,
                "this world was made by terrain generator v{saved}, but this build \
                 generates v{current}; opening it would mix old and new terrain"
            ),
            Self::BadShape(e) => write!(f, "corrupt world metadata: {e}"),
            Self::Chunk(e) => write!(f, "chunk decode error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl WorldStore {
    /// Open (or create) a world at `root`.
    ///
    /// - If no metadata file exists, the directory structure is created and
    ///   metadata is written from `new_world` — the seed, size and generator
    ///   version a NEW world should have.
    /// - If metadata exists, what it records is authoritative: the stored seed
    ///   and size are used and `new_world`'s are ignored. Read them back with
    ///   [`WorldStore::meta`]. The stored generator version must equal
    ///   `new_world.generator_version`, which callers set to the current
    ///   build's generator; a mismatch is refused rather than opened wrongly.
    pub fn open(root: impl AsRef<Path>, new_world: WorldMeta) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        let chunks_dir = root.join("chunks");
        let meta_path = root.join("world.meta");

        let meta = if meta_path.exists() {
            let saved = read_meta(&meta_path)?;
            if saved.generator_version != new_world.generator_version {
                return Err(StoreError::GeneratorMismatch {
                    saved: saved.generator_version,
                    current: new_world.generator_version,
                });
            }
            saved
        } else {
            fs::create_dir_all(&chunks_dir)?;
            write_meta(&meta_path, &new_world)?;
            new_world
        };

        Ok(Self {
            root,
            chunks_dir,
            meta,
        })
    }

    /// Everything `world.meta` records (authoritative once a world exists).
    pub fn meta(&self) -> WorldMeta {
        self.meta
    }

    /// The world's seed (authoritative once a world has been created).
    pub fn seed(&self) -> u64 {
        self.meta.seed
    }

    /// The world's size (authoritative once a world has been created).
    pub fn shape(&self) -> WorldShape {
        self.meta.shape
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file for a chunk, filed under its CANONICAL position so a chunk
    /// saved on one lap of the world is found on every other (ADR-0012).
    fn chunk_path(&self, pos: ChunkPos) -> PathBuf {
        let pos = self.meta.shape.canonical_chunk(pos);
        self.chunks_dir
            .join(format!("c.{}.{}.{}.vxc", pos.x, pos.y, pos.z))
    }

    /// Persist a chunk to disk. Caller decides *whether* to save (e.g. only
    /// when `chunk.is_modified()`); this always writes when called.
    pub fn save_chunk(&self, pos: ChunkPos, chunk: &Chunk) -> Result<(), StoreError> {
        let bytes = chunk.serialize();
        // Write to a temp file then rename, so a crash mid-write can't leave
        // a half-written chunk that fails to decode.
        let final_path = self.chunk_path(pos);
        let tmp_path = final_path.with_extension("vxc.tmp");
        fs::write(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }

    /// Load a chunk from disk if one was saved for `pos`. The returned chunk
    /// is marked modified (its presence on disk means it must keep
    /// persisting). Returns `Ok(None)` if no file exists.
    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        let path = self.chunk_path(pos);
        match fs::read(&path) {
            Ok(bytes) => {
                let mut chunk = Chunk::deserialize(&bytes).map_err(StoreError::Chunk)?;
                chunk.mark_modified();
                Ok(Some(chunk))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    /// Whether a saved chunk file exists for `pos` (without reading it).
    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        self.chunk_path(pos).exists()
    }
}

fn write_meta(path: &Path, meta: &WorldMeta) -> Result<(), StoreError> {
    let mut bytes = Vec::with_capacity(META_V2_LEN);
    bytes.extend_from_slice(&META_MAGIC);
    bytes.push(WORLD_META_VERSION);
    bytes.extend_from_slice(&meta.seed.to_le_bytes());
    bytes.extend_from_slice(&meta.shape.size_x().to_le_bytes());
    bytes.extend_from_slice(&meta.shape.size_z().to_le_bytes());
    bytes.extend_from_slice(&meta.generator_version.to_le_bytes());
    debug_assert_eq!(bytes.len(), META_V2_LEN);
    fs::write(path, &bytes)?;
    Ok(())
}

fn read_meta(path: &Path) -> Result<WorldMeta, StoreError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 5 {
        return Err(StoreError::BadMeta);
    }
    if bytes[0..4] != META_MAGIC {
        return Err(StoreError::BadMetaMagic);
    }
    match bytes[4] {
        1 => return Err(StoreError::LegacyWorld),
        WORLD_META_VERSION => {}
        v => return Err(StoreError::UnsupportedMetaVersion(v)),
    }
    if bytes.len() != META_V2_LEN {
        return Err(StoreError::BadMeta);
    }
    let u64_at = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().expect("8 bytes"));
    let i64_at = |i: usize| i64::from_le_bytes(bytes[i..i + 8].try_into().expect("8 bytes"));
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
    let shape = WorldShape::new(i64_at(13), i64_at(21)).map_err(StoreError::BadShape)?;
    Ok(WorldMeta {
        seed: u64_at(5),
        shape,
        generator_version: u32_at(29),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlockId;
    use crate::coords::LocalPos;

    /// Metadata for a new default-size world with the given seed.
    fn new_world(seed: u64) -> WorldMeta {
        WorldMeta {
            seed,
            shape: WorldShape::DEFAULT,
            generator_version: 7,
        }
    }

    /// Unique temp dir per test, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            p.push(format!("voxterra_test_{tag}_{nanos}"));
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn open_creates_world_and_persists_seed() {
        let dir = TempDir::new("create");
        let store = WorldStore::open(&dir.0, new_world(0xABCD)).unwrap();
        assert_eq!(store.seed(), 0xABCD);
        assert!(dir.0.join("world.meta").exists());
        assert!(dir.0.join("chunks").is_dir());

        // Reopening reads the stored seed, ignoring the passed one.
        let store2 = WorldStore::open(&dir.0, new_world(0x9999)).unwrap();
        assert_eq!(store2.seed(), 0xABCD, "stored seed must be authoritative");
    }

    /// Size and generator version survive a round trip, and an existing
    /// world's size wins over whatever a new world would have been given.
    #[test]
    fn world_size_and_generator_are_recorded_and_authoritative() {
        let dir = TempDir::new("shape");
        let small = WorldShape::new(32_768, 65_536).unwrap();
        let made = WorldMeta {
            seed: 3,
            shape: small,
            generator_version: 7,
        };
        WorldStore::open(&dir.0, made).unwrap();
        let reopened = WorldStore::open(&dir.0, new_world(99)).unwrap();
        assert_eq!(reopened.meta(), made);
        assert_eq!(reopened.shape(), small);
    }

    /// A world made by a different terrain generator is refused, not opened
    /// with its edited chunks stranded in new terrain.
    #[test]
    fn a_world_from_another_generator_is_refused() {
        let dir = TempDir::new("genver");
        WorldStore::open(&dir.0, new_world(1)).unwrap();
        let mut newer = new_world(1);
        newer.generator_version = 8;
        assert!(matches!(
            WorldStore::open(&dir.0, newer),
            Err(StoreError::GeneratorMismatch {
                saved: 7,
                current: 8
            })
        ));
    }

    /// Worlds saved before M10 carry version-1 metadata: no size, no generator.
    #[test]
    fn a_pre_m10_world_is_refused_with_a_clear_reason() {
        let dir = TempDir::new("legacy");
        fs::create_dir_all(&dir.0).unwrap();
        let mut v1 = b"VXTW".to_vec();
        v1.push(1);
        v1.extend_from_slice(&42u64.to_le_bytes());
        fs::write(dir.0.join("world.meta"), &v1).unwrap();
        let err = WorldStore::open(&dir.0, new_world(1)).unwrap_err();
        assert!(matches!(err, StoreError::LegacyWorld));
        assert!(err.to_string().contains("move or delete"));
    }

    /// THE seam test for the save layer. A chunk edited on one lap of the world
    /// must be found on every other: the player's coordinates never wrap, so
    /// the same place is requested at a different unwrapped position each lap.
    #[test]
    fn a_chunk_saved_on_one_lap_is_found_on_every_other() {
        let dir = TempDir::new("laps");
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        let lap = WorldShape::DEFAULT.size_x() / crate::coords::CHUNK_SIZE as i64;
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(4, 5, 6), BlockId(3));
        // Saved just west of the seam, through a negative unwrapped position.
        store.save_chunk(ChunkPos::new(-1, 2, -3), &chunk).unwrap();
        for pos in [
            ChunkPos::new(-1, 2, -3),
            ChunkPos::new(lap - 1, 2, lap - 3),
            ChunkPos::new(-1 + 5 * lap, 2, -3 - 2 * lap),
        ] {
            assert!(store.has_chunk(pos), "not found at {pos:?}");
            let loaded = store.load_chunk(pos).unwrap().expect("exists");
            assert_eq!(loaded.get(LocalPos::new(4, 5, 6)), BlockId(3));
        }
        // Y does not wrap: the same X/Z one layer up is a different chunk.
        assert!(!store.has_chunk(ChunkPos::new(-1, 3, -3)));
        // One place, one file.
        let files = fs::read_dir(dir.0.join("chunks")).unwrap().count();
        assert_eq!(files, 1);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = TempDir::new("roundtrip");
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        let pos = ChunkPos::new(-3, 5, 7);

        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(1, 2, 3), BlockId(1));
        chunk.set(LocalPos::new(30, 0, 30), BlockId(2));

        assert!(!store.has_chunk(pos));
        store.save_chunk(pos, &chunk).unwrap();
        assert!(store.has_chunk(pos));

        let loaded = store.load_chunk(pos).unwrap().expect("should exist");
        for p in LocalPos::iter() {
            assert_eq!(loaded.get(p), chunk.get(p));
        }
        // Loaded chunk is marked modified so it keeps persisting.
        assert!(loaded.is_modified());
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = TempDir::new("missing");
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        assert!(store.load_chunk(ChunkPos::new(0, 0, 0)).unwrap().is_none());
    }

    #[test]
    fn save_overwrites() {
        let dir = TempDir::new("overwrite");
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        let pos = ChunkPos::new(0, 0, 0);

        let mut a = Chunk::new_air();
        a.set(LocalPos::new(0, 0, 0), BlockId(1));
        store.save_chunk(pos, &a).unwrap();

        let mut b = Chunk::new_air();
        b.set(LocalPos::new(5, 5, 5), BlockId(9));
        store.save_chunk(pos, &b).unwrap();

        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(loaded.get(LocalPos::new(5, 5, 5)), BlockId(9));
        assert_eq!(loaded.get(LocalPos::new(0, 0, 0)), BlockId::AIR);
    }

    #[test]
    fn negative_coordinate_chunks_save_and_load() {
        let dir = TempDir::new("negcoord");
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        let pos = ChunkPos::new(-1_000_000, -42, 1_000_000);
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(7, 7, 7), BlockId(3));
        store.save_chunk(pos, &chunk).unwrap();
        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(loaded.get(LocalPos::new(7, 7, 7)), BlockId(3));
    }

    #[test]
    fn rejects_foreign_metadata() {
        let dir = TempDir::new("foreign");
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(dir.0.join("world.meta"), b"NOPExxxxxxxxx").unwrap();
        assert!(matches!(
            WorldStore::open(&dir.0, new_world(1)),
            Err(StoreError::BadMetaMagic)
        ));
    }
}
