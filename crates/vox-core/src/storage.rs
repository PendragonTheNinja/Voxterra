//! On-disk world storage (Milestone 02 task 5).
//!
//! A world lives in a directory:
//!
//! ```text
//! <world>/
//!   world.meta            versioned metadata (magic, version, seed, world size,
//!                         generator version)
//!   blocks.table          the world's own block ids, by key (ADR-0024)
//!   lod_edits.bin         the LOD's edit overlay (versioned; absent = none)
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
//!
//! ## The LOD edit overlay is saved too (M10 A3)
//!
//! Distant terrain is drawn from the seed, lowered wherever the player dug
//! ([`EditedColumns`]). The chunks on disk hold the edits themselves, but the
//! LOD never reads chunks — so without the overlay, every earlier session's
//! digs come back as ghost terrain at distance. It is saved alongside the
//! chunks, whenever they are, and a world with no overlay file simply has no
//! recorded edits.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkDecodeError};
use crate::coords::ChunkPos;
use crate::lod::{EditedColumns, EditsDecodeError};
use crate::planet::{WorldShape, WorldShapeError};
use crate::registry::{BlockRegistry, M14_BLOCK_KEYS};

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
    /// How this world numbers its blocks, against how this build does.
    blocks: Arc<BlockTable>,
}

const BLOCKS_MAGIC: [u8; 4] = *b"VXTB";
/// Block-table format version (`blocks.table`).
pub const BLOCK_TABLE_VERSION: u8 = 1;

/// A world's block ids and this build's, each way (ADR-0024).
///
/// A saved chunk stores numbers. Which block a number means is the WORLD's
/// business, recorded once in `blocks.table` as a list of permanent keys;
/// this build's registry may list its blocks in another order, or have
/// more of them. Chunks are translated here, at the save layer, and
/// nothing above it ever sees a world id.
#[derive(Debug)]
struct BlockTable {
    /// World id → this build's id.
    to_build: Vec<BlockId>,
    /// This build's id → world id.
    to_world: Vec<BlockId>,
    /// The two numberings are the same: no chunk needs translating. True
    /// for every world until the registry is first reordered.
    identity: bool,
}

