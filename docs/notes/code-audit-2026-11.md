# Voxterra code audit — November 2026

Done 2026-11-09 at the owner's request ("a full codebase audit … anything
to make this game better"), between M14's close and M15's first code. It
follows the July audit (`code-audit-2026-07.md`); that audit's open items are
scored at the end.

The work that follows from it is in `docs/plans/` — one work package per
finding that was not fixed here, written so a later session can pick one up
cold.

## How it was done, and what it covers

33 000 lines of Rust and WGSL. Read in full: `vox-core`'s chunk, storage,
light, streaming, heightmap, registry, world and coords; `vox-app`'s
streaming, relight, LOD, physics, edit and save paths; the generator's
chunk and LOD entry points. Read for structure and hot paths only:
`vox-render`, `vox-mesh` (both were profiled and reworked in M11–M12 and are
the best-tested code in the repository), and the inside of the stage-1
simulations (climate, drainage, rivers), which were audited for determinism
hazards rather than for geology. Not reviewed: the shaders beyond their
structure, and whether the terrain is *right* — that is the owner's eye.

Every performance claim below was measured in the cloud container (4 cores,
dev profile — `opt-level = 1`, as the game is run and benchmarked), with the
default seed. The game itself was also run there for the first time: a
software GPU (lavapipe) under a virtual display runs the real binary at
about 11 fps, enough to watch it stream, settle and not crash. How to do
that again is in CLAUDE.md, *Cloud sessions*.

Baseline before any change: `cargo clippy --workspace --all-targets -- -D
warnings` clean, `cargo fmt --check` clean, all tests passing (vox-core 288,
vox-mesh 71, vox-worldgen 72, vox-render 21, vox-app 11).

## The headline

**The frame was waiting in a queue behind background work.** Chunk
generation and LOD node building were `rayon::spawn`ed onto rayon's global
pool. Relighting and meshing run on that same pool, from the frame thread,
which blocks until its batch is done — and a batch submitted from outside
the pool joins the queue *behind* everything already spawned. One frame may
spawn 64 generation jobs (~1 ms each) and 48 LOD jobs (2–7 ms each).

Measured headless: a 32-chunk relight batch takes **6.3 ms** on an idle pool
and **42.7 ms** queued behind one frame's worth of spawns.

The telemetry books that wait as relight time, because `lt` is the batch's
wall clock (meshing has had a CPU-time counter since M12; relight never
did). This is very likely the answer to two open questions in the
retrospectives: why relight "doubled" from M09 to M10 and again from M11 to
M12 with no change to the lighting code. Worldgen got heavier each time
(terrain in M10, climate in M13, rivers in M14), so the jobs relight queued
behind got longer.

Fixed here with a separate pool for background work (below). The complete
fix — relight and mesh never blocking the frame at all — is work package
`P1` in the plans.

## What was fixed in this audit

All behaviour-preserving unless marked. Tests: vox-core 288 → 298,
vox-worldgen 72 → 74; clippy and fmt clean; the terrain fingerprint is
unchanged, so `GENERATOR_VERSION` stays 8.

