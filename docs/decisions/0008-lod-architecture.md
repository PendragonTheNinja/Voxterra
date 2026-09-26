# ADR-0008: LOD architecture — seed-driven far terrain over a chunk octree

- **Status:** Accepted (M08 shipped 2026-07-09). The single-level slice
  validated the architecture in production: seed-driven far nodes, skirts,
  skylight-only lighting composed with M07's `sky_scale`, hysteresis, disjoint
  partition. One amendment vs. the original sketch: coarse cell classification
  **rounds down** (topmost solid cube entirely at-or-below the surface) so LOD
  hides beneath full-res at the boundary rather than poking through. M09
  implements the multi-level octree, the near-ring downsample, and streaming
  prioritization; see `docs/milestones/08-lod-single-level.md` (retro) and
  `docs/milestones/09-lod-octree-streaming.md`.
- **Context:** Performance is constitutional pillar #1 — "very high render
  distances via a built-in LOD system (Distant Horizons-style), playable on
  low-end hardware." The constitution fixes the shape ("LOD is an octree over
  chunks; full-resolution chunks at the leaves, downsampled voxel data at higher
  nodes") but not the three questions that actually decide whether it works:
  where far terrain's data comes from, how it's lit, and how the boundaries are
  hidden. This ADR answers those three, grounded in the current worldgen,
  ADR-0005 skylight, ADR-0002 floating origin, and the M07 day/night work.

## The one enabling property (why this is tractable now)

`vox_worldgen::Generator::surface_height(wx, wz)` is a **pure, cheap,
resolution-independent** function of world column — no chunk, no neighbors, no
"below." So terrain can be sampled at *any* stride without generating full-res
chunks. A proof-of-concept (headless, against the real worldgen) built a coarse
32³ node representing a 256³-block region (= 512 full-res chunks) by sampling
`surface_height` at stride 8:

- coarse node from seed: **~235 µs**
- generating the same region full-res: **~45,700 µs**
- **~195× cheaper**, and the coarse node **meshed with the existing greedy
  mesher unchanged** (1454 quads), carrying the normal M07 day/night vertex
  channels.

That result drives the whole design: **an LOD node is just a 32³ `Chunk` at a
coarser scale**, generated directly from the seed. This means LOD reuses the
entire existing pipeline — `Chunk` storage, the greedy mesher, the sky/block
vertex format, the day/night shader — instead of introducing a parallel one.

## Decision

### Structure: a chunk octree of "scaled chunks", streamed in concentric rings

- The near field (a full-resolution radius around the camera) is real 32³
  chunks — the leaves — exactly as today.
- Beyond it, concentric **LOD rings**, each one octree level coarser. A node at
  level `k` is a 32³ `Chunk` whose every cell represents a `2^k`-block cube
  (`k=0` = a real chunk). Ring `k` covers `2^k`× the linear span of ring `k-1`
  for the same 32³ node cost.
- Streaming reuses the existing `Streamer` radius/`update()`/`mark_loaded`
  pattern, once per level, with each level's load/unload radii. The existing
  rayon gen+mesh pipeline (the "async pipeline" candidate from the M04 retro)
  generates and meshes LOD nodes off-thread the same way it does chunks.

### Q1 — Far-chunk data source: **seed-driven for the far field, downsample for the near ring**

- **Far field (beyond the full-res radius):** generate each LOD node **directly
  from the seed** at the node's stride, as the PoC does. Never generate the
  underlying full-res chunks; never require having visited or stored the region.
  This is the only approach that reaches Distant-Horizons distances on low-end
  hardware (pillar #1) — you cannot afford to generate-then-downsample millions
  of unseen chunks.
- **Near ring (the full-res↔LOD boundary):** the first LOD ring is
  **downsampled from the real generated chunks** you already have loaded, so the
  seam matches the full-res geometry exactly rather than a re-sampled
  approximation of it. This is the constitution's "downsampled voxel data at
  higher nodes," applied precisely where correctness at the boundary matters.

**The forward constraint this creates (record loudly):** seed-driven far-gen
works only while worldgen exposes a **cheap, coarse-samplable** surface/density
query. Today's `surface_height` does. The future geology pipeline (two-stage:
low-res planetary sim → local detail) **must** keep a fast coarse query — i.e.
stage-1 maps must be directly evaluable at LOD strides without running stage-2
detail. If real geology ever makes the coarse query expensive, far-LOD collapses.
This is a hard requirement the geology milestone inherits from this ADR.

### Q2 — Lighting at distance: **skylight-only, from the heightmap, × the same `sky_scale`**

