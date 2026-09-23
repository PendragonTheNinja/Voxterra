# Milestone 09 — LOD Levels & Streaming Quality

**Goal:** make the M08 horizon *good*. Three thrusts, in priority order:
(1) **priority-ordered, budgeted streaming** so nearby work always beats
distant work — fixing the transient unlit ("black") chunks and the sprint-fly
throughput dip from the M08 retro; (2) a **multi-level LOD octree** so coarse
resolution recedes with distance instead of standing beside the player;
(3) an **exact near ring** downsampled from real chunks so the full-res↔LOD
boundary matches instead of underlap-and-overdraw. Geomorph transitions are a
stretch goal, not a commitment.

M08 proved the pipeline with one level and the simplest boundary; M09 is the
quality pass ADR-0008 planned for it. Everything here builds on shipped,
tested machinery — no new architecture questions.

## Acceptance scene

Sprint-fly across the mountains. The terrain immediately around you is always
full-res and always lit — no dark unlit chunks appearing nearby while distant
work churns, because near work is served first. Ahead, resolution falls off in
steps you have to *look for*: fine coarse terrain near the full-res edge,
coarser beyond, coarsest at the horizon — no more 8-block cubes adjacent to
real terrain. Stop at the full-res boundary and look at the seam: the first
LOD ring matches the real terrain's silhouette exactly (it was downsampled
from it), not a sunken approximation. Watch a region cross the boundary as you
fly: the swap is unobtrusive (hysteresis; geomorph if the stretch lands). The
telemetry shows the dirty/relight backlogs draining nearest-first and the fps
floor during sprint-fly no lower than M08's ~90.

## Acceptance criteria

1. **Priority-ordered streaming (the fix that matters most).** Relight, mesh,
   full-res gen, and LOD queues drain **nearest-camera-first** (today's
   arbitrary HashSet order is the root of distant-black-chunk persistence).
   Budgets remain time-capped. Concretely testable: (a) headless — batch
   selection returns positions sorted by camera distance; (b) in-game — a
   freshly streamed area near the camera never shows an unlit chunk for more
   than a moment, even with a deep distant backlog; standing still drains
   `dirty` to 0 with no black chunk surviving.
2. **Unlit-chunk convergence hole closed.** A fresh chunk whose first relight
   computes "unchanged" (all-dark, neighbors absent) must not be meshed dark
   and forgotten: track first-light state so a chunk is meshed only after a
   relight that ran with all six neighbors resident OR is re-queued when they
   arrive (the existing requeue), with near-first ordering making the heal
   immediate. No black chunks under normal streaming.
3. **Multi-level LOD.** At least three levels (proposal: strides 2, 4, 8 in
   concentric rings, tunable), each ring one octree level coarser, reusing
   `generate_lod_node`/`mesh_lod_node` unchanged (both are already
   stride-generic). `LodRing` generalizes to per-level radii with the same
   disjoint-partition and hysteresis guarantees, headlessly tested per level
   and across level boundaries (no gap, no overlap between rings).
4. **Exact near ring.** The innermost LOD ring is **downsampled from resident
   full-res chunks** (majority/topmost rule per coarse cell) instead of
   re-sampled from the seed, so the boundary silhouette matches the real
   terrain. Falls back to seed-sampling when the source chunks aren't resident
   (e.g. teleport). Headless test: downsampled node's surface equals the
   full-res surface within one fine-stride everywhere.
5. **Per-level transition stability.** Hysteresis at every ring boundary (as
   M08 had at one); crossing a boundary produces no thrash and no double
   terrain at any level pair. The depth-bias overlap trick remains only at the
   full-res edge; between LOD levels the partition is exact.
6. **Performance.** With three levels and the same ~1 km+ horizon: steady fps
   within the M07/M08 envelope (~144 normal, sprint-fly floor ≥ 90), LOD
   streaming still budgeted and off-thread, and the M08 dirty-backlog plateau
   (~2000) measurably reduced by prioritization (report before/after).
7. **Bookkeeping.** Retro with numbers; CLAUDE.md status; note in ADR-0008
   that the octree + near-ring halves are now implemented.

## Amendments (owner-approved, added mid-milestone)

Criteria 1–5 were met with voxel-grid LOD nodes, but testing showed the result
still read as obviously-not-terrain. Two corrections and three additions were
approved rather than deferred, on the grounds that the milestone's goal is
"make the horizon good", not "ship the planned tasks".

