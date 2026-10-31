# Milestone 12 — Performance Pass

**Goal:** spend one milestone making what exists cheaper, measured, before
Climate & Biomes (M13) adds load to every chunk. Sprint-flight should keep up
with itself; nothing should send the GPU work it throws away; and the frame's
CPU cost should be known before low-end hardware is asked to carry it.

Depends on M11 (the benchmark, the headless renderer and the hole census are
the instruments). Written 2026-10-31, at the owner's request, from M11's
benchmark and a review of another engine (`docs/vision.md` §10).

## Why this milestone exists

Pillar 1 is performance on low-end hardware, and M11's benchmark (RTX 4060,
`--bench`, 2026-10-17) says where the time goes:

| Measured | Standing | Sprint-fly (120 m/s) |
|---|---|---|
| fps median, 6 / 8 levels | 905 / 778 | 135 / 128 |
| mesh work, ms per second | 0 | **~580** |
| relight work, ms per second | 0 | ~95 |
| `dirty` backlog at the end of a 30 s sprint | 0 | **650–730** — never drains while moving |

1. **Meshing limits streaming.** Six times the relight cost, and the backlog
   only empties once the player stops. The full-resolution T-junction fix at
   M11's close (`emit_sealed_rect`) added work to every mesh and has not been
   benchmarked yet.
2. **The GPU is sent triangles it discards.** LOD fans and walls store each
   triangle as a quad whose second half has zero area: after M11's wall fix
   the harness submitted 2.17 M LOD triangles where 1.50 M were sent before
   it (6 levels, 2026-10-18), most of the difference padding. Task 0 measures
   it exactly. The full-resolution mesh stopped doing this at M11's close.
3. **Draw calls grow with view distance.** One draw per chunk (~200) and per
   LOD node (640–1 872). Irrelevant at 900 fps on the development machine; the
   first thing to matter on weak CPUs, and nobody has measured it.

## Acceptance criteria

1. **Measured first.** A `--bench` baseline on the M11 tag, with the
   full-resolution fix in, before any change. Every later criterion compares
   against it, same machine, same window size.
2. **The benchmark reports what this milestone changes:** chunks meshed per
   second and mean mesh time per chunk; CPU time to encode a frame; draw calls
   per frame; triangles submitted and triangles drawn.
3. **Sprint keeps up.** At 8 levels, the sprint phases' `dirty` backlog ends
   at most a third of the baseline's (≤ ~240), or mesh ms/s halves —
   whichever the profile shows is reachable. A measured miss is recorded, not
   hidden.
4. **No zero-area triangles in LOD.** LOD fans and walls emit three indices per
   triangle with shared vertices; submitted LOD triangles equal drawn. Every
   existing LOD test (watertightness, morph, coverage) still passes.
5. **Draw-call cost known, and cut if it matters.** If encoding a frame costs
   more than ~1 ms of CPU at 8 levels, chunk and LOD geometry move into shared
   buffers drawn with indirect draws; otherwise the measurement is recorded and
   the work deferred. A fallback must exist for adapters without multi-draw.
6. **Nothing looks different.** The hole census reads the same or better on
   the owner's GPU; the speck and white-box tests still pass; lighting
   unchanged.
7. **Bookkeeping.** An ADR for any change to how geometry reaches the GPU
   (shared buffers / indirect draws). Retro with before/after numbers.

## Tasks

0. **Baseline and instruments.** Extend `--bench` (criterion 2); run it on
   the M11 tag.
   - [x] Instruments (2026-10-31). The report gains a second table, one row
     per phase: chunks meshed per second; mesh µs per chunk — the workers'
     summed CPU time per job, not the main thread's wall time, which the
     first table's mesh ms/s still reports; encode ms, median and p99 — CPU
     from after acquiring the frame's target to after `submit`, so culling,
     recording every pass and handing it to the driver, but not waiting for
     a swapchain image; draw calls, every pass; triangles submitted and
     triangles with area (`vox_mesh::area_triangles`, counted once per mesh
     at upload), and the zero-area share between them. Code is the M11 tag
     (`v0.11.0-m11` is the commit before) plus these counters; nothing
     drawn or streamed changes.
   - [ ] Baseline: the owner runs `cargo run -p vox-app -- --bench` (as in M11)
     on the RTX 4060 and the table is recorded here, with the window size.
   - [ ] Lavapipe worst case: one fixed headless frame's time in the cloud
     container (see "Low-end hardware" below).
1. **Profile meshing under streaming.** Where does `mesh_chunk` spend its
   time: building `MeshInput`, the per-slice masks, AO and light corners, the
   sealed-rectangle pass, allocation? Fix in order of measured share. Likely
   candidates, to be confirmed by the profile, not assumed:
   - chunks that cannot have faces (buried, or entirely one block with
     like-for-like neighbours) skipped before any mask is built — uniform
     chunks are ~86% of the stream (CLAUDE.md lesson 6);
   - buffers reused across slices and jobs instead of allocated per call;
   - the sealed-rectangle corner set built without a hash set.
2. **LOD as true triangles** (criterion 4).
3. **Draw-call measurement, then batching if warranted** (criterion 5).
4. **Retro.**

## Non-goals

- **No new visuals.** Fading into the real sky colour, clouds, better water and
  shadows are ideas in `docs/vision.md` §10, not this milestone.
- **No GPU culling or GPU meshing.** Possible later; this milestone measures
  first and does the cheap things.
- **No change to what is generated.** `GENERATOR_VERSION` does not move.

## Low-end hardware without a low-end machine

No weaker machine is available (owner, 2026-10-31), so low-end performance is
judged by proxy until one is:

- **Budgets, not just fps.** Report CPU milliseconds per frame (encode,
  streaming, uploads) and per-job costs, not only fps on the RTX 4060 — a
  machine four times slower has a quarter of the headroom, and fps at 900
  hides where it would go.
- **Scale with work, not hardware.** Prefer changes that cut work per chunk,
  per node and per frame (fewer triangles, draws, allocations), which help on
  any machine, over ones that trade CPU for GPU or the reverse.
- **A software renderer as the worst case.** The cloud container runs the
  real renderer on lavapipe (CPU-only Vulkan) through
  `Renderer::new_headless`; a fixed headless frame's time there is a
  repeatable, if pessimistic, stand-in for weak graphics.
- **Defaults stay modest.** The default view distance and load radius are
  chosen for an ordinary machine, not the development one (M11's open
  question).