| # | Change | Measured |
|---|---|---|
| 1 | **Generation: a chunk column's ground is computed once, not once per chunk of its stack** (`GroundCache`, vox-worldgen). `generate_chunk` evaluated the full surface function for all 1 024 columns before it could tell a chunk was all air or all stone — and the streamer loads ~7 chunks per column. | 4.16 → 1.26 ms per 7-layer stack. Uniform chunks 483 → ~0–145 µs, mixed 889 → 438 µs |
| 2 | **Background work has its own thread pool** (`background_pool`, vox-app): generation and LOD no longer queue ahead of the frame's relight and mesh batches. | 32-chunk relight batch under load: 42.7 → 11.4 ms (2 background threads on 4 cores) |
| 3 | **Relight: the block-light flood is skipped when a chunk has no emitter and no lit border** — nearly every chunk of an unlit world. It ran a full 34³ flood to write zeros. | Surface-chunk relight 516 → 246 µs |
| 4 | **Light is stored as cheaply as its contents allow** (`Light::{Dark, Uniform, Full}`, vox-core). Open air under the sky is one value in every voxel; each such chunk held it 32 768 times. | 32 KB → 1 byte per uniformly lit chunk: by count of air chunks, ~20 MB at radius 8, ~200 MB at radius 24 (estimated, not measured) |
| 5 | **Applying and comparing light is one pass over bytes**, not 65 536 nibble calls per chunk, on the frame thread. | Apply 33 → 13 µs per chunk |
| 6 | **A column's loaded chunks are looked up, not searched for** (`ColumnHeights::resident_layers`). Each arriving chunk whose height rose walked every loaded chunk to find its own stack — on the frame thread, against CLAUDE.md's own rule. | O(loaded chunks) → O(layers in the column) per arriving chunk |
| 7 | **Autosave every 60 s** *(new behaviour)*. Edits reached disk only when their chunk unloaded or the game closed cleanly; a crash lost everything built near the player. Only chunks changed since their last save are written (`Chunk::is_unsaved`), so unloading an old base no longer rewrites it either. | — |
| 8 | **`--bench` runs in its own world folder** *(new behaviour)*, `bench/world-v<generator version>`. It opened the player's world, so it measured that world's edits and refused to run when the world was a version old — which stopped the M14 re-bench on 2026-11-04. | — |
| 9 | **An edit's height rescan reads exactly the resident chunks** *(bug fix, rare)*. It scanned the layers the streamer *loads*; the streamer *keeps* one more around the camera. A roof in that band was invisible to the scan, the height was set below the real top, and daylight was invented under it. | — |
| 10 | **A block can no longer be placed into a chunk that is still streaming in** *(bug fix, rare)*. `World::set_block` created the chunk; the real one then replaced it, the block vanished, and the column's resident count was left one short. | — |
| 11 | Two `powi(2)` calls in drainage generation replaced by multiplication, as `climate.rs`'s determinism rule requires (bit-identical here; fingerprint unchanged). A stray `git diff` output committed as a file named `-files release` removed from the repository root. | — |

**The live run agrees.** Old and new builds, started at spawn under the
software GPU, end in the same state to the digit — 1 398 chunks loaded,
256 meshes, 0.60 M triangles drawn, 202 MB of GPU buffers — which is the
best evidence available here that nothing visible changed. Relight's share
of the frame thread over that start-up fell from ~1 260 ms to ~410 ms, and
the world settled in ~11 s against ~13 s. (At 11 fps on a software
rasteriser those are indicative only; the owner's `--bench` is the real
measurement.)

**What the owner should check**, because nothing here can: that the game
looks and feels the same or better in flight; that breaking and placing
blocks lights correctly, including under an overhang and down a shaft
(items 3, 6, 9 touch that path); that `autosave: saved N edited chunks`
appears in the log about a minute after an edit; and the bench numbers.

## Findings still open

Ordered by what they cost the player. Each has a work package in
`docs/plans/engineering-backlog.md` under the code given.

### Needs the owner's decision first

- **D1. The heightmap is documented as raise-only and the code lowers it.**
  CLAUDE.md, *Lighting & streaming invariants*: "The heightmap is raise-only
  (edits/digging never lower it)". But every block break and place runs
  `recompute_height_column` (raise-only) and then, through
  `invalidate_lod_at`, `recompute_column_height`, which calls
  `ColumnHeights::set` and *does* lower it — since M09's "ghost block" fix;
  `heightmap.rs` documents `set` as "the edit path, which must be able to
  lower it". The two functions have near-identical names and
  `recompute_height_column` carries three stacked, contradictory doc
  comments. Analysis: lowering to the *true* top of the resident column is
  physically right and gives correct light at once; raise-only is also
  right, but slower to converge and immune to a rescan that misses a chunk
  (item 9 above was exactly that). Either is defensible; having both, with
  the constitution describing one and the code doing the other, is how the
  next lighting bug gets "fixed" in the wrong function. Decide, record it,
  delete one path, and add the seam test.