- **A1 — LOD is a heightfield, not a voxel grid (design correction).** ADR-0008
  had LOD nodes reuse the 32³ `Chunk` and the greedy mesher. That reuse was
  elegant and shipped fast, but a voxel grid quantizes *height* to the cell
  size, so distant terrain rendered as stacked terraces of tall slabs — the
  visual ceiling three rounds of stride tuning could not lift. LOD now stores
  exact per-column heights and meshes a top quad plus walls down to lower
  neighbours (`vox_mesh::mesh_lod_heightfield`): horizontal detail is still
  quantized, vertical detail is exact. This supersedes the "scaled chunk"
  claim in ADR-0008.
- **A2 — Distance fog.** Terrain fades toward the sky as it recedes. Not
  decoration: it is what makes a detail transition unreadable, by dropping
  contrast before the LOD change becomes visible. Its absence was the single
  largest remaining reason the falloff was obvious.
- **A3 — Settings menu (ESC).** Live sliders for render distance, LOD level
  radii, fog, and time of day. Every visual decision in this milestone cost a
  recompile-and-squint cycle; live tuning collapses that to seconds and is the
  tool the remaining work needs.
- **A4 — Geomorph transitions** (was the optional stretch, now committed):
  lerp heights across a band at ring boundaries so level swaps are invisible.
  Attempt after A2 and A3, since fog may make the steps much less visible and
  the sliders make tuning the band cheap.

## Notes on cost, for tuning

Each level holds roughly the same node count regardless of stride, and each
node costs about the same, while every level covers **twice** the radius of the
one before. View distance therefore scales *logarithmically* in cost — adding a
coarse level is cheap. What is expensive is fine detail near the camera (the
stride-2 ring is the memory hog). Tune accordingly: reach for another coarse
level before widening a fine one.

A large unclaimed optimization: the heightfield emits one quad per column even
across flat ground. Greedy-merging equal-height neighbours would collapse flat
regions substantially.

## Non-goals

- No worldgen changes (the mountains amendment stands as-is).
- No block light / caves at distance; LOD stays skylight-only surface.
- No persistence of LOD nodes (regenerate/downsample on demand).
- No new crate.

## Tasks

1. **Priority ordering (`vox-app` + a small `vox-core` helper; mostly
   headless).** A distance-keyed batch selector (pure function: set +
   camera chunk + N → nearest N, headlessly tested) used by the relight, mesh,
   gen, and LOD spawn drains. Add the first-light gate from criterion 2.
2. **Multi-level `LodRing` (`vox-core`, headless).** Generalize to a list of
   (stride, inner, outer, unload) levels with exact inter-level partition;
   truth-table tests per boundary.
3. **Near-ring downsample (`vox-core` or `vox-worldgen`, headless).**
   `downsample_chunks_to_node`: 2×2×2-per-level reduction from resident
   chunks with the surface-match test from criterion 4; seed fallback.
4. **Wiring (`vox-app`/`vox-render`, review-only).** Drive levels, route the
   near ring through the downsampler, per-level spans/AABBs (the render path
   is already span-generic). Debug: per-level tint toggle to *see* the rings
   while tuning.
5. **Tune + retro.** Level radii/strides, skirt depths per level, backlog
   before/after numbers, fps; retrospective; close-out. Stretch: geomorph.

## Notes for whoever builds this

- **Do task 1 first and alone.** It fixes the user-visible black chunks and
  the fps floor *before* any octree complexity lands on top, and every later
  task benefits from ordered queues. Resist starting the octree first because
  it is more interesting.
- The M08 named trap (boundary double-draw/gaps) now applies at **every**
  level boundary; the exact inter-level partition in task 2's tests is the
  guard. Only the full-res edge keeps the depth-bias overlap.
- `generate_lod_node` and `mesh_lod_node` are already stride-generic — if a
  task seems to need changes to them beyond parameters, re-read ADR-0008;
  the octree was designed to reuse them as-is.
- Per-level constants live where M08 put the single level's (top of
  `vox-app/main.rs`); keep them together.
- Read the M08 retro's "Known limitations" — criteria 1–4 map to it 1:1.

---

# Retrospective (2026-08-25)

## What shipped

**Streaming (criteria 1–2).** A distance-keyed batch selector in `vox-core`
drains the relight, mesh, gen and LOD queues **nearest-camera-first**, replacing
arbitrary `HashSet` order. Paired with a first-light gate so a chunk whose first
relight ran without all six neighbours is not meshed dark and forgotten. Between
them these closed the black-chunk defect carried since M08 and cut the relight
bill by roughly 3×.

