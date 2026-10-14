# ADR-0014: Per-level LOD ring centring

- **Status:** Proposed (M11 task 1, 2026-10-14). Accepted once the owner has
  confirmed it in play; until then the code is on the M11 branch.
- **Context:** M11 (The Horizon) needs eight LOD levels, strides 2–256 chunks,
  out to 64 km. Supersedes the shared snapped centre of ADR-0008's
  multi-level ring (as implemented in M09, `vox_core::lod`). The partition
  stays exact; only where each level is centred changes.

## Context

Since M09 every level's annulus was measured from ONE centre: the camera's
chunk floored to the **coarsest** stride. Sharing it is what made the
partition exact — every boundary radius was a multiple of the coarsest stride,
so every boundary fell on every level's grid.

The cost is the offset. The shared centre can sit up to a coarsest stride from
the camera, and every level inherits that: with three levels (coarsest 8
chunks) it was 256 blocks, tolerable. With M11's eight (coarsest 256 chunks)
it is **8 km** — the finest LOD ring, meant to sit just outside the
full-resolution region, could be centred 8 km from the player, and every
radius had to be a multiple of 256 chunks. Everything that keys off a level's
position paid it too: LOD sampling chose exact sampling for any level within
reach of full resolution *plus one coarsest stride*, which at eight levels is
every level; and the geomorph band had to absorb a swap that arrives up to a
coarsest stride early or late (ADR-0009 notes).

## Decision

**Each level has its own square, snapped to the next coarser level's grid.**

- Level `i` owns `Q_i`: half-width `outer_i` chunks around the camera's chunk
  floored to `snap_i` — the stride of level `i+1`, or its own stride for the
  coarsest level.
- Level `i`'s nodes are those wholly inside `Q_i` and wholly outside
  `Q_{i-1}` (for level 0, the `lod_inner_chunks` square, snapped to level 0's
  stride; empty when 0).

**Why the partition is exact.** `Q_i`'s edges lie on `snap_i`, which is level
`i+1`'s stride and a multiple of level `i`'s — so a node of either level is
wholly inside or wholly outside it, never half-owned. And `Q_{i-1} ⊂ Q_i` from
every camera position, given the one condition the constructor asserts:

    outer_i - outer_{i-1} >= snap_i - snap_{i-1}

— the most the two floored centres can differ. A column is then covered by
exactly the first level whose square holds it.

**Constraints, replacing "every radius a multiple of the coarsest stride":**
`inner_0` a multiple of level 0's stride; each `outer_i` a multiple of
`snap_i`; the nesting condition above. `LodLevel::extended` builds the
menu's configurations (base levels plus appended doubling levels) onto that
grid, so the menu and the tests share one rule.

**Hysteresis is per level:** unload slack is the configured margin but never
less than the level's snap step, so a camera standing on a snap boundary does
not reload the far rim each time it crosses. The handover between levels stays
crisp, as before: a node inside the finer square is dropped at once.

**What moves with it:**

- `closest_approach_chunks(level)` is `inner - stride(level) + 1`: the inner
  edge now snaps to the level's own stride, not the coarsest.
- vox-app's LOD sampling reach is the unload radius plus
  `inner_snap_chunks(level)` (the level's own stride), not the coarsest, so
  sparse sampling (ADR-0008 M10 amendment) still applies to the far levels at
  eight levels.
- `morph_end_blocks` is unchanged — the outer radius — but is now measured
  from a centre within one `snap_i` of the camera, so the geomorph band
  (ADR-0009) has less offset to absorb.

## Consequences

- Level `i` is never more than one `snap_i` from the camera: the finest ring
  within 4 chunks (128 blocks) of where it belongs whatever the horizon, where
  the shared centre allowed 8 km at eight levels.
- Radii are free to be as fine as their own grids allow, not forced up to
  256-chunk multiples.
- A single step can move several squares at once (crossing a coarse grid line
  crosses every finer one), so the worst update is about what the shared
  centre's every update was; ordinary steps move only the fine levels.
- The coarsest level still snaps to its own stride, so the far rim moves in
  256-chunk steps at eight levels — the same as before.

## Tests

`vox_core::lod`: the partition is exact — every column covered once — for
eight levels at six camera positions: every snap phase of the coarsest grid,
negative coordinates, and positions several laps from the origin (the ring
works in the unwrapped frame, ADR-0012 §4; it canonicalises nothing). The
finest level stays within one snap step of a camera at the coarse grid's worst
offset (a camera at x = 255 chunks, where the shared centre put it 255 chunks
off). Stepping back and forth across a coarse snap boundary moves nothing at
the far rim. The closest-approach bound is attained and never undercut. Every
level count the menu can reach constructs.
