//! On-disk world storage (Milestone 02 task 5).
//!
//! A world lives in a directory:
//!
//! ```text
//! <world>/
//!   world.meta            versioned metadata (magic, version, seed)
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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::{Chunk, ChunkDecodeError};
use crate::coords::ChunkPos;

const META_MAGIC: [u8; 4] = *b"VXTW";
/// World-metadata format version. Independent of the chunk format version.
pub const WORLD_META_VERSION: u8 = 1;

/// Handle to a world directory on disk. Cheap to clone (just a path).
#[derive(Clone, Debug)]
pub struct WorldStore {
    root: PathBuf,
    chunks_dir: PathBuf,
    seed: u64,
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
    /// Open (or create) a world at `root` with the given `seed`.
    ///
    /// - If no metadata file exists, the directory structure is created and
    ///   metadata is written with `seed`.
    /// - If metadata exists, the stored seed is used and the passed `seed`
    ///   is ignored (the saved world's seed is authoritative). Use
    ///   [`WorldStore::seed`] to read it back.
    pub fn open(root: impl AsRef<Path>, seed: u64) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        let chunks_dir = root.join("chunks");
        let meta_path = root.join("world.meta");

        let resolved_seed = if meta_path.exists() {
            read_meta(&meta_path)?
        } else {
            fs::create_dir_all(&chunks_dir)?;
            write_meta(&meta_path, seed)?;
            seed
        };

        Ok(Self {
            root,
            chunks_dir,
            seed: resolved_seed,
        })
    }

    /// The world's seed (authoritative once a world has been created).
    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn chunk_path(&self, pos: ChunkPos) -> PathBuf {
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

fn write_meta(path: &Path, seed: u64) -> Result<(), StoreError> {
    let mut bytes = Vec::with_capacity(13);
    bytes.extend_from_slice(&META_MAGIC);
    bytes.push(WORLD_META_VERSION);
    bytes.extend_from_slice(&seed.to_le_bytes());
    fs::write(path, &bytes)?;
    Ok(())
}

fn read_meta(path: &Path) -> Result<u64, StoreError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 5 {
        return Err(StoreError::BadMeta);
    }
    if bytes[0..4] != META_MAGIC {
        return Err(StoreError::BadMetaMagic);
    }
    let version = bytes[4];
    if version != WORLD_META_VERSION {
        return Err(StoreError::UnsupportedMetaVersion(version));
    }
    let seed_bytes = bytes.get(5..13).ok_or(StoreError::BadMeta)?;
    Ok(u64::from_le_bytes(seed_bytes.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlockId;
    use crate::coords::LocalPos;

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
        let store = WorldStore::open(&dir.0, 0xABCD).unwrap();
        assert_eq!(store.seed(), 0xABCD);
        assert!(dir.0.join("world.meta").exists());
        assert!(dir.0.join("chunks").is_dir());

        // Reopening reads the stored seed, ignoring the passed one.
        let store2 = WorldStore::open(&dir.0, 0x9999).unwrap();
        assert_eq!(store2.seed(), 0xABCD, "stored seed must be authoritative");
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = TempDir::new("roundtrip");
        let store = WorldStore::open(&dir.0, 1).unwrap();
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
        let store = WorldStore::open(&dir.0, 1).unwrap();
        assert!(store.load_chunk(ChunkPos::new(0, 0, 0)).unwrap().is_none());
    }

    #[test]
    fn save_overwrites() {
        let dir = TempDir::new("overwrite");
        let store = WorldStore::open(&dir.0, 1).unwrap();
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
        let store = WorldStore::open(&dir.0, 1).unwrap();
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
            WorldStore::open(&dir.0, 1),
            Err(StoreError::BadMetaMagic)
        ));
    }
}
