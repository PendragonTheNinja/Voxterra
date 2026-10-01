//! The skylight column heightmap (ADR-0005), bounded by residency.
//!
//! World `(x, z)` → the highest solid block among the RESIDENT chunks of that
//! column. It drives each chunk's skylight top boundary directly, so a chunk
//! computes its daylight in one pass without waiting on the chunks above it.
//!
//! ## Why it tracks residency (M10 A3)
//!
//! It used to be a bare map that only ever grew: every column that had ever
//! been loaded kept its entry forever, ~0.26 GB per 10 km flown. Nothing reads
//! a column that has no resident chunk — relight reads only its own chunk's
//! footprint, and edits happen only in resident chunks — so an unloaded
//! column's heights are pure leak. Worse, keeping them would be WRONG, not
//! merely wasteful: they describe chunks that are gone, and when the column
//! reloads its height must be rebuilt from what actually arrives.
//!
//! So the map counts resident chunks per chunk column and drops a column's
//! 1 024 heights when its last chunk unloads. Writes to a column with no
//! resident chunk are ignored, so the map cannot outgrow the loaded world by
//! construction rather than by every caller remembering to clean up.
//!
//! Dropping restores "unknown", which the skylight code already treats as
//! COVERED: a reloaded column starts dark and brightens once its chunks
//! arrive. That is the safe direction (CLAUDE.md lighting invariants).
//!
//! Coordinates are in the unwrapped frame (ADR-0012 §4), like everything the
//! streamer touches; nothing here knows the world wraps.

use std::collections::HashMap;

use crate::coords::{CHUNK_SIZE, ChunkPos};

/// A height slot no resident chunk has reported yet. `i64::MIN` cannot serve:
/// it already means "known, and nothing solid".
const UNKNOWN: i64 = i64::MAX;

const S: i64 = CHUNK_SIZE as i64;

/// One chunk column's heights, alive while any of its chunks is resident.
///
/// Stored per chunk column rather than per block column because relighting
/// reads a chunk's whole 32x32 footprint at once, for every relight: one map
/// lookup instead of 1 024 (M11 task 0 measured the per-block map at ~80 us
/// per relight, a third of all relight time).
#[derive(Debug)]
struct Column {
    residents: u32,
    /// Heights indexed `x + z * 32` within the column; [`UNKNOWN`] until
    /// reported.
    heights: Box<[i64]>,
    /// How many slots are not [`UNKNOWN`], for [`ColumnHeights::len`].
    known: usize,
}

/// Per-column highest-solid heights for the resident world.
#[derive(Debug, Default)]
pub struct ColumnHeights {
    /// Chunk column `(cx, cz)` → its heights and resident-chunk count. A
    /// column is here exactly while that count is non-zero.
    columns: HashMap<(i64, i64), Column>,
    /// Known heights across all columns.
    known: usize,
}

/// Chunk column and in-column slot of world column `(x, z)`.
fn slot(x: i64, z: i64) -> ((i64, i64), usize) {
    let key = (x.div_euclid(S), z.div_euclid(S));
    (key, (x.rem_euclid(S) + z.rem_euclid(S) * S) as usize)
}

impl ColumnHeights {
    pub fn new() -> Self {
        Self::default()
    }

    /// The known height of a column, or `None` when it is unknown — no
    /// resident chunk has reported a solid there. Unknown means COVERED to
    /// the skylight code, never open.
    pub fn get(&self, x: i64, z: i64) -> Option<i64> {
        let (key, i) = slot(x, z);
        let h = self.columns.get(&key)?.heights[i];
        (h != UNKNOWN).then_some(h)
    }

    /// Raise a column to at least `h`. Returns whether it changed.
    ///
    /// Raise-only is what chunk arrival needs: chunks of a column stream in
    /// any order and the highest solid wins. Ignored for a column with no
    /// resident chunk (see the module docs).
    pub fn raise(&mut self, x: i64, z: i64, h: i64) -> bool {
        debug_assert!(h != UNKNOWN, "height {h} is the unknown sentinel");
        let (key, i) = slot(x, z);
        let Some(col) = self.columns.get_mut(&key) else {
            return false;
        };
        let e = &mut col.heights[i];
        if *e == UNKNOWN {
            // First report: the column becomes known even at `i64::MIN`,
            // though only a real height counts as a change.
            *e = h;
            col.known += 1;
            self.known += 1;
            h > i64::MIN
        } else if h > *e {
            *e = h;
            true
        } else {
            false
        }
    }

