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

/// Per-column highest-solid heights for the resident world.
#[derive(Debug, Default)]
pub struct ColumnHeights {
    /// World `(x, z)` → highest solid world-Y. `i64::MIN` is a legitimate
    /// stored value: "known, and nothing solid" (a column mined out
    /// entirely), which differs from absent ("unknown").
    heights: HashMap<(i64, i64), i64>,
    /// Chunk column `(cx, cz)` → number of resident chunks in it. A column is
    /// resident while this is non-zero; its heights live exactly that long.
    residents: HashMap<(i64, i64), u32>,
}

impl ColumnHeights {
    pub fn new() -> Self {
        Self::default()
    }

    /// The known height of a column, or `None` when it is unknown — no
    /// resident chunk has reported a solid there. Unknown means COVERED to
    /// the skylight code, never open.
    pub fn get(&self, x: i64, z: i64) -> Option<i64> {
        self.heights.get(&(x, z)).copied()
    }

    /// Raise a column to at least `h`. Returns whether it changed.
    ///
    /// Raise-only is what chunk arrival needs: chunks of a column stream in
    /// any order and the highest solid wins. Ignored for a column with no
    /// resident chunk (see the module docs).
    pub fn raise(&mut self, x: i64, z: i64, h: i64) -> bool {
        if !self.is_resident(x, z) {
            return false;
        }
        let e = self.heights.entry((x, z)).or_insert(i64::MIN);
        if h > *e {
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
        if self.is_resident(x, z) {
            self.heights.insert((x, z), h);
        }
    }

    /// A chunk became resident. Call once per chunk actually inserted into
    /// the world — not for a chunk that replaced one already there.
    pub fn chunk_loaded(&mut self, pos: ChunkPos) {
        *self.residents.entry((pos.x, pos.z)).or_insert(0) += 1;
    }

    /// A chunk stopped being resident. When it was its column's last, the
    /// column's heights are dropped. An unload with no matching load is
    /// ignored rather than underflowing the count.
    pub fn chunk_unloaded(&mut self, pos: ChunkPos) {
        let key = (pos.x, pos.z);
        let Some(count) = self.residents.get_mut(&key) else {
            return;
        };
        *count -= 1;
        if *count > 0 {
            return;
        }
        self.residents.remove(&key);
        let s = CHUNK_SIZE as i64;
        let (x0, z0) = (pos.x * s, pos.z * s);
        for z in z0..z0 + s {
            for x in x0..x0 + s {
                self.heights.remove(&(x, z));
            }
        }
    }

    /// Number of columns with a known height.
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Number of chunk columns with at least one resident chunk.
    pub fn resident_columns(&self) -> usize {
        self.residents.len()
    }

    fn is_resident(&self, x: i64, z: i64) -> bool {
        let s = CHUNK_SIZE as i64;
        self.residents
            .contains_key(&(x.div_euclid(s), z.div_euclid(s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = CHUNK_SIZE as i64;

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