LOD nodes do **not** run 3D light propagation (unaffordable at distance, and
pillar #1 forbids it). Instead:

- A distant surfel is, by construction, at or near the top of its column, so by
  ADR-0005's rule it is **full skylight (15)** — no propagation needed. LOD
  lighting is therefore skylight exposure derived from the surface itself
  (orientation + open-sky assumption), with **no block light** (torches are
  invisible at kilometers) and **no cave detail** (unresolvable at LOD).
- Crucially, LOD nodes carry the **same sky/block vertex channels** as near
  chunks and are dimmed by the **same `sky_scale` uniform** the M07 day/night
  system already feeds the chunk shader. So day/night "just works" across the
  entire view — near torches hold their pools of light, the far horizon fades
  and brightens with the sun — with **no separate lighting path** and automatic
  near↔far continuity (both sides share the heightmap and `sky_scale`; only the
  resolution of the skylight estimate differs).

This answer is a direct dividend of doing day/night (M07) before LOD: the
two-channel vertex format and `sky_scale` uniform are exactly what LOD needs.

### Q3 — Seams: skirts for cracks, hysteresis (later geomorph) for pops

- **Cracks / T-junctions** where a coarse node edge meets a subdivided finer
  edge: hide with **skirts** — short vertical aprons dropped from each node's
  border, the standard Distant-Horizons approach. Cheap overdraw, no geometric
  stitching, robust. Chosen over stitching (correct but complex) for simplicity;
  the wasted fill is negligible at distance.
- **Popping** when a region switches LOD level: use a **hysteresis band** on the
  switch distance (distinct load-in vs drop-out radii — the same split the
  existing `Streamer` already uses for load/unload) so levels don't thrash at
  the boundary. **Geomorphing** (lerping vertex heights between adjacent levels
  across a transition band) is the eventual polish for invisible transitions,
  deferred to a later milestone and marked as such — skirts + hysteresis ship
  first.
- **Floating origin (ADR-0002):** LOD nodes use the same render-origin-relative
  f32 positions as chunks, via the same per-node offset uniform. Precision
  demands are *lower* per-vertex at LOD (coarse, sparse geometry), so this is
  strictly easier than the near field. The M07 sky pass is origin-independent
  (it reconstructs ray directions), so it is unaffected by any of this.

## Consequences

- **Massive reuse, little new surface area.** `Chunk`, the greedy mesher, the
  vertex format, and the day/night shader are all reused as-is. The genuinely
  new pieces are: (a) a coarse-node generator (sample surface/profile at stride
  — the PoC is the seed of it), (b) an octree/ring streamer wrapping the
  existing `Streamer`, (c) skirts in the mesher, (d) a small LOD-lighting path
  (skylight-from-surface). No new render pipeline, no second mesher.
- **A concrete cross-milestone constraint on geology** (Q1's forward
  requirement) is now written down, so the geology milestone can't accidentally
  design away far-LOD.
- **Proposed milestone split** (this is very likely two milestones, not one):
  - **M08 — single-level LOD, end to end.** One coarse ring beyond the full-res
    radius, seed-driven, skirts, skylight-only lighting composed with M07's
    `sky_scale`, hysteresis on the switch. Goal: prove the whole pipeline and
    *see a real horizon*. Deliberately one level, so the octree bookkeeping
    doesn't bury the core proof.
  - **M09 — full octree + polish.** Multiple LOD levels, the near-ring
    downsample-from-real-chunks path, geomorph transitions, and the async
    generation budgeting to keep far-gen off the frame thread at scale.
- **Left as tuning, not decided here:** full-res radius, per-level radii and
  level count, skirt depth, hysteresis width, coarse-cell classification rule
  (the PoC's mid-cell surface test is a placeholder). These need the visual
  scene, like all such knobs.
- **Open question flagged for M08 spec:** the coarse-cell *classification* (how a
  `2^k`-block cube becomes one block id) is trivial for the current
  grass/dirt/stone profile but becomes a real design question under geology
  (which rock type represents a mixed cube? majority? topmost visible?). M08
  can use a simple rule; the geology milestone revisits it.

## Status of evidence

The seed-driven far-gen bet and the "LOD node = scaled chunk reuses the mesher"
claim are validated by a headless PoC against the real worldgen (numbers above).
The lighting and seam decisions are design choices grounded in ADR-0005 and
established LOD practice, not yet prototyped — M08 is where they get built and
proven.

---

## Implementation status (2026-08-25, after M09)

Both halves this ADR deferred to M09 are now implemented, with two claims
superseded.

**Implemented:**

- **The octree half.** Multiple concentric levels (strides 2/4/8 at
  512/1024/2048 blocks), with an exact inter-level partition and per-level
  hysteresis in `vox_core::lod::LodRing`. The depth-bias overlap trick remains
  *only* at the full-res edge, as designed; between LOD levels the partition is
  exact.
- **The near-ring half.** The innermost ring uses real column heights where they
  are known, falling back to seed sampling elsewhere.
- **Async budgeting.** LOD generation and meshing run on the rayon pool behind a
  per-frame spawn budget, drained nearest-camera-first. LOD backlog measured 0
  in every telemetry sample of the M09 close-out run.

**Superseded:**

- **"An LOD node is a scaled chunk, reusing the greedy mesher."** Wrong, and
  wrong for a reason worth recording: a voxel grid quantizes *height* to the
  cell size, so distant terrain rendered as stacked terraces that three rounds
  of stride tuning could not lift. M09 amendment A1 replaced it with a
  heightfield — exact per-column heights, meshed as a top quad plus walls down
  to lower neighbours. Horizontal detail is quantized; vertical detail is exact.
  The reuse was elegant and shipped fast; it was also the visual ceiling.
- **The coarse-cell classification question** (which block id represents a mixed
  cube) is moot under a heightfield — there are no mixed cubes, only column
  heights. The *geology* version of the question survives: which surface
  material represents a coarse cell once rock types vary. That returns with
  worldgen.

**Still open, as flagged here:** per-level radii, level count, and skirt depths
remain tuning knobs. M09 shipped the original proposal unchanged because it
looked right in play — tuned by inspection, not by measurement.

Geomorph transitions, listed here as an M09 item, are specified separately in
ADR-0009.

---

## Amendment (M10 A3, 2026-09-26): exact only where LOD can meet full resolution

**The contract as it stood.** Every LOD cell takes the minimum real surface over
its footprint, so coarse terrain never rises above real ground. It exists for
one reason: LOD underlaps the full-resolution region, and a coarse cell above
the real surface pokes up through real ground there.

**What it cost.** Honouring it exactly means reading every column of every cell:
`1024 × stride²` surface samples per node. The sampler capped this at 8 probes
per axis, which still meant 65 536 samples per node at stride 8 and beyond —
the `lod +N` burst frames and the ~60 ms spikes that fail M10 criterion 8.

**And the cap had already broken it.** Beyond stride 8, 8 probes per axis is no
longer every column. The contract test only ever checked stride 8; extended to
stride 16 it fails by a block. Levels 3 and up had never honoured it.

### Decision

Two samplings (`vox_worldgen::LodSampling`), chosen per level:

- **Exact** — every column. For a level that can come within reach of
  full-resolution terrain.
- **Sparse** — a 4×4 grid per cell, 16 samples at any stride, a quarter of the
  old cost at stride 8 and beyond. For every other level. At strides of 4 or
  less the grid is every column, so sparse and exact agree there.

**"Can come within reach"** is `LodRing::closest_approach_chunks(level)` — the
nearest any node of the level can be to the camera, `inner − S + 1` chunks for
coarsest stride `S` (0 for the innermost level) — against the full-resolution
region's reach: the streamer's unload radius plus one coarsest stride. The extra
stride covers retired nodes, which stay drawn until their replacements land
(ADR-0009), by which time the camera may be one snap step nearer. The bound is
proven tight by a property test sweeping every camera position over two snap
periods.

The choice is **per level, never per node**. A node's geometry stays a function
of its id and the terrain alone, so camera motion never forces a rebuild. A
settings change rebuilds the ring, and the choice with it.

**Why nothing is lost.** Where full resolution is absent, a coarse cell a block
above a dip it did not sample is simply the terrain's shape at distance — there
is no real ground for it to poke through. The harm the contract prevents needs
real ground present.

### Consequences

- **Geomorph (ADR-0009).** Its enabling property — a coarse cell is exactly the
  minimum of the four finer cells under it — relied on both levels being exact.
  A sparse level's samples are a subset of its finer neighbour's (the lattices
  nest), so the coarse cell is at least that minimum rather than equal to it. At
  full morph a finer node can therefore sit below the coarser node by whatever
  dip the coarse lattice missed, and the handover lifts the surface by that
  much: a few blocks, at the kilometre ranges where sparse levels live. Levels
  that meet full resolution are exact on both sides of every boundary they
  share with each other, so the near field is unchanged.
- **Retiree overlap (ADR-0009).** "The finer node wins the depth test wherever
  they differ" now holds only between exact levels. Between sparse levels the
  coarse surface can sit a block or two above the finer one in places, for the
  frames both are drawn.
- **Tests.** `exact_lod_heightfield_never_exceeds_real_terrain` (strides 8 and
  16; fails on the old sampler at 16),
  `sparse_is_exact_at_fine_strides_and_never_below_it_beyond`, and the two
  `closest_approach_is_a_tight_bound_*` tests in `vox-core::lod`.
- **Forward constraint for M11.** The horizon adds coarser levels, all sparse,
  at a flat per-node cost — that is the point. If M11's curvature or a new ring
  layout changes how close a level can come, `closest_approach_chunks` is the
  one place that must change with it.