    /// Replace a column's height with a fresh rescan of its resident blocks —
    /// the edit path, which must be able to lower it when a block is mined.
    /// `i64::MIN` records "known, nothing solid". Ignored for a column with no
    /// resident chunk.
    pub fn set(&mut self, x: i64, z: i64, h: i64) {
        debug_assert!(h != UNKNOWN, "height {h} is the unknown sentinel");
        let (key, i) = slot(x, z);
        if let Some(col) = self.columns.get_mut(&key) {
            if col.heights[i] == UNKNOWN {
                col.known += 1;
                self.known += 1;
            }
            col.heights[i] = h;
        }
    }

    /// A chunk became resident. Call once per chunk actually inserted into
    /// the world — not for a chunk that replaced one already there.
    pub fn chunk_loaded(&mut self, pos: ChunkPos) {
        self.columns
            .entry((pos.x, pos.z))
            .or_insert_with(|| Column {
                residents: 0,
                heights: vec![UNKNOWN; (S * S) as usize].into_boxed_slice(),
                known: 0,
            })
            .residents += 1;
    }

    /// A chunk stopped being resident. When it was its column's last, the
    /// column's heights are dropped. An unload with no matching load is
    /// ignored rather than underflowing the count.
    pub fn chunk_unloaded(&mut self, pos: ChunkPos) {
        let key = (pos.x, pos.z);
        let Some(col) = self.columns.get_mut(&key) else {
            return;
        };
        col.residents -= 1;
        if col.residents == 0 {
            self.known -= col.known;
            self.columns.remove(&key);
        }
    }

    /// The daylight entering `pos` through its top face, as a 32x32 plane
    /// indexed `x + z * 32`: 15 where the column's highest solid is below the
    /// chunk, else 0 — and 0 for every UNKNOWN column, which is covered,
    /// never open (CLAUDE.md lighting invariants). This is every relight's
    /// `top_sky`, read with one lookup for the whole footprint.
    pub fn sky_top(&self, pos: ChunkPos) -> Vec<u8> {
        let mut plane = vec![0u8; (S * S) as usize];
        if let Some(col) = self.columns.get(&(pos.x, pos.z)) {
            let floor = pos.y * S;
            for (out, &h) in plane.iter_mut().zip(col.heights.iter()) {
                if h != UNKNOWN && h < floor {
                    *out = crate::MAX_LIGHT;
                }
            }
        }
        plane
    }

    /// The skylight entering `pos` from above when the chunk above is ABSENT
    /// AND NEVER COMING (sealed), as a 32x32 plane indexed `x + z * 32` — the
    /// layout of a +Y neighbour's sky plane.
    ///
    /// Why it exists (M10 A1): over deep ocean only the seabed and the sea
    /// surface are resident; the water column between is not. With no chunk
    /// above, the seabed chunk received no daylight at all, although nothing
    /// but water — which passes light — lies between it and the sky.
    ///
    /// The rule is the heightmap's own, one chunk up: a column whose highest
    /// opaque block is below the chunk above has open sky there, so 15; any
    /// other column, and every UNKNOWN one, 0 — unknown is covered (CLAUDE.md
    /// lighting invariants). Only for a sealed neighbour: one still coming
    /// supplies its real plane when it arrives, and guessing before then would
    /// light a cave under a surface chunk that has not streamed in yet.
    pub fn sky_plane_above(&self, pos: ChunkPos) -> Vec<u8> {
        self.sky_top(ChunkPos::new(pos.x, pos.y + 1, pos.z))
    }

    /// Number of columns with a known height.
    pub fn len(&self) -> usize {
        self.known
    }

    pub fn is_empty(&self) -> bool {
        self.known == 0
    }