- **D2. What happens to a player's world when the generator changes.**
  Today a world made by another `GENERATOR_VERSION` is refused. That is
  right for development and cannot ship: every terrain improvement would
  delete every player's world. Needs an ADR before the first build anyone
  else plays (options in the plan).
- **D3. Saved worlds store raw numeric block ids.** `STONE = 1` is the
  save format. Geology will add hundreds of blocks; one reordering corrupts
  every saved chunk silently. A name table in the world's metadata fixes
  it, and is cheap now and expensive later.

### Performance (pillar 1)

- **P1. The frame thread still blocks on relight and mesh.** The separate
  pool removes the queueing; the frame still waits for its own batches
  (budgets of 3 ms and 4 ms that a single slow batch overruns — a 32-chunk
  relight batch is ~3 ms of work on 4 cores *after* this audit's
  speed-ups). The durable design is results over a channel, as generation
  already does.
- **P2. Vertices are 36 bytes** (`vox_mesh::Vertex`, `LodVertex`: six
  `f32`/`u32` fields; the comment says 32). A chunk-local position needs
  18 bits, light 8, shade 8, a layer 10: 8 bytes holds a vertex, and
  indices are `u32` where most meshes would fit `u16`. GPU memory is 202 MB
  at the *default* settings — about 190 MB of it the 640 LOD nodes, ~0.3 MB
  each — and a low-end card has 1–2 GB.