impl BlockTable {
    /// Reconcile a world's keys (in world-id order) with a build's
    /// registry. Returns the table and the world's keys as they must now
    /// be saved: the same, with any block the build has and the world
    /// lacked appended. An id, once given to a key, is never moved.
    fn reconcile(
        mut keys: Vec<String>,
        registry: &BlockRegistry,
    ) -> Result<(Self, Vec<String>), StoreError> {
        if keys.first().map(String::as_str) != Some("air") {
            return Err(StoreError::BadBlockTable); // id 0 is air, always
        }
        let mut to_build = Vec::with_capacity(keys.len());
        let mut unknown = Vec::new();
        for key in &keys {
            match registry.id_of_key(key) {
                Some(id) => to_build.push(id),
                None => unknown.push(key.clone()),
            }
        }
        if !unknown.is_empty() {
            return Err(StoreError::UnknownBlocks(unknown));
        }
        // Two world keys naming one block (a key and its alias both in an
        // old table) would make saving ambiguous; refuse rather than guess.
        let mut to_world = vec![None; registry.len()];
        for (world, build) in to_build.iter().enumerate() {
            let slot = &mut to_world[build.0 as usize];
            if slot.is_some() {
                return Err(StoreError::BadBlockTable);
            }
            *slot = Some(BlockId(world as u16));
        }
        // Blocks this build has and the world has not met: next free ids.
        for (build, slot) in to_world.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(BlockId(keys.len() as u16));
                to_build.push(BlockId(build as u16));
                keys.push(registry.key(BlockId(build as u16)).to_string());
            }
        }
        let to_world: Vec<BlockId> = to_world.into_iter().flatten().collect();
        let identity = to_build.iter().enumerate().all(|(i, b)| b.0 as usize == i);
        Ok((
            Self {
                to_build,
                to_world,
                identity,
            },
            keys,
        ))
    }
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
    /// `blocks.table` is not one, is truncated, or does not start with air.
    BadBlockTable,
    /// The world uses blocks this build does not have (their keys): it was
    /// saved by a newer build, and this one must not save over them.
    UnknownBlocks(Vec<String>),
    /// A chunk holds a block id its world's table has no entry for.
    UnknownBlockId(u16),
    /// A chunk file failed to decode.
    Chunk(ChunkDecodeError),
    /// The LOD edit overlay failed to decode.
    Edits(EditsDecodeError),
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
            Self::BadBlockTable => write!(f, "corrupt block table (blocks.table)"),
            Self::UnknownBlocks(keys) => write!(
                f,
                "this world uses blocks this build does not have ({}); it was saved by a \
                 newer version of the game",
                keys.join(", ")
            ),
            Self::UnknownBlockId(id) => {
                write!(
                    f,
                    "a chunk holds block id {id}, which its world's table lacks"
                )
            }
            Self::Chunk(e) => write!(f, "chunk decode error: {e}"),
            Self::Edits(e) => write!(f, "LOD edit overlay: {e}"),
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
        Self::open_with_blocks(root, new_world, &BlockRegistry::default_set())
    }

    /// [`WorldStore::open`] against a given block registry: the world's
    /// block table is reconciled with it (ADR-0024).
    ///
    /// - A new world records the registry's keys, in its order.
    /// - A world with a table keeps its own numbering; chunks are
    ///   translated on the way in and out. Blocks the registry has gained
    ///   since are appended to the table, and the file rewritten, before
    ///   any chunk can be saved with them.
    /// - A world with no table predates them: its ids are M14's
    ///   ([`M14_BLOCK_KEYS`]), and it is given the file now.
    /// - A world using a block the registry lacks is refused, by key.
    pub fn open_with_blocks(
        root: impl AsRef<Path>,
        new_world: WorldMeta,
        registry: &BlockRegistry,
    ) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        let chunks_dir = root.join("chunks");
        let meta_path = root.join("world.meta");
        let existed = meta_path.exists();

        let meta = if existed {
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

        let table_path = root.join("blocks.table");
        let saved: Option<Vec<String>> = match fs::read(&table_path) {
            Ok(bytes) => Some(decode_block_table(&bytes)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(StoreError::Io(e)),
        };
        let keys = match &saved {
            Some(keys) => keys.clone(),
            // No table: a world from before ADR-0024 holds M14's ids; a
            // brand-new one takes this build's.
            None if existed => M14_BLOCK_KEYS.iter().map(|k| k.to_string()).collect(),
            None => registry.keys().map(str::to_string).collect(),
        };
        let (blocks, keys) = BlockTable::reconcile(keys, registry)?;
        if saved.as_ref() != Some(&keys) {
            let tmp = table_path.with_extension("table.tmp");
            fs::write(&tmp, encode_block_table(&keys))?;
            fs::rename(&tmp, &table_path)?;
        }
        Ok(Self {
            root,
            chunks_dir,
            meta,
            blocks: Arc::new(blocks),
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
        // In the world's numbering (ADR-0024); the usual case is identity.
        let bytes = if self.blocks.identity {
            chunk.serialize()
        } else {
            // Every id a chunk can hold is the registry's, and the table
            // covers the whole registry; an id beyond it reads as air does.
            chunk.serialize_mapped(|b| {
                self.blocks
                    .to_world
                    .get(b.0 as usize)
                    .copied()
                    .unwrap_or(BlockId::AIR)
            })
        };
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
                // From the world's numbering to this build's (ADR-0024).
                // When they are the same the palette is left alone, but an
                // id the table has no entry for is still refused.
                let table = &self.blocks;
                chunk
                    .remap_blocks(|b| match table.to_build.get(b.0 as usize) {
                        Some(_) if table.identity => Some(b),
                        other => other.copied(),
                    })
                    .map_err(|b| StoreError::UnknownBlockId(b.0))?;
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

    fn edits_path(&self) -> PathBuf {
        self.root.join("lod_edits.bin")
    }

    /// Persist the LOD edit overlay, replacing any saved one. Written via a
    /// temp file and rename, like chunks, so a crash mid-write leaves the
    /// previous overlay intact rather than a truncated one.
    ///
    /// Panics if the overlay was built for a different world size: that is a
    /// programming error, and saving it would misplace every edit.
    pub fn save_edited_columns(&self, edits: &EditedColumns) -> Result<(), StoreError> {
        assert_eq!(
            edits.shape(),
            self.meta.shape,
            "edit overlay built for a different world size"
        );
        let final_path = self.edits_path();
        let tmp_path = final_path.with_extension("bin.tmp");
        fs::write(&tmp_path, edits.serialize())?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    }

    /// Load the LOD edit overlay. A world with no overlay file has no recorded
    /// edits and gets an empty one — true of every new world, and of worlds
    /// saved before the overlay was persisted.
    pub fn load_edited_columns(&self) -> Result<EditedColumns, StoreError> {
        match fs::read(self.edits_path()) {
            Ok(bytes) => {
                EditedColumns::deserialize(&bytes, self.meta.shape).map_err(StoreError::Edits)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(EditedColumns::new(self.meta.shape))
            }
            Err(e) => Err(StoreError::Io(e)),
        }
    }
}

/// `blocks.table`: magic, version, entry count (u16), then each key in
/// world-id order as a length byte and that many ASCII bytes.
fn encode_block_table(keys: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&BLOCKS_MAGIC);
    out.push(BLOCK_TABLE_VERSION);
    out.extend_from_slice(&(keys.len() as u16).to_le_bytes());
    for key in keys {
        out.push(key.len() as u8);
        out.extend_from_slice(key.as_bytes());
    }
    out
}

fn decode_block_table(bytes: &[u8]) -> Result<Vec<String>, StoreError> {
    let bad = || StoreError::BadBlockTable;
    if bytes.len() < 7 || bytes[0..4] != BLOCKS_MAGIC || bytes[4] != BLOCK_TABLE_VERSION {
        return Err(bad());
    }
    let count = u16::from_le_bytes([bytes[5], bytes[6]]) as usize;
    let mut at = 7;
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let len = *bytes.get(at).ok_or_else(bad)? as usize;
        let key = bytes.get(at + 1..at + 1 + len).ok_or_else(bad)?;
        if len == 0 || !key.is_ascii() {
            return Err(bad());
        }
        keys.push(String::from_utf8(key.to_vec()).map_err(|_| bad())?);
        at += 1 + len;
    }
    if at != bytes.len() || keys.is_empty() {
        return Err(bad());
    }
    Ok(keys)
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

    // ---- M10 A3: the LOD edit overlay persists with the world ----

    /// THE bug: the overlay lived only in memory, so reopening a world forgot
    /// every earlier session's digs and the LOD rebuilt ghost terrain over
    /// them. It must come back exactly as saved.
    #[test]
    fn edit_overlay_survives_reopening_the_world() {
        let dir = TempDir::new("edits_reopen");
        let store = WorldStore::open(&dir.0, new_world(7)).unwrap();
        let mut edits = EditedColumns::new(store.shape());
        edits.record(12, -30, -5);
        edits.record(700, 4, 18);
        store.save_edited_columns(&edits).unwrap();
        drop(store);

        let reopened = WorldStore::open(&dir.0, new_world(7)).unwrap();
        let back = reopened.load_edited_columns().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.get(12, -30), Some(-5));
        assert_eq!(back.get(700, 4), Some(18));
    }

    /// New worlds, and worlds saved before the overlay existed, have no file:
    /// that is "no recorded edits", not an error.
    #[test]
    fn a_world_without_an_overlay_file_has_no_edits() {
        let dir = TempDir::new("edits_missing");
        let store = WorldStore::open(&dir.0, new_world(7)).unwrap();
        let edits = store.load_edited_columns().unwrap();
        assert!(edits.is_empty());
        assert_eq!(edits.shape(), store.shape());
    }

    #[test]
    fn saving_the_overlay_replaces_the_previous_one() {
        let dir = TempDir::new("edits_replace");
        let store = WorldStore::open(&dir.0, new_world(7)).unwrap();
        let mut edits = EditedColumns::new(store.shape());
        edits.record(1, 1, 10);
        store.save_edited_columns(&edits).unwrap();
        edits.record(1, 1, 3);
        edits.record(2, 2, 4);
        store.save_edited_columns(&edits).unwrap();
        let back = store.load_edited_columns().unwrap();
        assert_eq!(back.get(1, 1), Some(3));
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn a_corrupt_overlay_is_reported_not_ignored() {
        let dir = TempDir::new("edits_corrupt");
        let store = WorldStore::open(&dir.0, new_world(7)).unwrap();
        fs::write(dir.0.join("lod_edits.bin"), b"junk").unwrap();
        assert!(matches!(
            store.load_edited_columns(),
            Err(StoreError::Edits(EditsDecodeError::Truncated))
        ));
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

    // ---- A world remembers its blocks by name (ADR-0024) ----

    /// A chunk holding one of each of `registry`'s first `n` blocks, along
    /// a row, and what each cell's key is.
    fn sampler(registry: &BlockRegistry, n: usize) -> (Chunk, Vec<&'static str>) {
        let mut chunk = Chunk::new_air();
        let mut keys = Vec::new();
        for i in 0..n {
            chunk.set(LocalPos::new(i as u8, 3, 7), BlockId(i as u16));
            keys.push(registry.key(BlockId(i as u16)));
        }
        (chunk, keys)
    }

    /// The keys read back along the sampler's row, under `registry`.
    fn keys_in(chunk: &Chunk, registry: &BlockRegistry, n: usize) -> Vec<&'static str> {
        (0..n)
            .map(|i| registry.key(chunk.get(LocalPos::new(i as u8, 3, 7))))
            .collect()
    }

    /// The starter set in another order: air first (it must be), then the
    /// rest reversed — every other block has a different id.
    fn reordered() -> BlockRegistry {
        let mut types = BlockRegistry::default_set().types();
        types[1..].reverse();
        BlockRegistry::from_types(types)
    }

    /// The whole point: the registry is reordered between a save and a
    /// load, and every block is still the block it was.
    #[test]
    fn a_reordered_registry_loads_every_block_as_itself() {
        let dir = TempDir::new("blocks_reorder");
        let was = BlockRegistry::default_set();
        let n = was.len();
        let (chunk, keys) = sampler(&was, n);
        let pos = ChunkPos::new(2, -1, 5);
        WorldStore::open(&dir.0, new_world(1))
            .unwrap()
            .save_chunk(pos, &chunk)
            .unwrap();

        let now = reordered();
        assert_ne!(
            now.key(BlockId(1)),
            was.key(BlockId(1)),
            "the order really changed"
        );
        let store = WorldStore::open_with_blocks(&dir.0, new_world(1), &now).unwrap();
        assert!(!store.blocks.identity);
        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(keys_in(&loaded, &now, n), keys);

        // Saved again by the reordered build, and read by the original: the
        // file is still in the WORLD's numbering, whoever wrote it.
        let mut edited = loaded;
        edited.set(LocalPos::new(0, 9, 0), now.id_of_key("peat").unwrap());
        store.save_chunk(pos, &edited).unwrap();
        let back = WorldStore::open(&dir.0, new_world(1))
            .unwrap()
            .load_chunk(pos)
            .unwrap()
            .unwrap();
        assert_eq!(keys_in(&back, &was, n), keys);
        assert_eq!(was.key(back.get(LocalPos::new(0, 9, 0))), "peat");
    }

    /// A world from before worlds kept a table: its ids are M14's. It
    /// opens, is given the table, and reads exactly as it did — also under
    /// a build whose order has since changed.
    #[test]
    fn a_world_without_a_table_holds_m14_ids() {
        let dir = TempDir::new("blocks_legacy");
        let was = BlockRegistry::default_set();
        let (chunk, keys) = sampler(&was, M14_BLOCK_KEYS.len());
        let pos = ChunkPos::new(0, 0, 0);
        WorldStore::open(&dir.0, new_world(1))
            .unwrap()
            .save_chunk(pos, &chunk)
            .unwrap();
        fs::remove_file(dir.0.join("blocks.table")).unwrap(); // as such a world is

        let now = reordered();
        let store = WorldStore::open_with_blocks(&dir.0, new_world(1), &now).unwrap();
        let table = decode_block_table(&fs::read(dir.0.join("blocks.table")).unwrap()).unwrap();
        assert_eq!(
            table, M14_BLOCK_KEYS,
            "given M14's table, not this build's order"
        );
        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(keys_in(&loaded, &now, keys.len()), keys);
        // And under the unchanged build it is the identity: nothing to do.
        fs::remove_file(dir.0.join("blocks.table")).unwrap();
        assert!(
            WorldStore::open(&dir.0, new_world(1))
                .unwrap()
                .blocks
                .identity
        );
    }

    /// A build that has gained blocks appends them to the world's table;
    /// no id the world already used moves.
    #[test]
    fn new_blocks_are_appended_and_old_ids_never_move() {
        let dir = TempDir::new("blocks_grow");
        let full = BlockRegistry::default_set();
        let early = BlockRegistry::from_types(full.types()[..20].to_vec());
        let (chunk, keys) = sampler(&early, 20);
        let pos = ChunkPos::new(1, 1, 1);
        WorldStore::open_with_blocks(&dir.0, new_world(1), &early)
            .unwrap()
            .save_chunk(pos, &chunk)
            .unwrap();
        let before = decode_block_table(&fs::read(dir.0.join("blocks.table")).unwrap()).unwrap();
        assert_eq!(before.len(), 20);

        // The later build lists its new blocks FIRST (after air): the worst
        // case for a format that trusted the order.
        let mut types = full.types();
        types[1..].rotate_right(6);
        let later = BlockRegistry::from_types(types);
        let store = WorldStore::open_with_blocks(&dir.0, new_world(1), &later).unwrap();
        let after = decode_block_table(&fs::read(dir.0.join("blocks.table")).unwrap()).unwrap();
        assert_eq!(after.len(), full.len());
        assert_eq!(
            &after[..20],
            &before[..],
            "ids already given are never moved"
        );
        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(keys_in(&loaded, &later, 20), keys);

        // A chunk using a new block round-trips.
        let mut with_new = Chunk::new_air();
        with_new.set(LocalPos::new(4, 4, 4), later.id_of_key("peat").unwrap());
        store.save_chunk(ChunkPos::new(9, 9, 9), &with_new).unwrap();
        let back = store.load_chunk(ChunkPos::new(9, 9, 9)).unwrap().unwrap();
        assert_eq!(later.key(back.get(LocalPos::new(4, 4, 4))), "peat");
    }

    /// A world using a block this build lacks is refused, by key — an old
    /// build must not open, and then save over, a newer world. A block
    /// that was renamed is not lacking: its alias finds it.
    #[test]
    fn unknown_blocks_refuse_by_name_and_aliases_resolve() {
        let dir = TempDir::new("blocks_unknown");
        let full = BlockRegistry::default_set();
        let pos = ChunkPos::new(0, 0, 0);
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(1, 1, 1), full.id_of_key("peat").unwrap());
        WorldStore::open(&dir.0, new_world(1))
            .unwrap()
            .save_chunk(pos, &chunk)
            .unwrap();

        // A build without peat or mud.
        let fewer: Vec<_> = full
            .types()
            .into_iter()
            .filter(|t| t.key != "peat" && t.key != "mud")
            .collect();
        match WorldStore::open_with_blocks(&dir.0, new_world(1), &BlockRegistry::from_types(fewer))
        {
            Err(StoreError::UnknownBlocks(keys)) => assert_eq!(keys, ["mud", "peat"]),
            other => panic!("expected the missing keys, got {other:?}"),
        }

        // A build where peat has been renamed "turf", with the alias.
        let mut types = full.types();
        types.iter_mut().find(|t| t.key == "peat").unwrap().key = "turf";
        let renamed = BlockRegistry::from_types_with_aliases(types, vec![("peat", "turf")]);
        let store = WorldStore::open_with_blocks(&dir.0, new_world(1), &renamed).unwrap();
        let loaded = store.load_chunk(pos).unwrap().unwrap();
        assert_eq!(renamed.key(loaded.get(LocalPos::new(1, 1, 1))), "turf");
    }

    /// Under the build that made it a world's numbering is the build's:
    /// chunk files are byte-for-byte what they always were, and a damaged
    /// table or an id beyond it is reported, never guessed at.
    #[test]
    fn the_unchanged_case_is_the_identity_and_damage_is_refused() {
        let dir = TempDir::new("blocks_identity");
        let registry = BlockRegistry::default_set();
        let store = WorldStore::open(&dir.0, new_world(1)).unwrap();
        assert!(store.blocks.identity);
        let (chunk, _) = sampler(&registry, registry.len());
        let pos = ChunkPos::new(3, 3, 3);
        store.save_chunk(pos, &chunk).unwrap();
        let on_disk = fs::read(store.chunk_path(pos)).unwrap();
        assert_eq!(on_disk, chunk.serialize(), "no translation: the same bytes");
        let table = fs::read(dir.0.join("blocks.table")).unwrap();
        assert_eq!(decode_block_table(&table).unwrap(), M14_BLOCK_KEYS);
        assert_eq!(
            encode_block_table(&decode_block_table(&table).unwrap()),
            table
        );

        // A chunk naming an id past the table's end.
        let mut alien = Chunk::new_air();
        alien.set(LocalPos::new(0, 0, 0), BlockId(4_000));
        fs::write(store.chunk_path(ChunkPos::new(8, 8, 8)), alien.serialize()).unwrap();
        assert!(matches!(
            store.load_chunk(ChunkPos::new(8, 8, 8)),
            Err(StoreError::UnknownBlockId(4_000))
        ));

        // A truncated table, a table of nothing, a table not starting with air.
        for bad in [
            &table[..table.len() - 3],
            &table[..5],
            b"VXTB\x01\x01\x00\x05stone".as_slice(),
        ] {
            fs::write(dir.0.join("blocks.table"), bad).unwrap();
            assert!(matches!(
                WorldStore::open(&dir.0, new_world(1)),
                Err(StoreError::BadBlockTable)
            ));
        }
    }
}
