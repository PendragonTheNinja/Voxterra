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
   - [x] **Baseline, first table (owner's run, 2026-10-31):** RTX 4060,
     800×600 as in M11, seed 0x7e22ac0de, defaults (radius 8, curvature on,
     air density 1). Built without the instruments (only the first table),
     but the same code otherwise: the M11 tag, full-resolution fix in. Every
     phase settled (2.4–2.8 s standing, 1.0 s into the sprints).

     | Phase | fps median | 1% low | Worst ms | Relight ms/s | Mesh ms/s | Dirty peak / end | LOD nodes | GPU MB | Tris M | Loaded |
     |---|---|---|---|---|---|---|---|---|---|---|
     | 6 levels, land, standing | 927 | 635 | 5.5 | 0 | 0 | 0 / 0 | 1216 | 599 | 1.82 | 1432 |
     | 6 levels, land, sprint | 119 | 53 | 27.0 | 173 | 591 | 1142 / 1017 | 1424 | 640 | 1.64 | 2204 |
     | 8 levels, land, standing | 793 | 514 | 3.7 | 0 | 0 | 0 / 0 | 1600 | 957 | 2.24 | 1432 |
     | 8 levels, land, sprint | 115 | 52 | 33.1 | 174 | 575 | 1181 / 1063 | 1872 | 1053 | 2.01 | 2204 |
     | 6 levels, ocean, standing | 818 | 552 | 3.0 | 0 | 0 | 0 / 0 | 1216 | 591 | 1.83 | 1475 |
     | 6 levels, ocean, sprint | 114 | 53 | 25.1 | 180 | 583 | 1155 / 1034 | 1424 | 603 | 1.57 | 2204 |
     | 8 levels, ocean, standing | 752 | 487 | 3.6 | 0 | 0 | 0 / 0 | 1600 | 947 | 2.21 | 1475 |
     | 8 levels, ocean, sprint | 111 | 53 | 30.8 | 182 | 570 | 1188 / 1056 | 1872 | 1015 | 1.88 | 2204 |

     Against M11's run (2026-10-17, before the LOD wall fix and the
     full-resolution fix): sprint fps 128 → 115 at 8 levels; the sprint
     backlog ends ~1 060, not ~680 — **criterion 3's target is therefore
     ≤ ~350** (a third of this baseline), not the ~240 the spec was written
     with; GPU memory +35–50%; standing triangles +15–35%. Relight ms/s
     nearly doubled (~95 → ~178) though no lighting or streaming code
     changed between the runs — unexplained; a repeat run will say whether
     it is noise.

     **Mesh ms/s cannot measure meshing cost.** The mesh pass stops at
     `MESH_TIME_MS` (4 ms, overshooting by up to one sub-batch), and
     sprinting it is always full: 575 ms/s at 115 fps is 5.0 ms a frame, as
     in M11. The number is the budget, not the work, so criterion 3's
     "or mesh ms/s halves" can never be met by a faster mesher — judge
     meshing by the second table's meshed/s and µs per chunk, and the
     backlog, instead. What the backlog says already: the same budget now
     drains ~50% fewer chunks — the full-resolution fix made each mesh
     dearer, as expected.
   - [x] **Baseline, second table (owner's run, 2026-10-31, instruments
     commit, same machine and window).** Its first table repeated the one
     above within noise (sprint 113–120 fps, backlog ending 973–1 046,
     relight 173–179 ms/s), so the relight rise is real, not noise.

     | Phase | Meshed /s | Mesh µs/chunk (CPU) | Encode ms median / p99 | Draws | Tris submitted M | Tris drawn M | Zero-area % |
     |---|---|---|---|---|---|---|---|
     | 6 levels, land, standing | 0 | 0 | 0.51 / 0.88 | 613 | 1.82 | 1.39 | 24 |
     | 6 levels, land, sprint | 1684 | 1595 | 0.89 / 1.76 | 577 | 1.64 | 1.24 | 24 |
     | 8 levels, land, standing | 0 | 0 | 0.64 / 1.13 | 682 | 2.24 | 1.66 | 26 |
     | 8 levels, land, sprint | 1626 | 1599 | 1.03 / 1.93 | 628 | 2.01 | 1.47 | 27 |
     | 6 levels, ocean, standing | 0 | 0 | 0.64 / 1.05 | 673 | 1.83 | 1.41 | 23 |
     | 6 levels, ocean, sprint | 1612 | 1690 | 0.94 / 1.76 | 570 | 1.57 | 1.19 | 24 |
     | 8 levels, ocean, standing | 0 | 0 | 0.70 / 1.19 | 735 | 2.21 | 1.65 | 25 |
     | 8 levels, ocean, sprint | 1591 | 1675 | 1.02 / 1.90 | 613 | 1.88 | 1.39 | 26 |

     Reading:
     - **Meshing:** ~1 600 chunks a second at ~1.6 ms of worker CPU each —
       2.6 s of CPU every second. The backlog holds near 1 000 for the whole
       sprint rather than growing: a steady lag, which only cheaper chunks
       (or more budget) shorten.
     - **Encode** is ~1.0 ms median at 8 levels while sprinting (p99
       ~1.9 ms), ~0.65 standing: exactly criterion 5's ~1 ms line on a fast
       CPU, so a machine four times slower would spend ~4 ms. Task 3 has a
       measured case.
     - **Draws** are 570–735, under the spec's estimate (~200 + 640–1 872):
       frustum and horizon culling already drop most LOD nodes.
     - **Zero-area triangles are 23–27% of everything submitted**, ~0.4–0.6 M
       a frame, all of it LOD (full resolution has emitted true triangles
       since `emit_sealed_rect`). Task 2's prize.
     - **These numbers are a dev build.** `cargo run` builds the workspace's
       own crates at `opt-level = 1` (dependencies at 3), and so did every
       earlier benchmark — comparable with each other, not with what a player
       runs. Headlessly (cloud container, real chunks off the land sprint
       path) a release build meshes only ~13% faster, so the mesher's cost is
       in the algorithm, not the profile.
   - [x] Lavapipe worst case (2026-11-01): see task 2's notes — taken
     after tasks 1 and 2, with the commit before task 2 as its "before".