- **P3. Draw order.** Opaque chunks are drawn in hash-map order, not front
  to back, so a weak GPU shades fragments that are then covered.
  Transparent faces are drawn unsorted with blending and no depth write, so
  water seen through water (a lake behind a river, a waterfall's sheet) can
  blend in the wrong order.
- **P4. No mipmaps** on the block texture array (`mip_level_count: 1`,
  nearest filtering): distant full-resolution ground shimmers as the camera
  moves, and every distant fragment samples the full-size tile.
- **P5. One GPU buffer set per mesh, reallocated on every re-mesh**; the
  render-origin move rewrites every mesh's offset uniform each time the
  camera crosses a chunk (July audit P2 #5, still open; M12 measured the
  encode at ~0.7 ms and deferred it).
- **P6. LOD nodes cost ~7 ms each to sample** at every stride above 2
  (1.9 ms at stride 2) and 48 may be spawned per tick. They do not use the
  new ground cache.
- **P7. The frame rate is uncapped by default** (`PresentMode::Immediate`;
  vsync only through an environment variable). Right for measuring, wrong
  for a laptop: the GPU runs flat out to draw frames the display drops.
- **P8. Every benchmark so far is a dev build.** There is no tuned release
  profile, and players will run release. The two can rank optimisations
  differently.

### Safety of the player's work

- **S1. Saves are synchronous on the frame thread, one file per chunk,
  uncompressed** (32 KB each), and the whole LOD edit overlay is rewritten
  whenever any chunk saves. Fine today; a hitch generator once gameplay
  modifies many chunks (July audit P2 #3).
- **S2. Nothing but chunks is saved.** The player's position, the time of
  day and every setting reset on each launch.
- **S3. A panic saves nothing.** Autosave bounds the loss to a minute; a
  panic hook that attempts a final save would bound it to nothing.
- **S4. `Chunk::compact()` is still never called** (July audit P1 #2): an
  edited chunk's palette only grows, and a chunk mined back to uniform
  never regains the uniform fast paths or the 7-byte saved form.
- **S5. Player builds are invisible at distance.** The LOD's edit overlay
  only lowers terrain (`a_placed_block_never_raises_a_cell`); a tower
  vanishes at the full-resolution radius. For a building game this is the
  largest visual gap in the engine.

### Structure

- **A1. `vox-app/src/main.rs` is 3 400 lines and one struct.** Streaming,
  relighting, meshing, LOD, physics, editing, saving, input, the benchmark
  hooks and the event loop are methods of `App`, which cannot exist without
  a window. So the code where every M05 bug lived — interactions across
  chunk seams and load order — has no test at all, and CLAUDE.md's lesson 2
  ("isolated unit tests cannot validate multi-chunk systems") has nowhere
  to be applied. Extracting the world session (world, streamer, heightmap,
  queues, relight and mesh scheduling) into a headless module that a test
  can drive with a scripted camera is the single most valuable structural
  change available. Items 9 and 10 above were found by reading; a soak test
  would have found them by running.
- **A2. Physics is frame-rate dependent and unswept**
  (`App::physics_update`): one step per frame with `dt` up to 0.1 s, a
  collision test at the end position only (at terminal velocity that is a
  7.8-block step — through a floor on a hitch), and a blocked move is
  cancelled whole rather than moved to contact. No swimming. Needs a fixed
  timestep before entities, and before multiplayer makes it a protocol.
- **A3. The world is built before the window exists** (`App::new`): six
  seconds of drainage simulation with nothing on screen.
- **A4. Toolchain and editions are not pinned.** The workspace says edition
  2021, four crates say 2024, `vox-app` inherits 2021; CLAUDE.md says both.
  No `rust-toolchain.toml`, so native 1.96 and the cloud's 1.97 can disagree
  about clippy — the round-trips `docs/notes/clippy-lints.md` records.
- **A5. No CI.** Every check depends on someone remembering the gauntlet.
- **A6. `vox-worldgen`'s tests take 63 s**, most of it rebuilding the same
  stage-1 maps in each of ~40 tests.

### Small things

- `bench/` is git-ignored, so bench reports have no tracked home (the M15
  spec said "committed under `bench/`"; that cannot work). Baselines now go
  in `docs/bench/`.
- `App::lod_tick` rebuilds the block-to-texture table every frame; trivial
  now, not with hundreds of blocks.
- Chunk deserialisation accepts trailing bytes and duplicate palette
  entries, and carries no checksum.
- The single-channel light functions (`compute_chunk_light`,
  `relight_chunk`, `relight_chunk_2ch`) are still exported and still have
  no production caller (July audit P2 #4; CLAUDE.md warns about them).
- CLAUDE.md said the jump clears ~0.5 m; the code's clears 1.25 blocks
  (`JUMP_SPEED`). Corrected in this audit, along with the edition and the
  test counts.
- wgpu 24, egui 0.31 and winit 0.30 are each several releases old.

## What is solid

Worth saying, because it is most of the code.

- **The lighting invariants hold in the code**, and each has its
  regression test. The border ring never conducts; unknown columns are
  covered; a chunk queued for relight is never meshed. D1 is the one place
  the documentation and the code have drifted.
- **Determinism is taken seriously where it matters**: integer heights,
  hand-built `exp`, fixed summation order, ties broken by index, a pinned
  fingerprint that fails on any change to output. The audit found only the
  two `powi` calls, both harmless.
- **The torus seam is where the constitution says it is** — generation,
  the save layer, the LOD overlay — and nowhere else.
- **Serialised formats are versioned and written atomically** (temp file,
  then rename), all four of them.
- **The mesher** is tested against a naive reference on random chunks, and
  its M12 optimisations each carry the measurement that justified them.
- **Streaming and LOD selection are pure bookkeeping**, separate from what
  they schedule, and fully tested. That separation is what made this
  audit's fixes small.
- **The instruments are unusually good**: the bench, the hole census, the
  world map, the surveys. Few projects this size can answer "is it faster?"
  and "is it still Earth-like?" with a command.

## The July audit's items

| July item | Now |
|---|---|
| P1 #1 `column_heights` grows without bound | Fixed in M10 A3 (`ColumnHeights` tracks residency) |
| P1 #2 `Chunk::compact()` never called | **Still open** — S4 |
| P2 #3 Unload saves synchronous in the frame | **Still open** — S1; reduced here (only unsaved chunks are written) |
| P2 #4 Dead single-channel light path | **Still open** — small things |
| P2 #5 GPU buffer churn | **Still open** — P5 |
| P2 #6 Uniform fast-path blending gap | Not seen since; the guard is unchanged |
| P2 #7 Ambient floor undecided | Decided in M07 (0.004) |
