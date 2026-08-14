# Milestone 08 — Single-Level LOD (a real horizon)

**Goal:** terrain stops ending at the full-resolution radius. Beyond it, one
ring of coarse, seed-generated terrain extends the view to a real horizon —
lit by the same day/night system as the near field, its cracks hidden, its
transitions stable. This is the first, deliberately-single level of the LOD
system whose architecture is set in ADR-0008; the full octree and polish are
M09.

This milestone proves the whole LOD pipeline end to end on the simplest case:
**one** coarse level. The central bet — that far terrain can be generated
directly from the seed and meshed by the existing greedy mesher — is already
validated by a headless PoC (~195× cheaper than full-res generation, meshes
unchanged). M08 turns that into something you can see.

## Acceptance scene

Stand in the full-res world and look to the horizon. Instead of terrain ending
at a hard edge into empty sky (today's behavior at the render-distance boundary),
coarse hills roll on to a distant horizon. Fly toward them: a coarse node
resolves into full-resolution terrain as you approach, and full-res drops back
to coarse behind you — **without** popping flicker, without cracks of sky
showing through the seam, without z-fighting shimmer where the two meet. Watch a
sunset: the far terrain dims, warms, and darkens in lockstep with the near
terrain (they share `sky_scale`), and at night the distant hills are dark
silhouettes under the stars. Turn in a circle at the full-res/LOD boundary and
see no flickering ring of chunks loading and unloading.

## Acceptance criteria

1. **Coarse nodes are generated directly from the seed.** A single LOD level at
   a fixed stride `S` (proposal: `S = 8`, so one 32³ coarse node represents a
   `(32·S)³` = 256³-block region). Generation samples `surface_height` at the
   stride and classifies cells by the vertical profile — it never generates the
   underlying full-res chunks. Deterministic (same seed+node → same node).
2. **Coarse nodes carry skylight, so day/night just works.** The coarse
   generator sets the sky light channel by ADR-0005's rule at coarse resolution
   (cells at/above the coarse surface = 15, below = 0; block light = 0). The
   node then meshes with the **existing greedy mesher** and renders through the
   **existing chunk shader**, so the M07 `sky_scale` uniform dims it with the
   near field automatically. No second lighting path, no second pipeline.
3. **Skirts hide cracks.** Coarse node meshes emit downward skirt aprons at
   their outer edges, so gaps between adjacent LOD nodes and at the full-res↔LOD
   boundary show terrain-colored skirt instead of see-through sky. No visible
   cracks in the acceptance scene.
4. **Full-res and LOD regions are disjoint — no double terrain.** A world column
   is drawn by full-res chunks OR by an LOD node, never both. No z-fighting, no
   double-brightness, no coarse terrain poking through near ground.
5. **Transitions are stable (hysteresis).** LOD nodes load at one radius and
   unload at a larger one; the full-res↔LOD swap uses the same load-in/drop-out
   split the `Streamer` already uses. Standing at the boundary produces no
   load/unload thrash; moving across it produces no rapid flip-flop.
6. **A real horizon, at a real distance.** Effective render distance with LOD on
   is several times the full-res radius (proposal: full-res ~256 blocks → LOD
   horizon ≥ ~1 km) at a frame rate that stays within the M07 envelope
   (~144 fps normal; sprint-fly floor ≥ 90). LOD node generation and meshing run
   **off the frame thread** (reuse the existing rayon gen/mesh pipeline); a burst
   of new LOD nodes must not stall the frame.
7. **Bookkeeping.** Retrospective with numbers (LOD gen cost, mesh cost incl.
   skirts, node counts, frame rate at the new render distance); CLAUDE.md status
   updated; ADR-0008 moved to accepted; M09 candidates (full octree, geomorph,
   downsample-from-real-chunks near ring) re-listed.

## Non-goals (deferred to M09 or later, per ADR-0008)

- **No multiple LOD levels / no octree.** Exactly one coarse level. The octree
  and concentric rings are M09; building them now would bury the core proof.
- **No geomorphing.** Level transitions are hidden by skirts + hysteresis, not
  by vertex morphing. Some pop at the swap is acceptable for M08.
- **No downsample-from-real-chunks near ring.** M08 generates the boundary ring
  from the seed too; a slight mismatch against full-res at the seam is
  acceptable (skirts hide it). The exact-match downsample path is M09.
- **No block light or caves at distance.** LOD is skylight-lit surface only.
- **No new crate.** LOD logic fits existing crates (see Tasks); do not add
  `vox-lod` for a single level.
- **No worldgen changes.** M08 consumes today's `surface_height`/profile as-is.
  (The forward constraint that real geology must keep a cheap coarse query is
  recorded in ADR-0008; it is not M08's job.)
  **Amendment (task 5, owner-approved):** one exception — the placeholder
  terrain gained a cubed mountain octave (peaks ~+108, valleys ~-59) because
  the old ~50-block relief made LOD vistas untestable. Still placeholder; real
  geology remains a future milestone.

## Tasks

1. **Coarse node generation (`vox-worldgen`, headless).** `generate_lod_node`:
   given a node position and stride, sample `surface_height` at the stride,
   classify each coarse cell (air / grass / dirt / stone by the same profile as
   `generate_chunk`), and set the sky light channel by the coarse heightmap
   rule. Returns a 32³ `Chunk` usable by the mesher unchanged. Tests:
   determinism; sampled columns match `surface_height`; sky light is 15 on the
   coarse surface and 0 well below it; all-air / all-solid nodes handled. The
   headless PoC is the starting point.
2. **Skirts in the mesher (`vox-mesh`, headless).** Add a skirt option to the
   mesher (a parameter or a sibling entry point) that, after the normal greedy
   pass, emits downward apron quads along the node's four outer edges (and the
   bottom if needed), of a configurable depth. Skirts carry the edge cells'
   light so they blend. Tests: skirt quads present only on outer edges, correct
   orientation/depth, absent on a normal (non-LOD) mesh; the differential oracle
   and seam tests still pass for the non-skirt path. Bench the added cost.
3. **LOD ring selection (`vox-core`, headless).** A pure function: given the
   camera chunk position, the full-res radius, the LOD radius, and the node
   stride, return the set of LOD node positions to have loaded — **excluding**
   any node region that overlaps the full-res area — with hysteresis (load vs
   unload radii). Truth-table tests: correct node set at sample positions; the
   full-res exclusion leaves no overlap; hysteresis band prevents flip-flop
   across the boundary.
4. **LOD render + streaming wiring (`vox-render` / `vox-app`, review-only —
   owner compiles).** Render: a coarse node's mesh is built with vertex
   positions already scaled to block units (cell × stride), so the **existing
   chunk pipeline renders it unchanged** with just its floating-origin offset —
   no shader/scale-uniform change if scale is baked at mesh time (preferred; fall
   back to a per-node scale uniform only if needed). App: drive task 3's node
   set, generate+mesh+upload LOD nodes through the existing rayon pipeline
   off-thread, keep full-res and LOD disjoint (task 3's exclusion), and add a
   debug key to toggle LOD on/off and one to force-show the seam. `uv` staying in
   cell units (texture stretched across each coarse cell) is fine for M08 —
   distant terrain reads as lower detail by design.
5. **Polish + retrospective.** Tune LOD radius, stride, and skirt depth in the
   acceptance scene; confirm day/night on the horizon, disjoint regions, and no
   thrash; measure LOD gen/mesh cost and frame rate at the new render distance;
   write the retrospective; update CLAUDE.md; accept ADR-0008.

## Notes for whoever builds this (human or model)

- **The named trap: the full-res↔LOD boundary.** This is where the milestone
  succeeds or fails. Two failure modes: (a) full-res and LOD both draw the same
  region → z-fighting/double terrain (fix: strict disjoint regions, task 3's
  exclusion), and (b) a gap of sky between the last full-res chunk and the first
  coarse node → cracks (fix: skirts). Get these two right before tuning anything
  visual.
- **An LOD node is just a scaled 32³ `Chunk`.** Resist inventing a parallel
  representation. It flows through `Chunk` storage, the greedy mesher, the
  two-channel vertex format, and the day/night shader unchanged — that reuse is
  the whole architectural win (ADR-0008). The only genuinely new code is coarse
  generation, skirts, ring selection, and a little streaming glue.
- **Bake the scale at mesh time**, not into the day/night or camera path. A
  coarse node's vertices in block units (cell × stride) render through the
  existing pipeline with only the normal per-chunk offset. Keep `sky_scale` and
  the celestial uniforms exactly as M07 left them.
- **Lighting composes for free** because LOD nodes carry the same sky/block
  channels and are dimmed by the same `sky_scale`. Do not add an LOD-specific
  lighting path; if you find yourself writing one, the node isn't carrying its
  sky channel correctly.
- **Keep generation off the frame thread.** The M07 retro's sprint-fly dip was
  mesh/stream throughput; LOD adds more of both. Reuse the rayon pipeline and
  budget node work per frame so a burst of new nodes never stalls rendering.
- Read ADR-0008 (the three resolved questions and the milestone rationale) and
  the CLAUDE.md lighting/streaming invariants before starting. Write failing
  tests first for tasks 1–3 (all headless).

---

# Retrospective (2026-07-09)

M08 shipped a real horizon: one coarse, seed-generated LOD ring extends the
view ~1 km beyond the full-res radius, lit by the same day/night system, with
skirts hiding cracks and hysteresis keeping transitions stable. All criteria
met; the visual quality limits hit are exactly the ones the spec accepted and
deferred to M09.

## What shipped

- **`vox-worldgen::generate_lod_node`** — coarse 32³ nodes sampled directly
  from the seed (never generating the underlying 512 full-res chunks), with
  skylight baked (heightfield ⇒ every air cell = 15) so the M07 `sky_scale`
  uniform lights distant terrain with no separate lighting path. Cell
  classification **rounds down** (topmost solid cube entirely at-or-below the
  surface) so coarse terrain hides BENEATH full-res at the boundary instead of
  poking through — a real bug found in review and fixed.
- **`vox-mesh::mesh_lod_node`** — meshes a node with sides/bottom culled,
  emits downward skirt aprons per border column, and bakes the cell→block
  scale into positions AND UVs (texture tiles per block, not stretched per
  cell).
- **`vox-core::lod`** — `LodRing`: pure ring policy with a disjoint full-res
  partition at node granularity and outer hysteresis; mirrors `Streamer`.
- **App/render wiring** — LOD nodes stream through a pending queue at a
  per-frame budget on the rayon pool; rendered by a depth-biased clone of the
  chunk pipeline (full-res wins overlaps) with per-node frustum culling; `L`
  toggles; telemetry gained `lod N`.
- **Task 5 amendment (owner-approved):** placeholder terrain gained a cubed
  mountain octave — relief grew from ~50 to ~167 blocks ([-59, +108],
  verified over 8 km²) so LOD vistas are actually testable. Wiping stale
  `world/` saves after worldgen changes is required (note for the future
  persistence milestone: save a worldgen version).

## Numbers

- LOD PoC → production: a 256³ region as one coarse node, ~195× cheaper than
  full-res generation; production integration ~1368 quads/node, sky-lit.
- Owner's machine, LOD on (~90–98 nodes resident), heavy sprint-fly with the
  new mountainous terrain: **~92–97 fps** with full-res streaming saturated
  (dirty backlog ~2000, mesh budget pegged at ~500 ms/frame, relight
  ~390 ms). LOD adds no measurable frame cost; the bottleneck is (and was,
  per the M07 retro) full-res mesh/relight throughput during fast travel.
- Headless tests at close: vox-core 161, vox-mesh 28, vox-worldgen 13.

## Bugs found and fixed along the way

- Per-frame spawn budget silently DROPPED un-spawned ring nodes (ring
  requested once, only 2 spawned) → sparse floating islands. Fixed with a
  pending queue + cancellation.
- Coarse cells rounded UP (surface-through-cube) → LOD poked up through
  full-res at the boundary. Fixed by rounding down (review catch).
- In-flight LOD meshes for since-unloaded nodes were uploaded. Fixed (drain
  keys on the in-flight set).
- `env_logger::init()` with no `RUST_LOG` silenced all telemetry; now
  defaults to `vox_app=info`.

## Known limitations → M09 (all anticipated by ADR-0008)

1. **Chunky near LOD** — one level at stride 8 puts 8-block cubes adjacent to
   the full-res edge. M09: multi-level octree (fine near, coarse far).
2. **Coarse visible beside the player** — the underlap-and-overdraw boundary
   shows coarse terrain wherever full-res lags or ends. M09: near ring
   downsampled exactly from real chunks + geomorph transitions.
3. **Transient unlit ("black") chunks during heavy streaming** — a fresh
   chunk relit before its side neighbors exist can compute all-dark =
   unchanged, get meshed dark, and heal only when neighbor arrivals requeue
   it — seconds later under backlog, glaring at ambient 0.004. Root cause
   traced; fix is M09's async budgeting + **priority (nearest-first) queue
   ordering** (today's HashSet drain order is arbitrary).
4. Full-res mesh/relight throughput saturates during sprint-fly (the fps dip
   to ~90) — same M09 budgeting item.

## Next

**M09 — LOD levels & streaming quality:** multi-level octree, exact near-ring
downsample, priority-ordered budgeted streaming (fixes 3 & 4), geomorph as
stretch. Spec: `docs/milestones/09-lod-octree-streaming.md`.
