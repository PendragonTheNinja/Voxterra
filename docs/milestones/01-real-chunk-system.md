# Milestone 01 — Real Chunk System

- **Status:** COMPLETE (2026-06-13)
- **Spec written:** 2026-06-12
- **Depends on:** Milestone 00, ADR-0001

## Goal

Turn the single-chunk demo into a real multi-chunk engine: palette-compressed
storage, a 3D world of many chunks with correct cross-chunk face culling,
greedy meshing, meshing on background threads, and frustum culling.

**Acceptance scene:** a 16×16×16 grid of chunks (4,096 chunks — a 512³-block
cube of placeholder-noise terrain) that the fly camera can move through at a
solid framerate. This is the milestone that *proves* the cubic-chunk
architecture instead of declaring it.

## Acceptance criteria

1. **Palette compression** replaces `Chunk`'s flat array. Internals only:
   the existing public API (`get`/`set`/`filled`/`is_all_air`) is unchanged
   and all existing vox-core and vox-mesh tests pass without modification.
   - A palette (list of distinct `BlockId`s present) plus a packed index
     array at `ceil(log2(palette_len))` bits per voxel, in the established
     Y-major `LocalPos::index` order.
   - Uniform chunks (single palette entry — e.g. all air, all stone) store
     **no** index array. This is the common case in a cubic-chunk world
     (most chunks are entirely air or entirely underground) and is the
     single biggest memory win.
   - `set` that introduces a new block type grows the palette, widening the
     packed array when needed. (Shrinking/compaction: explicit
     `compact()` method; never automatic.)
   - Tests: round-trip equivalence against a dense reference on randomized
     chunks; uniform fast path verified; growth across bit-width boundaries
     (1→2, 2→4, etc.); memory footprint of uniform chunk is O(1).
2. **World container** in vox-core: `World` (or `ChunkMap`) holding
   `HashMap<ChunkPos, Chunk>` with `get_block`/`set_block` taking `WorldPos`
   (missing chunks read as air). No column logic anywhere.
3. **Cross-chunk culling:** meshing a chunk consults its six face-neighbors,
   so no quads are emitted between two solid blocks across a chunk border.
   The Milestone 00 "border faces always emitted" limitation is dead.
   Neighbor access must not require the whole `World` (mesher takes a chunk
   + six optional neighbor references, or equivalent) so meshing stays
   parallelizable and headless-testable.
4. **Greedy meshing:** coplanar, same-block, same-facing quads are merged
   into maximal rectangles.
   - Differential test: for randomized chunks, the greedy mesh and the naive
     culled mesh must cover exactly the same set of unit face cells (build
     per-cell coverage maps from each mesh's quads and assert equality, each
     cell covered exactly once). This is the test that lets us trust greedy
     forever.
   - The naive mesher is kept, as the test oracle.
5. **Threaded meshing:** chunk meshing runs on background threads (rayon);
   the main thread only uploads finished meshes. A dirty-chunk set drives
   re-meshing; editing one block re-meshes only the affected chunk(s)
   (including neighbors when the edit is on a border). [Block editing UI is
   Milestone 03 — for now a debug keybind that punches a hole proves the
   invalidation path.]
6. **Frustum culling:** chunks outside the camera frustum are not drawn.
   Plane extraction from the view-proj matrix + AABB test. Debug overlay or
   log line showing drawn/total chunk counts proves it works.
7. **Placeholder worldgen** in vox-worldgen: a seeded, deterministic
   heightmap function (hash-based value noise; no external noise crate yet)
   generating any requested chunk independently. Same seed + same `ChunkPos`
   → byte-identical chunk (determinism test). This is scaffolding for
   Milestone 02 streaming, not the real geology pipeline.
8. **Per-chunk rendering:** renderer manages many GPU meshes keyed by
   `ChunkPos`. Chunk world offset is baked into vertex positions at mesh
   time. (Known limitation, accepted for now: f32 precision degrades far
   from origin; camera-relative rendering is a future ADR, well before
   continent-scale worlds.)
9. The acceptance scene (4,096 chunks) generates and meshes in seconds, not
   minutes, and renders at 60+ FPS at 1080p on the dev machine (RTX-class
   NVIDIA). Empty/all-air chunks cost nothing (no mesh, no draw call).
10. `cargo test` green across the workspace; clippy/fmt clean.

## Non-goals (explicitly out of scope)

- Infinite world / chunk streaming / load-unload by distance (Milestone 02).
- Saving or loading anything to disk (Milestone 02).
- LOD, lighting, textures, sky (Milestone 03+).
- Real geological worldgen (its own milestone after 03).
- Player physics/collision; block interaction beyond the debug keybind.
- A general job system — rayon is sufficient until profiling says otherwise.

## Task breakdown (suggested order)

1. **Palette storage** in vox-core, behind the existing `Chunk` API.
   Headless, test-heavy — same delivery pattern as Milestone 00 task 2.
2. **World container** + neighbor-aware mesher signature; cross-chunk
   culling tests (solid blocks across borders emit nothing; air across
   borders emits exactly one face).