**Multi-level LOD (criteria 3–5).** `LodRing` generalized to per-level
(stride, outer) pairs — shipped as strides 2/4/8 at 512/1024/2048 blocks — with
an exact inter-level partition and per-level hysteresis, headlessly tested per
boundary. The near ring reads real column heights where they are known and falls
back to seed sampling elsewhere.

**A1 — heightfield LOD (design correction).** Criteria 1–5 were met with
voxel-grid nodes and the result still read as obviously-not-terrain: a voxel
grid quantizes *height* to the cell size, so distant terrain came out as stacked
terraces. LOD now stores exact per-column heights and meshes a top quad plus
walls down to lower neighbours. Horizontal detail is quantized; vertical detail
is exact. Supersedes the "LOD node = scaled chunk" claim in ADR-0008.

**A2 — distance fog.** The single largest remaining reason the falloff was
obvious. Fog drops contrast before a detail change becomes readable.

**A3 — settings menu (ESC).** Live sliders for render distance, LOD radii, fog,
geomorph band, and time of day. Every visual decision in this milestone
previously cost a recompile-and-squint cycle. This paid for itself repeatedly
and is the reason the geomorph bugs below were findable at all.

**A4 — geomorph (ADR-0009).** Each LOD vertex carries the Y it takes at the next
coarser level, and the vertex shader lerps toward it so a handover swaps
geometry for geometry that already matches. The enabling property: strides nest
2:1 and every level takes the *minimum* surface over its cell, and minimum is
associative — so a coarse cell is exactly the minimum of the four fine cells
inside it, computable locally with no extra sampling.

**Crosshair.** A centre dot, drawn through the existing egui pass rather than a
dedicated wgpu pipeline.

## Numbers (owner's machine, 3 levels, load radius 8, ~1 km horizon)

| | M08 | M09 |
|---|---|---|
| Stationary, drained | — | **851–967 fps**, worst frame 3.3 ms |
| Sprint-fly, per-second | ~92–97 fps | **127–564 fps**, median ~180, min 127 |
| Sprint-fly, worst frame | — | 16–33 ms typical, one 61.5 ms outlier |
| Dirty backlog | ~2000, **plateaued** | **0–410, repeatedly drains to 0** |
| Relight | ~390 ms/s | **95–130 ms/s** |
| Mesh | ~500 ms/s | 500–590 ms/s (unchanged) |
| LOD backlog | — | **0 in every sample** |

- LOD nodes resident: 640 at spawn, peak 864, settling ~752. GPU 309 MB idle,
  380–434 MB in flight. 0.7–1.4 M triangles drawn of ~700–850 meshes.
- Criterion 6's fps envelope is **met on the per-second average** (floor 127 vs
  the ≥ 90 target) and **not met on worst-frame** — a 26–33 ms frame is 30–38 fps
  instantaneously, and one outlier hit 61.5 ms. Reporting both rather than
  picking the flattering reading.
- The backlog result is the headline: M08's ~2000 was a *plateau* that never
  drained during flight. M09 peaks at 410 and returns to 0 repeatedly, so the
  queue is keeping up rather than falling behind.
- Headless tests at close: vox-core 190, vox-mesh 35, vox-worldgen 12.

## Bugs found and fixed along the way

Every one of these was found by playing the build, not by reading the code.

1. **Ghost blocks I — phantom nodes.** Both LOD invalidation paths pushed a node
   id into the pending queue for *every* level unconditionally. Level 1's
   annulus starts 16 chunks out, so breaking a block queued a stride-4 and a
   stride-8 node **at the player's feet**, built from seed heights, on top of the
   full-res terrain — where suppression can't reach it (its footprint fails the
   whole-footprint-inside-radius pre-filter) and nothing unloads it until the
   camera travels 256 blocks. Now only nodes the ring already wanted are
   re-requested.
2. **Ghost blocks II — coarse levels never saw edits.** Real heights were gated
   behind `if n.level == 0`, so strides 4 and 8 were built from the seed forever.
   Fixed with `EditedColumns`, a sparse chunk-bucketed overlay applied at every
   level. Sparse because the dense alternative is 32×32×stride² lookups per node
   — 65 536 at stride 8, nearly all misses on columns no chunk has loaded.
