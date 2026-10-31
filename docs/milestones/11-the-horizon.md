# Milestone 11 — The Horizon

**Goal:** see as far as you would in real life. A default view of **32 km**,
engineered to run at **64 km**, with Earth's real curvature so plains fall away
past the horizon while mountains rise above it from tens of kilometres out —
and a frame rate that doesn't notice.

Depends on M10 (the torus, `f64` world positions, and the sparse LOD sampler).
Climate & Biomes (M13, after M12's performance pass) benefits: a biome you can see from
30 km away is a destination.

## Changes at M10 close (2026-10-01)

This spec was written before M10 finished, and M10 took on part of it:

- **Criterion 8 (reverse-Z) is done** — ADR-0013, in M10. What remains is to
  confirm it holds at 32 and 64 km.
- **Criterion 7 (distant water) changed shape.** ADR-0011 decision 1 moved to
  (c): LOD draws its real seabed and a translucent sea surface, not a flat
  opaque top. Flat-cell merging (criterion 6) still applies to the sea surface;
  the seabed under it merges by its own heights.
- **New task 0 — why relighting doubled.** M10 closed with criterion 8 partly
  met: sprint-fly median ~130 fps against M09's ~180, with relight at
  192–253 ms/s against M09's 95–130 and meshing unchanged (M10 retrospective).
  Before the horizon spends the same budget, measure whether each relight got
  slower or more relights happen per chunk (per-chunk timing at the
  `v0.9.0-m09` tag against now; relights per loaded chunk in telemetry), and fix
  what the measurement names. Optimize from measurements, not vibes.
- **Criterion 9 (atmosphere) is wavelength-dependent.** Distant terrain should
  turn blue-violet with distance, as real mountains do through air, not fade to
  white: per-channel extinction (blue scattered most) with in-scattering toward
  the sky colour. The owner raised this independently on 2026-10-01; see
  `docs/vision.md`.

## Status and remaining work — START HERE

*Kept current at every task boundary. A new session reads this list, verifies
the remote matches it, and starts at the first unchecked item.*

### Done

- [x] **Task 0 — why relighting doubled (measured, three fixes).** A headless
  model of `stream_tick`'s relight loop (the real generator, streamer,
  heightmap and `compute_chunk_light_2ch`, with vox-app's triggers copied)
  reproduced the sprint log: ~880 ms/s of single-thread relight work, which
  over the worker pool is the ~220 ms/s `lt`. Where it went, at 120 m/s and
  radius 8 (~470 chunks loaded per second, ~1 800 relights per second):
  - **Uniform air under open sky missed the fast path** — 985 relights/s at
    ~425 us each, ~70% of the lighting function's time. The fast path refused
    any uniform chunk whose side neighbours carried sky light, which is every
    air chunk above the ground. Where 15 enters every column from above, side
    planes cannot change a cell; an all-solid chunk likewise. Both now take
    the fast path, checked against the full path in tests (418 → 31 ms/s).
  - **`top_sky` cost ~80 us per relight**: 1 024 hash-map lookups, one per
    block column. `ColumnHeights` now stores each chunk column's heights as
    one array, and `ColumnHeights::sky_top` reads a chunk's whole footprint
    with one lookup (141 → 2.5 ms/s). Semantics unchanged, unknown included.
  - **`Streamer::update` re-sampled the terrain every frame**, standing still
    included: ~245 columns × 5 elevation samples, ~0.3 ms a frame on the main
    thread, in neither `lt` nor `msh`. It now returns early when nothing has
    changed (its own field comment already promised this), and vox-app keeps
    spans in a `SpanCache` (0.32 → ~0.001 ms per still frame; 51 → 3 ms/s in
    flight). The likeliest cause of M10's ~15% lower standing fps.

  Overall: relight work 879 → 292 ms/s in the model, with every relight's
  result identical (same count, same causes). Not all of this was new in M10 —
  the fast-path miss and the per-column map predate it — but M10 streams more
  air (the surface band's upper layers and the camera window), so it paid
  them more often. No exact M09 comparison exists: the `v0.9.0-m09` tag is an
  in-progress commit and M09's completion landed together with M10's tasks
  1–4. Still open in the model, for later: 83% of relights change nothing
  (most queued as a new chunk's six neighbours), and non-uniform chunks are
  now most of the cost. To confirm in play: the M10 retro's sprint-fly and
  standing runs at radius 8, 3 levels — compare `lt`, fps and worst frame;
  and that lighting looks unchanged (caves dark, no bright or black chunks
  while flying).

- [x] **Lighting asks `opaque`, not `solid` (ADR-0011).** Found during task
  0: `light.rs` still asked the physical `solid` at its five registry
  queries (the skylight column heights, the uniform fast path, the padded
  map the flood fills read), though the M10 checklist recorded ADR-0011's
  step 1 as converting it. Invisible while every block has
  `solid == opaque`; wrong the first time one doesn't. Three tests with a
  glass block (solid, not opaque) failed on the old code — daylight under a
  glass roof, a glass chunk, the heightmap — and pass now.
  `BlockRegistry::from_types` (test-only) builds such registries.

- [x] **Task 1 — per-level centring (ADR-0014, Proposed).** Each level's
  square snaps to the next coarser level's grid instead of every level sharing
  one centre snapped to the coarsest; the partition stays exact (the
  constructor asserts the nesting condition that guarantees it). The finest
  level stays within one snap step of the camera — 128 blocks — where the
  shared centre allowed 8 km at eight levels. `LodLevel::extended` is now the
  one rule for building the menu's levels (vox-app and tests used two copies);
  unload slack is per level, at least one snap step; LOD sampling's reach and
  `closest_approach_chunks` use the level's own stride. Tests: exact
  partition for eight levels at six camera positions (every coarse snap
  phase, negative, several laps out), the fine level hugging the camera at
  the coarse grid's worst offset, no rim thrash across a coarse boundary.
  Visible change at today's three levels: small — the fine rings sit closer
  to the player and move in 4- and 8-chunk steps. To confirm in play: walk
  and fly at 3 and 5 levels; no holes or doubled terrain where levels meet,
  no new popping.

- [x] **Task 2 — more levels + view-distance clamp.** The extra-levels slider
  now reaches 5 (eight levels, strides 2–256, 64 km). Its old cap of 2 existed
  because the shared centre dragged fine radii out with every coarse level
  (2 extra measured 2 416 nodes); with per-level centring each extra level
  adds 192 nodes: 640 at 3 levels, 1 024 at 5, 1 600 at 8. A world too small
  for the asked horizon clamps it under half its width (ADR-0012 rule 3,
  `LodLevel::extra_levels_within`) — the default world takes all eight, the
  smallest stops at five — and the menu shows the clamped value. Side effect
  to expect at 4–5 levels: the fine levels keep their designed radii instead
  of being rounded out to the coarsest grid, so far fewer nodes (the 5-level
  log's `lod 2416` should read ~1 024) and less GPU memory, with fine detail
  reaching as far as at 3 levels. Flat-cell merging (task 3) is not in yet,
  so 8 levels draws every open-ocean cell. To confirm in play: step the
  slider 0 → 5, logging `lod`, GPU MB, triangles and fps at each.

- [x] **In play, 2026-10-14 (owner's log, radius 8): tasks 0–2 confirmed.**
  Standing at 3 levels: **908–1 052 fps** (M10 733–817, M09 851–967) — the
  per-frame terrain re-sampling was the standing-fps regression. LOD slider
  0 → 5: `lod` 640 / 832 / 1 024 / 1 216 / 1 408 / 1 600 exactly as predicted,
  GPU 187 / 270 / 353 / 440 / 534 / 631 MB (5 levels was 726 MB), 0.69 →
  2.20 M triangles, standing 843 → ~680 fps at 8 levels and a 64 km horizon.
  Flight at 8 levels: 126–161 fps with relight 126–163 ms/s (M10 sprint:
  192–253) on 2.3 M triangles; meshing (~490 ms/s, its whole 4 ms/frame
  budget) is now the limit. One 3-level flight (07:25–07:37) ran 215–426 fps,
  median ~340 — sprint or not is unconfirmed. Flying at 8 levels holds
  ~2 020 nodes rather than 1 600: each level keeps one snap step of slack
  (ADR-0014), which for the 256-chunk levels is a wide band. No holes,
  doubling or popping reported; "looking pretty fancy".

- [x] **Task 3 — flat-cell merging.** `mesh_lod_heightfield` merges top
  quads greedily, but ONLY across cells that do not move under morph (each
  already equals its 2x2 group minimum) and share a height: the shader takes
  each vertex's morph factor from its own distance, so a merged quad that
  morphed would tilt against the walls on its edges (ADR-0009's warning).
  Cells that morph keep their own quad; walls are unchanged. Measured on real
  terrain around spawn (49 nodes per stride): quads per node 1 588 → 787 at
  stride 2, 1 824 → 1 151 at 4, 1 997 → 1 410 at 8, 2 600 → 2 329 at 32,
  3 181 → 3 135 at 256 — about a quarter fewer LOD quads over eight levels.
  Coarse strides gain little: their cells are rarely flat, and walls are now
  most of their geometry. A flat field or open seabed is one top quad.
  Tests: every cell covered by exactly one top at its exact height and morph
  target (four fields, three strides and origins); merged quads never move
  under morph; the inversion test now includes plateaus where merged and
  single cells meet. To confirm in play: at 8 levels, triangles below the
  2.20 M above, and no sparkles or cracks along the edges of flat areas.
  Left for later: merging WALLS along runs, the bigger share at coarse
  strides, under the same morph constraint.

- [x] **Task 5 — curvature and horizon culling (ADR-0015, Proposed).**
  `curve()` in `curvature.wgsl`, appended to both terrain shaders so full
  resolution and LOD drop by the same `d²/2R` (Earth's radius, ADR-0012 §5);
  the coefficient rides in the terrain uniform's spare `fog_range.z` from
  `vox_core::curvature_coefficient`, and the menu's "Earth curvature" toggle
  zeroes it for comparison. Clip position only — fog, the full-res mask and
  all of the simulation stay flat. LOD culling now judges nodes where they
  are drawn: boxes stretched down by the drop for the frustum test, and a
  node wholly below the sight line grazing the sea-level surface skipped
  (`vox_core::below_horizon`, conservative). Horizon culling removes less than
  criterion 5 expected — 105 of 1 600 nodes from standing height at spawn,
  36 from 100 blocks up, none from 1 000 — because nearly every 8 km node
  holds ground that does show over the horizon, as the milestone intends.
  vox-render's tests now run headlessly in cloud sessions, and gained naga
  validation of every shader (it caught a deliberately broken one) plus the
  culling cases. To confirm in play: on a plain, terrain falls away past a
  few km while distant ranges stand above the horizon; toggle curvature to
  compare; no crack or shimmer where full resolution meets LOD (they share
  one function); at 32 and 64 km no z-fighting (reversed-Z, task 4).

- [x] **Task 6, part 1 — aerial perspective (ADR-0016, Proposed).** One
  `atmosphere()` in `atmosphere.wgsl`, appended to both terrain shaders like
  curvature: per-channel extinction with real sea-level Rayleigh
  (5.8 / 13.5 / 33.1 × 10⁻⁶ per block) and haze (21 × 10⁻⁶) coefficients,
  thinning with height (8 km / 1.2 km), in-scattering the horizon sky colour;
  then the existing distance fade, kept because air alone does not hide where
  terrain ends. A dark green hill is unchanged at 0.5 km, teal-grey at 10 km,
  pale blue at 32 km, nearly sky at 64 km; clearer from a summit. New "air
  density" slider (1 = Earth's air, 0 = off); the fog toggle switches the
  whole atmosphere. To confirm in play: each ridge bluer and fainter with
  distance; near terrain unchanged; no colour seam where full resolution
  meets LOD; night still fades distant terrain to dark.

- [x] **In play, 2026-10-16: curvature and aerial perspective confirmed**
  ("curvature looks great", the atmosphere "works great").
- [x] **Fix, found in play: white boxes on terrace steps at the edge of full
  resolution.** Where a step rises outward across that edge, full res draws
  no face for it (it faces a chunk that will never load), so its only face is
  an LOD wall lying exactly on the edge of the full-res mask. The LOD shader
  judged each fragment by its own position, float error sent some into the
  masked column, and those were discarded with nothing behind them but sky.
  The shader now asks a sixteenth of a block away from the camera: back faces
  are culled, so every visible wall belongs to the cell behind it.
  Reproduction test failed first (a fragment at x = 31.9999 discarded), then
  passed; `FULLRES_NUDGE` is pinned equal in Rust and WGSL by a test. To
  confirm in play: the screenshot's spot — steps crossing the edge of the
  loaded area — shows no white.

- [x] **In play, 2026-10-16: the white boxes are gone.**
- [x] **Fix, found in play: white specks scattered over distant terrain —
  T-junctions from task 3.** A merged top quad's edge ran past the corners
  of every wall and single cell beside it; rasterization does not promise to
  close such a junction, so single pixels of sky leaked through, mostly far
  away where triangles are thin. A merged region is now a FAN from its
  centre with a vertex at every cell corner along its border (each triangle
  stored as a quad with a repeated corner, keeping LOD pieces four vertices;
  the repeat's zero-area half is discarded by the GPU), or one quad per cell
  where that is fewer. Test `top_faces_have_no_t_junctions` failed first,
  then passed; coverage is now checked per triangle. Merging therefore saves
  less than task 3 reported: triangles per node 3 176 → 2 376 at stride 2,
  3 647 → 3 037 at 4, 3 994 → 3 465 at 8, 5 200 → 4 894 at 32, 6 362 → 6 017
  at 256 — about a tenth over eight levels, not a quarter (that figure
  counted rectangles that were not watertight). The `tris` telemetry counts
  the zero-area halves too, so it reads somewhat above what is drawn. To
  confirm in play: no specks in the distance. (The triangular wedges along
  one-block steps in the same session's screenshot are full resolution's
  step faces with ambient occlusion split along their diagonal — unchanged.)
- [x] **In play, 2026-10-16: the specks persist after the fan fix** (still
  far off, still when standing, gone while moving), so the top faces' T-junctions
  were not the only cause. Debug keys (`9a87f4b`): **H** paints the sky
  magenta, **J** rebuilds LOD with flat-cell merging off, **K** with skirts
  off. Owner's results, 2026-10-17: every speck turns magenta (holes onto the
  sky); J and K change nothing (skirts are needed — the white boxes return
  without them).
- [x] **Fix: T-junctions between walls.** Two walls side by side in one
  vertical plane meet along a corner line, and each ended at its own cells'
  heights, so the taller one's edge ran past the other's corners — a
  T-junction on most corner lines in rough terrain, merging or not, and
  between neighbouring skirts on node borders too. Walls are now collected
  first, every wall's top and bottom is recorded on both its corner lines,
  and each wall is emitted with a vertex at every breakpoint on its two
  vertical edges (`emit_lod_wall`, a zipper of triangles; unchanged where it
  has none). Test `no_opaque_face_has_a_t_junction` (every axis-aligned edge
  of every opaque face, merging on and off, strides 2 and 4) failed first,
  then passed. Cost in drawn triangles per node on real terrain (seed
  0x7e22ac0de): stride 2 2 817 → 2 855, 4 2 947 → 2 995, 8 3 109 → 3 181,
  32 4 403 → 4 815, 256 5 987 → 7 847 — the coarsest level's walls are the
  tallest. Known limit: added vertices morph linearly up their own wall, so
  two walls sharing one may part slightly inside a morph band while it
  animates. To confirm in play: H shows no magenta specks in the distance.
- [x] **In play, 2026-10-18: the specks remain, magenta under H.** Neither
  T-junction fix touched them, and the skirts are ruled out too (on real
  terrain no node border steps further than a skirt reaches: 0 of 10 240
  border pairs at any stride). New instrument: `Renderer::new_headless` and
  `read_frame` draw the real pipelines into a texture on any adapter —
  lavapipe in the cloud container (`apt-get install mesa-vulkan-drivers`) —
  and the ignored test `specks_scratch::specks` renders the whole LOD ring
  with a magenta sky and counts pinholes. It finds almost none: 0–11 per
  frame at 6 and 8 levels, 800×600 to 1920×1080, 40 to 2 000 blocks up,
  curvature on or off, before and after the wall fix — so the game's specks
  come from something the harness does not do (full resolution and its
  mask, node retirement, the GPU). Next: the owner presses **P** on a
  speckled view (logs a `VIEW` line: position, yaw, pitch, window, time,
  settings) and the harness renders that exact frame.

  2026-10-31: the owner's first `VIEW` (radius 8, 3 levels, 1920×1057, near
  sea level by the shore) renders in the harness with 5 pinholes, all at the
  far horizon; the screenshot that came with it was lossy WebP, which
  smeared any single-pixel speck away, so it could not be compared. **P now
  also runs `Renderer::hole_census`** on the player's own GPU: the frame is
  re-rendered offscreen with the sky magenta six ways — as drawn, without
  full-res chunks, without the full-res mask, without the LOD depth bias,
  without LOD culling, and LOD alone — and each variant's pinhole count
  (`vox_render::pinholes`) is logged as a `HOLES` line. The variant that
  drops to zero names the cause. Also seen in that screenshot, separately:
  the near sea 90–250 blocks out shows the magenta sky through the water
  (about 82, 61, 123 against blue-grey beyond) — inside the full-res
  radius, so no seabed is drawn under that water. Not yet diagnosed.
  (Owner, same day: not a bug — those rays run under the surface over a
  seabed falling ~1 in 10, faster than they sink, and leave the 2 km LOD
  horizon. Checked against the generator: −16 at 300 blocks, −117 at 1 200,
  −212 at 2 000. The real fix is water that absorbs with path length, a
  later improvement.)
- [x] **The specks found and fixed: T-junctions in the FULL-RESOLUTION
  greedy mesh.** The owner's census (2026-10-31, RTX 4060): as drawn 84;
  without the LOD bias or culling 84; without the mask 74; **LOD alone 0**.
  The distant terrain was watertight all along; the holes were in the
  full-res chunks, whose greedy rectangles' edges ran past the corners of
  the smaller faces beside them, out where full-res triangles are a pixel
  wide. Why they vanished while moving: columns still streaming are left out
  of the mask, so LOD drew underneath and filled the cracks; standing still,
  every column completes and the mask exposes them (the mask arrived in M10
  A1 — LOD used to underlap everywhere). Test
  `greedy_mesh_has_no_t_junctions` (a terraced chunk) found 1 017 and failed
  first. Fix: `mesh_chunk` collects its rectangles, then
  `emit_sealed_rect` gives each a vertex at every point of its border where
  another rectangle has a corner, and at every block corner of its border
  lying on a face of the chunk (the neighbour's corners there are unknown);
  a rectangle with none stays one quad, otherwise its border zips into
  n − 2 shared-vertex triangles (never more than naive; none degenerate;
  three indices each — the one place a `MeshData` is not all quads). Tests
  that counted quads on a lone solid chunk now use a box clear of the
  chunk's faces; the greedy-versus-naive cost check counts triangles.
  Harness with full resolution at the owner's first `VIEW`: old mesher 21
  pinholes, new 5 — the 5 the LOD alone shows on lavapipe at the far
  horizon. Cost, drawn full-res triangles over 9×9×5 real chunks: shore
  53 290 → 72 176, inland 5 524 → 16 102, hills 14 076 → 25 498 — mostly
  the chunk-face splits; tens of thousands against LOD's 0.5–2 M. To confirm
  in play: P's `HOLES as drawn` near zero, and no specks by eye.
- [x] **Benchmark mode: `cargo run -p vox-app -- --bench`.** One fixed,
  repeatable run instead of hand-timed retro runs: default settings, noon
  with time paused, eight phases — {6, 8 levels} × {land, ocean} ×
  {standing 20 s, sprint-flying 30 s along +X at 120 m/s} — each started
  only once every queue has been empty for a second (60 s timeout, reported
  if hit). The ocean start is the nearest spot whose whole 3.6 km path is
  ≥ 30 blocks under the sea. Reports per phase: median fps, 1% low, worst
  frame, relight and mesh ms/s, dirty peak and end, LOD nodes, GPU MB,
  median triangles, loaded chunks. Writes `bench/bench-<unix time>.md`
  (git-ignored), logs the same table and quits. Logic in `vox-app/src/bench.rs`
  (6 tests).

- [x] **Benchmark, 2026-10-17 (owner's run, before the wall fix):** RTX 4060,
  800×600 window, seed 0x7e22ac0de, defaults (radius 8, curvature on, air
  density 1). Every phase settled (2.2–2.3 s from standing, 1.0 s into the
  sprints); no timeouts.

  | Phase | fps median | 1% low | Worst ms | Relight ms/s | Mesh ms/s | Dirty peak / end | LOD nodes | GPU MB | Tris M |
  |---|---|---|---|---|---|---|---|---|---|
  | 6 levels, land, standing | 905 | 477 | 91.8* | 0 | 0 | 0 / 0 | 1216 | 448 | 1.36 |
  | 6 levels, land, sprint | 135 | 64 | 21.5 | 94 | 582 | 883 / 649 | 1424 | 472 | 1.58 |
  | 8 levels, land, standing | 778 | 436 | 4.3 | 0 | 0 | 0 / 0 | 1600 | 639 | 1.98 |
  | 8 levels, land, sprint | 128 | 62 | 20.4 | 94 | 574 | 896 / 658 | 1872 | 694 | 1.83 |
  | 6 levels, ocean, standing | 788 | 429 | 4.4 | 0 | 0 | 0 / 0 | 1216 | 451 | 1.73 |
  | 6 levels, ocean, sprint | 129 | 64 | 22.3 | 98 | 591 | 901 / 674 | 1424 | 454 | 1.52 |
  | 8 levels, ocean, standing | 753 | 409 | 4.8 | 0 | 0 | 0 / 0 | 1600 | 641 | 1.97 |
  | 8 levels, ocean, sprint | 126 | 62 | 22.3 | 98 | 582 | 929 / 729 | 1872 | 676 | 1.73 |

  \* The first phase's worst frame is start-up (pipelines and first uploads);
  every later standing phase's is under 5 ms. Reading: 64 km costs ~14% of
  standing fps over 32 km and ~5% while sprinting; sprint-fly holds M10's
  ~130 fps median at both. Meshing is the sprint bottleneck — ~580 ms/s of
  mesh work against ~95 of relight — and the `dirty` backlog does not drain
  during a 30 s sprint on land or sea (ends 650–730), though it empties within
  seconds of stopping. The 800×600 window flatters fps against full-screen
  play; later runs should keep the same size to compare.

- [x] **In play, 2026-10-31: the specks are gone.** "It's absolutely
  clean." Census at the owner's view (radius 8, 3 levels, 1920×1057):
  **as drawn 2** (from 84) — both at the top of the frame on the far
  horizon, and both present with LOD alone; full resolution adds none.

### Remaining, in order

Nothing — M11 closed 2026-10-31. Retrospective at the end of this file; next
is M12, `docs/milestones/12-performance-pass.md`.

## Why this milestone exists

Measured in the September 2026 audit: from random points on land, how often is a
mountain even in sight?

| View distance | Ground above 900 blocks visible from |
|---|---|
| 2 km (M10) | 10% of land |
| 8 km | 39% |
| 16 km | 67% |

The median walk to a 900-block peak is ~11 km. The terrain already has the
mountains; the player can't see them. Magnificence is watching a range grow on
the horizon for half an hour of walking, and that same view is what keeps travel
from being a slog — you are always walking *toward* something.

## What "real life" means here

On a flat world only the atmosphere limits sight: 30–50 km on a clear day. With
**Earth's radius** (6 371 km) and one block per metre, curvature gives exactly
real horizons:

| Eye height | Horizon |
|---|---|
| Standing (1.62 blocks) | ~4.5 km |
| On a 500-block hill | ~80 km |
| A 1 150-block summit, seen from the ground | above the horizon from ~120 km |

So from the ground, plains sink away beyond a few kilometres — as in life —
while high ground stays visible from far beyond the render distance.

Curvature is **visual only**. Physics, collision, the raycast and every
distance in the simulation stay flat. ADR-0012 records why the curvature radius
deliberately does not match the torus's circumference: a consistent radius for a
204.8 km world is ~32.6 km, which would put the horizon at ~330 blocks.

## Acceptance scene

Spawn on a lowland plain. The near ground is full resolution; beyond it the land
rolls away and drops below the horizon a few kilometres out. Above that line,
far off, a mountain range stands pale-blue with distance. Walk toward it for
twenty minutes: it grows steadily, foothills lift over the horizon, and at no
point does the landscape visibly swap detail or pop.

Climb a 500-block hill and look back. The plain you crossed is laid out below,
fading to haze tens of kilometres away, with the coastline and the sea beyond
it. Frame rate stays in the M10 envelope throughout, on the machine the M10
numbers were taken on.

## Acceptance criteria

1. **Per-level ring centring.** Each LOD level snaps its own centre to its own
   node grid, instead of all levels sharing one centre snapped to the coarsest
   grid. At 64 km the coarsest cell is 8 km, and a shared centre could sit
   kilometres from the player. The exact, gapless partition between levels
   (ADR-0008) is preserved. The rings stay in the player's **unwrapped** frame
   and must not canonicalise anything — the world's seam lives only in
   generation, saves and the edit overlay (ADR-0012 §4), so a correct ring
   needs no seam handling at all. Headless: every chunk column within the
   horizon is covered by exactly one level, at many camera positions including
   ones several laps from the origin.
2. **Levels to 64 km.** Eight levels (strides 2–256 chunks), ~1 600 nodes. Node
   alignment relative to the world's seam does not matter: nodes sample
   periodic terrain in the unwrapped frame (ADR-0012 §4).
3. **View distance clamps to less than half the world's period** (ADR-0012
   rule 3), so no terrain is ever visible twice around the world.
4. **Earth-radius curvature** in both terrain shaders — `shader.wgsl` and
   `lod.wgsl` — computed identically. The two must agree exactly or the
   full-resolution/LOD boundary tears. Headless where possible: the curvature
   function is shared and tested.
5. **Horizon culling.** A LOD node lying entirely below the curvature horizon
   for the current eye height is not drawn. From the ground this should remove
   most of the far ring.
6. **Flat-cell merging** in `mesh_lod_heightfield`. Adjacent cells with equal
   heights *and equal morph targets* merge into one quad. A node of open ocean
   becomes a handful of quads instead of 1 024. The morph-target condition is
   not optional — ADR-0009 records that merging on height alone reintroduces the
   see-through holes fixed in M09.
7. **Distant water.** LOD cells below sea level draw a flat sea-level top, per
   ADR-0011 decision 1, and merge per criterion 6.
8. **Reverse-Z depth.** Standard depth loses nearly all precision past a few
   kilometres; distant ranges would z-fight. Reverse the depth range (f32 depth,
   near maps to 1). The LOD depth bias (ADR-0008) inverts sign under reverse-Z
   and must be re-derived, not just flipped.
9. **Atmosphere.** Fog is retuned for tens of kilometres: distant terrain fades
   toward sky colour so range reads as depth. Aerial perspective, not a wall.
10. **Performance.** Against M10's final numbers on the same machine: steady fps
    within the envelope, LOD backlog returning to 0 after a ring update, GPU
    memory and triangle counts reported. Report numbers at 32 km and 64 km.
11. **Bookkeeping.** ADR for per-level centring (supersedes ADR-0008's shared
    snapped centre) and one for curvature and depth. Retro with numbers;
    CLAUDE.md status.

## Tasks

1. **Per-level centring (`vox-core`, headless).** The ring redesign, in the
   unwrapped frame, with partition tests at many positions. Everything else
   depends on this, so it lands first and alone.
2. **More levels + view-distance clamp.** Extend to eight levels behind the
   existing settings slider; clamp to half the period.
3. **Flat-cell merging (`vox-mesh`, headless).** With morph-target agreement
   and the existing inversion test extended to merged quads. Includes distant
   water.
4. **Reverse-Z (`vox-render`).** Re-derive the LOD depth bias.
5. **Curvature + horizon culling.** Shared curvature function, both shaders,
   CPU-side culling.
6. **Atmosphere retune + performance pass + retro.**

## Non-goals

- **No climate, biomes or vegetation** — M13. The long view will be green; it
  is judged on distance, stability and frame rate, not colour.
- **No true sphere.** The world stays a torus with visual curvature (ADR-0012).
- **No world-size selection UI.** Sizes stay a constant; the engine just has to
  be correct for any legal size.
- **No streaming of full-resolution terrain beyond the current load radius.**
  The horizon is LOD all the way out.

## Notes for whoever builds this

- **Judge it on foot.** Spectator sprint is 21× ground sprint. The acceptance
  scene is a walk.
- **The two shaders must curve identically.** Put the curvature formula in one
  place and derive both from it. Any drift shows up as a crack exactly at the
  full-resolution boundary, which is the one place the player is always
  looking.
- **Geomorph gets smoother for free.** With per-level centres snapped to each
  level's own (fine) grid, the handover boundaries move in small steps instead
  of 256-block jumps. Re-evaluate the residual pop ADR-0009 describes once
  criterion 1 lands; the time-driven morph it proposes as a fallback may become
  unnecessary.
- **Low-end hardware is pillar 1.** 64 km must be an option that runs well, but
  the *default* should be chosen by what an ordinary machine sustains, and
  lower-end defaults need deciding before release.

## Open questions for the owner

- **Default view distance by hardware.** 32 km is the target on the development
  machine. Whether weaker hardware defaults lower, and how that is detected or
  chosen, is a release-time decision — noted here so it isn't forgotten.

# Retrospective (2026-10-31)

## What shipped

- **Task 0 — why relighting doubled in M10.** A headless model of the live
  relight loop found three causes: uniform air under open sky missed the fast
  path; `top_sky` cost a hash lookup per block column; and the streamer
  re-sampled terrain every frame, standing still included. Relight work in
  the model 879 → 292 ms/s with identical results. Standing fps recovered
  from M10's 733–817 to **908–1 052**.
- **Lighting asks `opaque`** (ADR-0011's rule, missed in `light.rs`).
- **Per-level ring centring (ADR-0014)** and **eight levels to 64 km**, with
  the half-period clamp: the finest level stays within 128 blocks of the
  camera where the shared centre allowed 8 km.
- **Flat-cell merging** in LOD — open ocean is a handful of fans, not 1 024
  quads. Watertight merging saves about a tenth of LOD triangles, not the
  quarter first claimed (that figure counted rectangles that leaked).
- **Earth curvature and horizon culling (ADR-0015).** One `curve()` in both
  terrain shaders. Horizon culling removes little from the ground (105 of
  1 600 nodes), as the ADR records.
- **Aerial perspective (ADR-0016).** Real Rayleigh and haze coefficients per
  colour channel, thinning with height, in front of the distance fade —
  distant ridges turn blue, as asked in `docs/vision.md` §1.5.
- **Instruments that outlive the milestone:** `--bench` (a fixed eight-phase
  performance script writing a results table); `Renderer::new_headless` and
  `read_frame` (the real renderer drawing offscreen on any adapter, lavapipe
  included); the **P** key's `VIEW` line and `hole_census`, which re-renders
  the player's frame six ways on their own GPU and counts pinholes.

## Criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Per-level ring centring | Met (ADR-0014) |
| 2 | Levels to 64 km | Met — eight levels, 1 600 nodes standing, 1 872 in flight |
| 3 | View distance under half the period | Met |
| 4 | Curvature, both shaders identical | Met — one shared function |
| 5 | Horizon culling | Met, small payoff (105/1 600 from the ground) |
| 6 | Flat-cell merging | Met, ~10% of LOD triangles |
| 7 | Distant water | Met (shipped with ADR-0011 1c in M10, merged here) |
| 8 | Reverse-Z | Met in M10 (ADR-0013) |
| 9 | Atmosphere | Met (ADR-0016) |
| 10 | Performance, 32 and 64 km | Met — table below |
| 11 | Bookkeeping | ADRs 0014–0016 accepted; this retro |

## Numbers (owner's RTX 4060, `--bench`, 2026-10-17, 800×600)

| | 32 km (6 levels) | 64 km (8 levels) |
|---|---|---|
| Standing fps, land / ocean | 905 / 788 | 778 / 753 |
| Sprint-fly fps, land / ocean | 135 / 129 | 128 / 126 |
| LOD nodes standing / sprinting | 1 216 / 1 424 | 1 600 / 1 872 |
| GPU memory | 448–472 MB | 639–694 MB |

Sprint-flight holds M10's ~130 fps median out to 64 km; 64 km costs ~14% of
standing fps over 32 km and ~5% while sprinting. Taken before the
full-resolution T-junction fix, which adds full-res triangles (real chunks,
9×9×5: shore 53 k → 72 k, inland 5.5 k → 16 k, hills 14 k → 25 k) — to be
benchmarked at the start of M12.

## Bugs found in play

- **White boxes** where terrace steps cross the edge of full resolution: the
  LOD mask judged walls lying exactly on its edge by float error. Fixed by
  asking a sixteenth of a block away from the camera.
- **The white specks** — the milestone's long tail. Single pixels of sky,
  far off, only while standing still. Two fixes, each with a failing test,
  removed real T-junctions from the **LOD** mesher (merged tops, then walls
  and skirts) and changed nothing in play; the skirts were ruled out by
  measurement. The headless harness then showed LOD almost watertight, so
  the game had to differ — and the in-game census settled it in one run:
  as drawn 84, **LOD alone 0**. The holes were T-junctions in the
  **full-resolution** greedy mesh, exposed only since M10 A1's mask stopped
  LOD drawing underneath (which is also why they vanished while moving:
  columns still streaming fall outside the mask). Fixed by
  `emit_sealed_rect`; census 84 → 2.
- **Purple sea in the census screenshot** — not a bug: those view rays pass
  under the surface over a seabed falling faster than they sink and leave the
  2 km horizon. Water that absorbs with depth is in `docs/vision.md` §10.

## Known, carried forward

- **Sprint backlog does not drain** at 120 m/s (`dirty` ends at 650–730
  after 30 s); meshing is ~580 ms/s against relight's ~95. **M12.**
- **Zero-area padding in LOD** fans and walls — submitted triangles exceed
  drawn. **M12.**
- **Two LOD pinholes at the far horizon** in the owner's census (also 0–11
  per frame on lavapipe, all far out). Not seen by eye; left for M12's
  census runs.
- Split wall vertices morph linearly up their own wall, so two walls sharing
  one may part slightly inside a morph band while it animates.

## Lessons

1. **A failing test proves a bug exists, not that it is the bug the player
   sees.** Both speck fixes followed CLAUDE.md lesson 1 and were real, but in
   the wrong mesher. The question to ask before fixing is "does this test
   reproduce *the player's* symptom?" — and for a rendering symptom only the
   player's frame can answer it.
2. **Measure on the player's machine, by elimination.** The census — the
   same frame rendered with one ingredient removed at a time, counted on the
   owner's GPU — found in one keypress what three rounds of theory missed.
   Instruments that run where the bug lives beat reasoning about it.
3. **Lossy images cannot carry single-pixel evidence.** Screenshots reached
   the sandbox as WebP; a one-pixel speck does not survive. Count in the
   game, log the numbers.
4. **A headless renderer turns "Nathan is the only visual check" into "Nathan
   is the final visual check."** Lavapipe ran the real pipelines in the cloud
   container and reproduced the in-game result (21 → 5 pinholes) before the
   fix shipped.