    /// Number of chunk columns with at least one resident chunk.
    pub fn resident_columns(&self) -> usize {
        self.columns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(x: i64, y: i64, z: i64) -> ChunkPos {
        ChunkPos::new(x, y, z)
    }

    /// Load one chunk and report a height for every column of it, the way
    /// chunk arrival does.
    fn load_column(h: &mut ColumnHeights, pos: ChunkPos, height: i64) {
        h.chunk_loaded(pos);
        for z in pos.z * S..(pos.z + 1) * S {
            for x in pos.x * S..(pos.x + 1) * S {
                h.raise(x, z, height);
            }
        }
    }

    /// THE bug: the heightmap grew forever, one entry per column ever
    /// loaded. Travelling far must leave it bounded by what is resident.
    #[test]
    fn travelling_does_not_grow_the_heightmap() {
        let mut h = ColumnHeights::new();
        // A 3-column-wide strip sliding 200 chunk columns along +X.
        for step in 0..200 {
            for dz in -1..=1 {
                load_column(&mut h, cp(step + 1, 0, dz), 10);
            }
            if step >= 2 {
                for dz in -1..=1 {
                    h.chunk_unloaded(cp(step - 2, 0, dz));
                }
            }
        }
        // At most the 3x3 resident window's columns remain.
        assert!(
            h.resident_columns() <= 9,
            "{} columns resident",
            h.resident_columns()
        );
        assert!(
            h.len() <= 9 * (S * S) as usize,
            "heightmap grew to {} entries",
            h.len()
        );
    }

    /// A column keeps its heights while ANY of its chunks is resident: the
    /// surface chunk unloading while the camera window keeps a deep chunk of
    /// the same column does not make the column unknown.
    #[test]
    fn heights_survive_until_the_last_chunk_of_the_column_unloads() {
        let mut h = ColumnHeights::new();
        load_column(&mut h, cp(0, 0, 0), 20);
        h.chunk_loaded(cp(0, -5, 0));
        h.chunk_unloaded(cp(0, 0, 0));
        assert_eq!(h.get(3, 4), Some(20), "dropped with a chunk still resident");
        h.chunk_unloaded(cp(0, -5, 0));
        assert_eq!(h.get(3, 4), None, "kept after the last chunk unloaded");
        assert!(h.is_empty());
    }

    /// Dropping must restore UNKNOWN, not some stale value: a reloaded column
    /// is rebuilt from what actually arrives, and until then reads unknown
    /// (covered) — never the old height.
    #[test]
    fn a_reloaded_column_starts_unknown_and_is_rebuilt() {
        let mut h = ColumnHeights::new();
        load_column(&mut h, cp(2, 0, 2), 40);
        h.chunk_unloaded(cp(2, 0, 2));
        h.chunk_loaded(cp(2, 0, 2));
        assert_eq!(h.get(2 * S, 2 * S), None, "a stale height survived");
        h.raise(2 * S, 2 * S, 7);
        assert_eq!(h.get(2 * S, 2 * S), Some(7));
    }

    /// Writes to a column with no resident chunk are ignored, so no caller
    /// can leak an entry that nothing will ever prune.
    #[test]
    fn writes_to_non_resident_columns_are_ignored() {
        let mut h = ColumnHeights::new();
        assert!(!h.raise(5, 5, 10));
        h.set(5, 5, 10);
        assert!(h.is_empty());
    }

    #[test]
    fn raise_only_raises_and_reports_change() {
        let mut h = ColumnHeights::new();
        h.chunk_loaded(cp(0, 0, 0));
        assert!(h.raise(1, 1, 10));
        assert!(!h.raise(1, 1, 5), "a lower height must not change it");
        assert!(!h.raise(1, 1, 10), "an equal height is no change");
        assert!(h.raise(1, 1, 12));
        assert_eq!(h.get(1, 1), Some(12));
    }

    /// The edit path rescans and replaces — including lowering when mined,
    /// and recording a fully mined column as known-empty, not unknown.
    #[test]
    fn set_replaces_including_lowering_and_known_empty() {
        let mut h = ColumnHeights::new();
        load_column(&mut h, cp(0, 0, 0), 30);
        h.set(4, 4, 12);
        assert_eq!(h.get(4, 4), Some(12));
        h.set(4, 4, i64::MIN);
        assert_eq!(
            h.get(4, 4),
            Some(i64::MIN),
            "known-empty collapsed to unknown"
        );
    }

    /// Negative coordinates belong to the chunk column that floor division
    /// gives, and pruning removes exactly that column's footprint.
    #[test]
    fn negative_columns_prune_their_own_footprint() {
        let mut h = ColumnHeights::new();
        load_column(&mut h, cp(-1, 0, -1), 3);
        load_column(&mut h, cp(0, 0, 0), 4);
        assert_eq!(h.get(-1, -1), Some(3));
        h.chunk_unloaded(cp(-1, 0, -1));
        assert_eq!(h.get(-1, -1), None);
        assert_eq!(h.get(-S, -S), None);
        assert_eq!(h.get(0, 0), Some(4), "pruned the neighbouring column");
        assert_eq!(h.len(), (S * S) as usize);
    }

    /// The seabed under unloaded water gets daylight where the heightmap
    /// shows open sky above it — and only there.
    #[test]
    fn the_plane_above_follows_the_heightmap() {
        let mut h = ColumnHeights::new();
        let seabed = cp(0, -9, 0);
        h.chunk_loaded(seabed);
        let top_of_chunk = -9 * S + S - 1;
        h.raise(3, 4, top_of_chunk - 5); // seabed inside the chunk: open above
        h.raise(5, 6, top_of_chunk); // opaque in the chunk's top layer: still open above
        h.raise(7, 8, top_of_chunk + 40); // something opaque above: covered
        h.set(9, 9, i64::MIN); // known, nothing opaque: open
        let plane = h.sky_plane_above(seabed);
        let at = |x: i64, z: i64| plane[(x + z * S) as usize];
        assert_eq!(at(3, 4), crate::MAX_LIGHT);
        assert_eq!(at(5, 6), crate::MAX_LIGHT);
        assert_eq!(at(7, 8), 0, "daylight under something opaque");
        assert_eq!(at(9, 9), crate::MAX_LIGHT);
    }

    /// The lighting invariant: an unknown column is covered, never open.
    #[test]
    fn an_unknown_column_is_dark_above() {
        let mut h = ColumnHeights::new();
        h.chunk_loaded(cp(0, -9, 0));
        let plane = h.sky_plane_above(cp(0, -9, 0));
        assert!(
            plane.iter().all(|&v| v == 0),
            "an unknown column let daylight in"
        );
    }

    /// `sky_top` reads the whole footprint at once; it must agree, column
    /// for column, with the per-column rule through `get` — known below the
    /// chunk is open, known at or above it is covered, unknown is covered —
    /// including in negative chunk columns.
    #[test]
    fn sky_top_matches_the_per_column_rule() {
        for pos in [cp(0, 0, 0), cp(-3, -2, 5), cp(-1, 4, -1)] {
            let mut h = ColumnHeights::new();
            h.chunk_loaded(pos);
            let (x0, z0, floor) = (pos.x * S, pos.z * S, pos.y * S);
            for dz in 0..S {
                for dx in 0..S {
                    // Every column a different case; a quarter stay unknown.
                    match (dx + dz * 7) % 4 {
                        0 => {}
                        1 => h.set(x0 + dx, z0 + dz, floor - 1 - dx),
                        2 => h.set(x0 + dx, z0 + dz, floor + dz),
                        _ => h.set(x0 + dx, z0 + dz, i64::MIN),
                    }
                }
            }
            let plane = h.sky_top(pos);
            for dz in 0..S {
                for dx in 0..S {
                    let open = h.get(x0 + dx, z0 + dz).is_some_and(|y| y < floor);
                    let want = if open { crate::MAX_LIGHT } else { 0 };
                    assert_eq!(plane[(dx + dz * S) as usize], want, "{pos:?} ({dx},{dz})");
                }
            }
            assert_eq!(h.sky_plane_above(cp(pos.x, pos.y - 1, pos.z)), plane);
        }
    }

    /// A non-resident chunk column has no heights: all covered.
    #[test]
    fn sky_top_of_a_non_resident_column_is_dark() {
        let h = ColumnHeights::new();
        assert!(h.sky_top(cp(4, 0, 4)).iter().all(|&v| v == 0));
    }

    /// `len` counts known heights through every path that makes one known,
    /// and forgets them with their column.
    #[test]
    fn len_counts_known_heights() {
        let mut h = ColumnHeights::new();
        h.chunk_loaded(cp(0, 0, 0));
        h.raise(0, 0, 5);
        h.raise(0, 0, 9); // same column again: still one
        h.set(1, 0, 3);
        h.raise(2, 0, i64::MIN); // known-empty is known
        assert_eq!(h.len(), 3);
        h.chunk_unloaded(cp(0, 0, 0));
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn an_unmatched_unload_is_ignored() {
        let mut h = ColumnHeights::new();
        h.chunk_unloaded(cp(9, 0, 9));
        load_column(&mut h, cp(9, 0, 9), 1);
        h.chunk_unloaded(cp(9, 0, 9));
        h.chunk_unloaded(cp(9, 0, 9));
        assert_eq!(h.resident_columns(), 0);
        assert!(h.is_empty());
    }
}