1. ✅ **Profile meshing under streaming** (done 2026-11-01; see task 0's
   notes). Where does `mesh_chunk` spend its
   time: building `MeshInput`, the per-slice masks, AO and light corners, the
   sealed-rectangle pass, allocation? Fix in order of measured share. Likely
   candidates, to be confirmed by the profile, not assumed:
   - chunks that cannot have faces (buried, or entirely one block with
     like-for-like neighbours) skipped before any mask is built — uniform
     chunks are ~86% of the stream (CLAUDE.md lesson 6);
   - buffers reused across slices and jobs instead of allocated per call;
   - the sealed-rectangle corner set built without a hash set.
   - [x] **Uniform chunks sweep only their boundary slices** (2026-10-31).
     Profile (headless, cloud container, 1 296 real chunks along the land
     sprint path, the surface band ±2 chunks, every face neighbour present):
     915 uniform — 474 buried rock, 366 deep sea, 75 air — and 381 mixed.
     Every uniform rock and sea chunk emitted nothing, yet cost ~3.0 ms, the
     same as a surface chunk: the full 6 × 32-slice mask sweep.
     `MeshInput::build` was only ~0.2 ms of it, and a release build was only
     ~13% faster than the dev build, so the cost was the algorithm. Inside a
     uniform chunk every face is between two cells of one block, which
     `shows_face` always culls, so only each direction's outermost slice can
     show a face — 6 slices, not 192. Exact; the test
     `uniform_chunks_mesh_the_same_from_their_boundary_slices` compares it
     vertex for vertex with the full sweep against seven neighbour mixes
     (and fails if the slice is off by one).

     | Dev build (opt-level 1), µs per chunk | Before | After |
     |---|---|---|
     | Uniform rock / sea | ~3 200 | ~350 |
     | Mixed | ~3 700 | ~3 600 |
     | Mean, all 1 296 | 3 350 | **1 300 (−61%)** |

     What remains of a uniform chunk is mostly `MeshInput::build`; mixed
     chunks are now ~85% of the mesh CPU on this path and are the next
     target.

     **In game (owner's `--bench`, 2026-10-31, same machine and window),
     sprint phases, before → after:**

     | | Baseline | Boundary slices |
     |---|---|---|
     | Mesh µs per chunk (CPU) | 1 595–1 690 | **804–841 (−49%)** |
     | Chunks meshed per second | 1 591–1 684 | 2 121–2 240 (+33%) |
     | Backlog at the end, 8 levels | 1 046 / 1 028 | **414 / 454 (−58%)** |
     | fps median, 8 levels | 114 / 113 | 122 / 113 |
     | Relight ms/s | 175–179 | 172–181 |

     Standing phases, encode, draws and triangles are unchanged within
     noise, as they should be. Criterion 3 (≤ ~350) not yet met. Halving
     the cost per chunk raised throughput only a third: the workers now do
     ~1.8 s of meshing a second where they did ~2.6, so time inside the
     4 ms pass but outside the jobs (choosing candidates, uploads, rayon
     hand-off per 8-chunk sub-batch) is now a real share — to measure once
     the jobs stop shrinking.
   - [x] **A bitwise prefilter for the face sweep** (2026-10-31). Callgrind
     on the 381 mixed chunks (dev build): ~36 M instructions a chunk, ~78%
     in `mesh_chunk`'s own mask loop and the `cidx` calls it makes (not
     inlined at opt-level 1) — ~180 instructions for each of 196 608 cell
     questions, nearly all answered "no". Corner light, AO, merging and
     `emit_sealed_rect` together were under 6%; `MeshInput::build` ~22%.
     `FaceCandidates` builds rows of cells along X as bits — renders,
     occludes, and (when only one see-through block is in reach) clear —
     and finds each direction's possible faces 32 cells at a time; only
     those cells are asked `shows_face`, and slices with none are skipped.
     It can only drop cells that question rejects, so the mesh is identical:
     `prefiltered_greedy_mesh_is_identical` (random chunks, a sea over a
     seabed, water and glass meeting, random / sealed / absent neighbours,
     varying light) fails if the X shift is reversed or the one-clear-block
     rule is applied with two.

     | Dev build, µs per chunk (same 1 296 chunks) | Original | Boundary slices | + prefilter |
     |---|---|---|---|
     | Uniform rock / sea | ~3 200 | ~350 | ~320 |
     | Mixed | ~3 700 | ~3 600 | **~1 120** |
     | Mean | 3 350 | 1 300 | **554 (−83% overall)** |

     Release build: mean 396 µs, mixed 783. `MeshInput::build` (~350 µs in
     the dev build) is now most of every uniform chunk's cost and a third of
     a mixed one's.

     **In game (owner's `--bench`, 2026-10-31), sprint phases:**

     | | Baseline | Boundary slices | + prefilter |
     |---|---|---|---|
     | Mesh µs per chunk (CPU) | ~1 640 | ~820 | **540–590** |
     | Chunks meshed per second | ~1 630 | ~2 170 | 2 476–2 555 |
     | Mesh ms/s (main thread) | ~575 | ~550 | **365–391** |
     | Backlog end, 6 / 8 levels | ~1 000 / ~1 040 | ~450 / ~430 | 385–392 / 374–395 |
     | fps median, 6 / 8 levels | ~117 / ~113 | ~121 / ~118 | **645–755 / 575–616** |
     | 1% low | 51–53 | 52–55 | 57–61 |

     The mesh pass no longer fills its 4 ms budget, so the sprint frame
     rate is 5–6× what it was, close to standing. Headlessly the prefilter
     cut ~57% per chunk; in game ~30%, so in-game chunks are less like the
     sample (more re-meshes of surface chunks, which the prefilter helps
     least) — `MeshInput::build` is the obvious remaining share either way.
     Standing phases, encode, draws and triangles unchanged within noise.

     **Criterion 3 is 7–13% short (≤ ~350 asked, 374–395 measured) — but
     with the budget no longer full, the backlog may not be meshing's at
     all.** It can hold chunks still queued for relight and chunks waiting
     for a neighbour that is still being generated (the first-mesh gate),
     which no mesher speed drains. The benchmark now splits the backlog on
     each phase's last frame — ready / relight / neighbours — to say which.
   - [x] **The backlog split (owner's `--bench`, 2026-11-01).** Sprint
     phases otherwise as the prefilter run (fps 584–761, µs/chunk 545–586,
     backlog end 327–396). What the end-of-sprint backlog waits on:

     | Phase | Ready | Relight | Neighbours |
     |---|---|---|---|
     | 6 levels, land | 300 | 96 | 0 |
     | 8 levels, land | 242 | 138 | 0 |
     | 6 levels, ocean | 276 | 96 | 0 |
     | 8 levels, ocean | 184 | 143 | 0 |

     Two thirds are ready to mesh, a third waiting for relight, none for a
     neighbour (the first-mesh gate is not a bottleneck). So meshing is
     still the limit — in bursts: the pass averages ~0.6 ms a frame but
     hits its 4 ms cap whenever terrain arrives, leaving ready chunks for
     later frames. Cheaper chunks still drain it; the next fix stays in the
     mesher.
   - [x] **`MeshInput::build` row by row, and no snapshot where no face can
     be** (2026-11-01). Callgrind on the snapshot alone: a third of it was
     `Chunk::get` per cell — a runtime division and modulo by the palette
     width each time — plus a per-cell light copy and a walk over all 34³
     cells to find the shell. Now:
     - `Chunk::decode_row` / `Chunk::light_row` (vox-core) unpack an X row
       with shifts and masks; the snapshot copies rows, skips light for a
       dark chunk, and visits only the shell's cells
       (`rows_decode_exactly_what_get_reads`, every palette width;
       `snapshot_matches_per_cell_reads`, faces, edges and corners of the
       shell from all 26 neighbours, some absent);
     - an all-air chunk returns before the snapshot (it has no faces — they
       belong to the block they are on), and so does a uniform chunk its
       face neighbours hide: each touching layer all occluding or all the
       same block, a sealed side hiding, an absent one not
       (`uniform_chunk_is_hidden`; a mixed neighbour's layer is scanned —
       `a_mixed_neighbours_touching_layer_decides_whether_a_uniform_chunk_is_hidden`
       fails if the wrong layer is read or sealing is ignored).

     | µs per chunk (same 1 296 chunks) | Dev before | Dev after | Release after |
     |---|---|---|---|
     | Snapshot alone | ~356 | ~197 | — |
     | Uniform rock / sea / air | ~320 / ~320 / ~230 | **~2 / ~2 / ~0** | ~2 |
     | Mixed | ~1 120 | ~874 | ~773 |
     | Mean | 554 | **258** | 228 |

     Against the M12 baseline headlessly: 3 350 → 258 µs mean (−92%). The
     sample has every neighbour present; in game, chunks on the edge of the
     loaded region still build a snapshot.
   - [x] **Task 1 closed (owner's `--bench`, 2026-11-01).** Sprint phases:

     | | Baseline | Boundary slices | Prefilter | Snapshot fixes |
     |---|---|---|---|---|
     | Mesh µs per chunk (CPU) | ~1 640 | ~820 | ~570 | **188–195 (−88%)** |
     | Chunks meshed per second | ~1 630 | ~2 170 | ~2 500 | 2 782–2 870 |
     | Mesh ms/s (main thread) | ~575 | ~550 | ~385 | **251–262 (−56%)** |
     | Backlog end, 6 / 8 levels | ~1 000 / ~1 040 | ~450 / ~430 | ~385 | **183–184 / 208–210** |
     | Ready / relight at the end | — | — | ~250 / ~120 | 35–210 / 0–148 |
     | fps median, 6 / 8 levels | ~117 / ~113 | ~121 / ~118 | ~700 / ~600 | **735–796 / 698–705** |
     | 1% low | 51–53 | 52–55 | 57–61 | 61–64 |

     **Criterion 3 met by both its measures:** the backlog ends at ~20% of
     the baseline (a third was asked), and mesh ms/s fell by more than half
     — though, as noted above, that one measures the budget more than the
     work. Sprint-flight now runs at ~90% of standing fps. In game the
     per-chunk saving (−88%) nearly matches the headless one (−92%).
     Relight ms/s is unchanged at ~180 and is now ~40% of the main thread's
     streaming time; what remains of the backlog splits between it and
     meshing bursts. Standing phases, encode, draws and triangles are
     unchanged within noise. One standing phase (8 levels, land) has
     shown a 44–64 ms worst frame in two of four runs; the others stay
     under ~20 ms — a one-off at the phase start, to watch.
2. **LOD as true triangles** (criterion 4).
   - [x] **Fans and zipped walls share vertices, three indices per triangle**
     (2026-11-01). Each fan triangle and each zipped-wall triangle was a
     quad with a repeated corner — four vertices, six indices, half of it
     zero-area. Now a fan stores its centre and border once, a wall each
     edge's vertices once, and each triangle is three indices into them;
     plain rectangles stay two-triangle quads. Every vertex attribute is a
     function of position alone, so sharing changes nothing drawn.
     `LodMeshData::quad_count` became `triangle_count`; the tests that
     walked the mesh in groups of four (`top_rects`, the full-morph wall
     check, `the_morph_never_inverts_a_quad`) now walk triangles through the
     index buffer. Every watertightness, morph and coverage test passes
     unchanged otherwise.
   - **Not all zero-area triangles were padding.** A wall between two cells
     level at rest but not once morphed has zero height at rest and opens
     as the camera nears its ring's edge (ADR-0009) — geometry the morph
     needs. `LodMeshData::area_triangles` (which the benchmark's "drawn"
     uses) now counts a triangle with area at rest **or** at full morph; the
     earlier runs counted these walls as zero-area, so their "drawn" and
     "zero-area %" slightly overstate the padding. The criterion's test,
     `lod_submits_only_triangles_with_area` (land, sea, terraces, noise, a
     coast; strides 1–64; both passes), fails on the old emitters.
   - Headless, 576 real LOD nodes around the benchmark's start (strides
     2–256, land and sea, release build):

     | | Before | After |
     |---|---|---|
     | Triangles submitted | 4.26 M | **3.23 M (−24%)** |
     | Vertices | 8.53 M | **6.00 M (−30%)** |
     | Vertex + index bytes | 324 MB | **231 MB (−29%)** |

   - [x] **Confirmed (owner's `--bench`, 2026-11-01).** Zero-area 0% in
     every phase; drawn equals submitted. Against the task 1 run:

     | | Before | After |
     |---|---|---|
     | Tris submitted, 6 / 8 levels standing (land) | 1.82 / 2.24 M | **1.51 / 1.79 M (−17% / −20%)** |
     | GPU MB, 6 / 8 levels standing (land) | 599 / 957 | **469 / 689 (−22% / −28%)** |
     | Standing fps, land 6 / 8 levels | 875 / 774 | 886 / 791 |
     | Sprint fps, 8 levels land / ocean | 705 / 698 | 744 / 735 |
     | Encode ms median, 8 levels sprint | 0.72–0.73 | 0.69–0.70 |

     Submitted fell less than headlessly (−24%) because full-resolution
     triangles, unchanged, are in the total. "Drawn" rose (1.66 → 1.79 M at
     8 levels) only because it now counts morph-only walls (above); the
     earlier runs' 24–26% "zero-area" was ~19% padding and ~7% morph walls.
     Sprint backlog 165–237 at the end, criterion 3 still met.
   - [x] **Lavapipe, the software worst case** (cloud container,
     2026-11-01). `specks_scratch::specks` with `SPECK_FRAMES=10` renders
     the benchmark's land view at 800×600 with full resolution (radius 8)
     and times each frame including its readback, so the GPU's work is in
     it. The commit before task 2 with the same harness is the "before":

     | lavapipe, 800×600 | Before task 2 | After |
     |---|---|---|
     | Frame, 6 levels | 240 ms | **229 ms (−5%)** |
     | Frame, 8 levels | 309 ms | **257 ms (−17%)** |
     | Encode, 6 / 8 levels | 2.76 / 4.11 ms | 2.64 / 2.81 ms |
     | Triangles, 6 / 8 levels | 1.88 / 2.29 M | 1.56 / 1.84 M |
     | Draws, 6 / 8 levels | 639 / 708 | 639 / 708 |

     **Pixel-identical**: the before and after frames match in every pixel
     at both level counts, and neither has a pinhole (`hole_census`: 0) —
     criterion 6's headless half. On a software rasteriser the frame is
     ~4 fps and almost all raster: triangles, not draws, are its cost.
3. **Draw-call measurement, then batching if warranted** (criterion 5).
   - [x] **Measured; batching deferred, as the criterion says** (2026-11-01).
     At 8 levels on the owner's machine a frame's encode — culling,
     recording every pass, `submit` — is 0.66–0.79 ms median (p99
     1.2–2.0 ms) after tasks 1–2, below the ~1 ms line, for 613–735 draws.
     It was 1.03 ms median sprinting at the baseline, and fell with fewer
     index bytes and less main-thread contention, not fewer draws. On
     lavapipe the frame is ~99% raster (encode 2.8 of 257 ms). So shared
     buffers and indirect draws are not built in M12, and no ADR is needed.
     **Recorded for later:** a CPU four times slower would spend ~3 ms
     encoding — a real share of a 60 fps frame. That is when to revisit,
     with a low-end machine to measure on.
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
