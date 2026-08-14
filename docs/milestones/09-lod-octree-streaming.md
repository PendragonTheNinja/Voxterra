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

## Stretch (explicitly optional)

- **Geomorph transitions:** lerp vertex heights across a band at ring
  boundaries so level swaps are invisible. Attempt only after criteria 1–6
  are green; if it slips, record it as the M10 candidate.

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
