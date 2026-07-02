# Voxterra code audit — July 2026

Scope: full read of vox-core (light, chunk, storage, streaming, raycast),
vox-mesh, vox-worldgen, targeted read of vox-app and vox-render. Done right
after the M05 skylight saga concluded (border-ring conduit fix). Findings are
ordered by priority. Items marked [DO NOT FIX YET] are noted for later so the
current build stays a stable test target.

## What's solid (verified, no action)

- **Storage** is versioned (world meta + chunk format) and crash-safe:
  chunk saves write to a temp file then rename. Unload saves modified chunks
  before dropping them; exit saves everything resident. No data-loss path
  found.
- **Raycast** uses `floor()` (not integer truncation), so block selection is
  correct at negative coordinates — a classic voxel bug that is NOT present.
- **Greedy mesher** includes the face's light level in the merge mask, so
  quads never merge across differing light (no light bleeding across merged
  faces).
- **Worldgen** is deterministic per `(seed, ChunkPos)` and generates chunks
  independently, per the constitution. (Still placeholder terrain, as
  documented.)
- **Renderer** frees chunk meshes on unload (empty mesh → entry removed →
  buffers dropped).
- **Streaming** prunes ghost entries from `dirty`/`relight` each tick, and
  load/unload radii are hysteresis-separated (no thrash at boundaries).

## P1 — should fix soon

1. **`column_heights` grows without bound.** `HashMap<(i64,i64), i64>` gains
   1,024 entries per chunk-column ever visited and is never pruned (unload
   does not remove entries). ~74KB per chunk-column stack including HashMap
   overhead; a long exploration flight over ~10k chunk columns ≈ hundreds of
   MB. Recommended fix: re-key the heightmap per chunk-column
   (`HashMap<(i64,i64) /*chunk x,z*/, Box<[i64; 1024]>>`), which is cheaper
   per column and trivially prunable when the last chunk in a stack unloads.
   Note the raise-only semantics interact with pruning: pruning a stack means
   heights re-derive from chunks when the area reloads, which is correct
   (heights come from chunk contents).

2. **`Chunk::compact()` is never called in production.** Edited chunks'
   palettes grow monotonically (unused entries are never dropped) and a chunk
   edited back to uniform never regains `is_uniform()` — losing the ~5x
   relight fast path and the compact serialized form. Cheapest correct hook:
   call `compact()` in `save_chunk` just before serialize (chunk is already
   being written; compaction there is amortized and off the hot path), and/or
   opportunistically when `palette_len()` exceeds a small threshold.

## P2 — worth doing, not urgent

3. **Unload saves are synchronous in the frame.** `store.save_chunk` runs
   inline in `stream_tick` for each modified chunk being unloaded. Fine at
   current scale (only edited chunks are modified), but a burst of unloads in
   a heavily-edited area will hitch. Later: hand saves to a writer thread
   (chunk data is already owned at that point).

4. **Dead single-channel light path.** `compute_chunk_light`,
   `relight_chunk`, and `relight_chunk_2ch`'s single-channel ancestors have no
   production callers (the app uses `compute_chunk_light_2ch` via
   `relight_chunks_parallel`). Tests keep them compiling. Either delete them
   (and port any unique test coverage) or mark `#[deprecated]` with a comment
   pointing at the 2ch path, so future sessions don't "fix" the wrong
   function. This exact confusion cost time during the M05 debugging.

5. **GPU buffer churn.** Every `set_chunk_mesh` allocates fresh vertex/index/
   uniform buffers. During streaming bursts that's hundreds of allocations
   per second. Works, but a buffer pool (or `write_buffer` into resized
   allocations) is the standard fix when it shows up in profiles. Defer until
   LOD work forces a renderer pass anyway.

6. **Uniform fast-path blending gap (cosmetic, rare).** An all-air uniform
   chunk with MIXED per-column `top_sky` (some columns open, some covered —
   e.g. under a giant overhang) fast-path fills hard 15/0 column walls with no
   horizontal blending; the honest BFS would produce a gradient. Self-heals
   once neighbor planes exist (guard falls through to the full path), so at
   worst a brief hard light edge under large overhangs. Revisit only if seen.

7. **`AMBIENT = 0.06` decision still open** (from the M05 milestone doc):
   sky=0 renders at 6% grey, so deep caves are dim, never black. Deliberate
   floor vs. atmosphere choice — decide at M05 acceptance and record it.

## Process/meta notes

- The M05 root causes were all **interaction bugs** (padded-border conduit,
  unknown-heightmap-column default under async load order, mesh-before-light
  ordering), not local logic errors. Unit tests of `compute_chunk_light_2ch`
  in isolation could not catch any of them. The lasting mitigation is the
  invariants + lessons section added to CLAUDE.md — read it before touching
  lighting or streaming.
- Test suite at audit time: 135 vox-core + 16 vox-mesh, all green, including
  regression tests for every bug found in the saga.