3. **Greedy mesher** + coverage-map differential test against naive.
4. **Placeholder worldgen** with determinism test.
5. **Renderer: many meshes** keyed by ChunkPos; build the 16³ scene
   single-threaded first (correctness before speed).
6. **Rayon meshing + dirty set** + debug hole-punch keybind.
7. **Frustum culling** + drawn/total debug counter.
8. Measure: generation time, meshing time, FPS, memory. Record numbers in
   the retrospective — they're the baseline every future optimization is
   judged against.

## Notes for whoever builds this (human or model)

- Palette indices use the same Y-major ordering as `LocalPos::index`. That
  ordering is now load-bearing in two places; it must also be the order of
  the Milestone 02 serialized format. Do not diverge.
- Greedy meshing merges only quads with identical block *and* face
  direction. When per-face data later grows (textures, AO), merge criteria
  tighten — the differential test must keep passing regardless.
- Keep the naive mesher exported and benchmarked alongside greedy; it is
  the permanent correctness oracle, not dead code.
- Expect the uniform-chunk fast path to make or break the 4,096-chunk
  numbers: in the test scene most chunks are all-air or all-stone. If the
  scene is slow, check that path first.
- When this milestone is accepted: update CLAUDE.md status, write the
  retrospective with the measured baseline numbers, then write
  `02-infinite-world.md` before any Milestone 02 code.



## Retrospective (2026-06-13)

**Status: COMPLETE.** All seven tasks landed; all acceptance criteria met.

### Measured baseline (record for future comparison)

Dev machine: Windows, NVIDIA GPU, 20 logical cores. Release-profile
dependencies (`opt-level=3`), `dev` profile for workspace crates.

Acceptance scene — 16×16×16 = **4,096 chunks** (512³-block world):

- **Generation:** 204 ms (single-threaded). Not parallelized; fine at this
  scale. Parallelizing gen is a Milestone 02+ option if streaming needs it.
- **Meshing:** 64 ms (rayon, parallel) — vs. ~1,050 ms single-threaded in a
  one-core sandbox, i.e. ~16× from threading. This is the headline win of
  task 6.
- **Non-empty chunk meshes:** 416 of 4,096 (~90% of chunks are uniform
  air/stone and produce no mesh — the uniform fast path carrying the scene
  exactly as predicted).
- **Total quads:** 183,890 (greedy). Determinism confirmed: identical quad
  count on the dev machine and in the sandbox.
- **Frame rate:** 142–150 fps, vsync-capped (monitor refresh, not an engine
  ceiling). Steady whether drawing 1 or 415 chunks → not GPU-bound at this
  scale; substantial render-distance headroom remains.
- **Frustum culling range:** drawn count swings from 1–2 (facing empty
  space) to 407–415 (facing into the world), confirming correct culling.

### What went well

- The headless-core / sandbox-verified discipline held: vox-core, vox-mesh,
  and vox-worldgen logic were all proven before reaching the dev machine.
  Graphics (vox-render/vox-app) compile-tested only on the dev machine, as
  expected.
- The differential test (greedy vs. naive coverage maps) caught nothing
  because greedy was right the first time — but it is the reason we can
  trust that, and can refactor greedy later without fear.
- `ChunkNeighbors`-by-borrow (decided in task 2) made task 6's rayon
  parallelization fall out almost for free: each meshing job borrows seven
  chunks immutably, no shared mutable state.
- Uniform-chunk fast path is doing exactly the load-bearing work the spec
  predicted (90% of chunks free).

### Friction / notes for next time

- The Fable→Opus 4.8 model handoff happened mid-task-3. The CLAUDE.md +
  milestone-spec + committed-history scaffolding made it a near-non-event:
  re-clone, read the spec, regenerate the in-flight task cleanly. This
  validated the whole "make the project model-proof" approach.
- Recurring class of small breakage: the build sandbox runs an older rustc
  (1.75, edition 2021) than the dev machine (1.96, edition 2024). Three
  edition/lint gaps slipped through to the dev machine: `gen` is a reserved
  keyword in 2024 (not 2021); let-chains (`if let ... && ...`); and several
  clippy lints (`collapsible_if`, `needless_range_loop`,
  `unusual_byte_groupings`, unused imports left after refactors). Mitigation
  going forward: avoid 2024 reserved words in identifiers, pre-empt the
  common clippy lints, and double-check imports after deleting code.
- f32 world-space vertex positions (chunk offset baked in at mesh time) are
  fine at 512³ but degrade far from origin. Camera-relative rendering is the
  known fix; needs its own ADR before continent-scale worlds (Milestone 02
  criterion 8 already flags this).

### Carried into Milestone 02

- The Y-major `LocalPos::index` order is now load-bearing in palette storage
  AND meshing; it must also be the serialized chunk format's order.
- `set_chunk_mesh(pos, empty)` already removes stale geometry — the
  load/unload path has a head start.
- The dirty-set + neighbor-expansion pattern from the debug hole-punch is
  the model for all future block edits and for streaming re-meshes.