3. **White speckling across the LOD.** The morph lowers each cell to its *own*
   2×2 group minimum, so the two ends of a wall sink at different rates: a
   neighbour that starts level or higher can finish well below (a face never
   emitted), and an existing wall's top can sink beneath its bottom (winding
   flips, back-face culling deletes it). Both are see-through holes that widen
   with the band. The mesher now emits a wall if the cells differ at *either* end
   of the morph, and the invariant is tested directly.
4. **White flash on ring re-centre.** The unload loop dropped ~150 meshes in the
   frame the ring re-centred while replacements took many frames to build. Nodes
   are now *retired* rather than deleted — a node's geometry depends only on its
   own id and the terrain, never the camera, so a retired mesh stays correct and
   can keep drawing until its replacement lands (or be adopted back unchanged).
5. **Geomorph silently disabled by the floating origin.** Each node's morph
   completion distance rides in the offset uniform's `.w`, previously commented
   "unused padding" — and `set_render_origin` rewrote that uniform with `.w = 0`.
   The render origin moves every chunk crossing, so morphing switched itself off
   across the whole world within 32 blocks of walking.
6. **Geomorph measured from the wrong reference.** ADR-0009 originally measured
   morph distance from the ring's *snapped centre*, on the reasoning that this is
   where the annuli are measured from, so `t` would reach 1.0 exactly at the
   handover. That ignored time: the snapped centre is frozen between ring
   updates, then teleports 256 blocks at the instant of the swap. `t` never
   animated — it was a step function firing simultaneously with the thing it
   existed to hide. Now measured from the camera. **A reference frame that is
   exact but quantised is useless for smoothing.**

## Process notes

- **Only the owner can see the renderer.** `vox-render` and `vox-app` cannot be
  compiled in the sandbox — not for missing system libraries, but because Cargo
  1.75 cannot parse the winit/wgpu dependency tree, so no amount of installing
  reaches them. Three separate handoff failures this milestone came from
  changing one side of a contract that crosses that boundary. The mechanical
  checks that replace the compiler are now written down in
  `docs/notes/clippy-lints.md`: grep the repo for every changed shared type, run
  `cargo build --workspace --all-targets` and expect empty output, and audit
  bind-group `visibility` flags against every shader stage that reads them.
- **Net totals hide gaps.** The white-flash investigation was misdirected for
  two rounds by reading a LOD mesh *total* that rose (+48 loaded, −24 dropped)
  while coverage genuinely had a hole. Instrument the thing that is wrong, not
  the aggregate that contains it.
- **The owner's observations beat the assistant's theories, repeatedly.** "It
  scales with the band slider" and "it's the sky behind the chunks" each cut
  straight to a root cause that reasoning from the code had missed.

## Known limitations → M10

1. **Edits are invisible to coarse LOD across sessions.** `EditedColumns` is
   session-local, so reopening a world and approaching an old dig site from far
   away shows pre-edit coarse terrain until the chunk loads. Closing this means
   reading edit metadata from storage, which brushes M09's "no persistence of
   LOD nodes" non-goal.
2. **Full-res meshing is the remaining hitch.** 500–590 ms/s during flight,
   unchanged from M08, and the source of the 26–33 ms worst frames. Relight was
   fixed by prioritisation; meshing was not, because the cost is throughput, not
   ordering. The unclaimed greedy-merge optimisation (collapsing equal-height
   neighbours) is the obvious lever — note that it must preserve morph targets
   as well as heights, or it reintroduces bug 3 above.
3. **The horizon is uniformly green.** Distant terrain is hard to *evaluate*,
   let alone enjoy, because there is nothing for the eye to catch. Height-based
   tint was considered and deliberately deferred rather than shipped as a
   placeholder: altitude banding is what real biome data produces, and any ramp
   tuned against the current placeholder relief ([-59, +108]) is throwaway. This
   belongs to worldgen, applied to both pipelines from shared code.
4. **Level radii and skirt depths were never deliberately tuned** — 512/1024/2048
   with strides 2/4/8 is the original proposal, kept because it looked right in
   play. Tuned by inspection, not by measurement.
5. **Residual pop at handover.** Camera-relative morph cannot complete exactly at
   a swap radius measured from the snapped centre. If a wide band does not hide
   it, ADR-0009 records the next step: a per-node factor animated over *time*
   when the ring retires a node, riding on the retirement machinery from bug 4.

## Next

**M10 — worldgen.** Real terrain: geology, climate, biomes. It is also what
makes the LOD work above finally judgeable, and where limitation 3 gets its
proper answer.
