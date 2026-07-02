# ADR-0006: Padded light/solidity snapshot as the mesher's input

**Status:** accepted (M06 kickoff, 2026-07-02)

## Context

Smooth lighting and ambient occlusion are **per-vertex**: each face vertex
samples the light of the 4 cells adjacent to it in the face plane, and its AO
term from the 3 cells diagonal to it (two edges + one corner). For vertices on
a chunk's boundary, those samples reach into neighbor chunks — including
**edge and corner** neighbors, not just the 6 face neighbors the mesher's
`ChunkNeighbors` currently carries. Extending `ChunkNeighbors` to 26 optional
chunk references would push "which chunk owns this cell" arithmetic into every
per-vertex sample in the hottest meshing loops, and every sample would pay
palette/nibble decodes — the exact per-visit-decode cost we just removed from
the relight engine (M05 retro: 3.2x).

## Decision

Before meshing a chunk, build one **padded 34³ snapshot** of exactly what the
mesher needs — per cell: solidity (1 bit), packed light (u8, sky high nibble /
block low), and block id (u16, for face texturing/merging) — center from the
chunk itself, the 1-cell shell from up to 26 neighbors (absent neighbor cells
read as air / light 0, same standalone behavior as today). The mesher then
runs entirely against flat arrays with `x + y*34 + z*34²` indexing: no
chunk-boundary branching, no palette decodes, in the inner loops.

This deliberately mirrors the relight engine's `PaddedChunk`/`padded_solidity`
design (M05): pay one bounded gather up front, make the hot loop branch-free.
The snapshot is built by the app (which owns the world map and can reach all
26 neighbors) and handed to `vox-mesh`, keeping vox-mesh free of world/HashMap
knowledge and headlessly testable with hand-built snapshots.

## Consequences

- **Remesh dependencies widen.** A chunk's mesh now depends on border light of
  edge/corner neighbors too. The per-face border-change masks (M05) requeue
  face neighbors; for diagonals, adopt the conservative rule: when a face bit
  is set, also re-dirty the 4 edge neighbors sharing that face. Corner-only
  changes are a strict subset of some face change on the diagonal path in
  practice; if corner staleness is ever observed, widen further. (Invariant
  lesson from M05: stale-mesh dependencies must be requeued explicitly.)
- **Greedy merging must key on vertex data.** Faces merge only when block id
  AND all four per-vertex (light, AO) tuples match. Merge ratio will drop on
  gradients; measure, don't assume (bench yardstick pattern).
- The snapshot adds a per-mesh gather cost (~40k cells + 26 neighbor border
  reads). Measured against removing per-sample decodes from ~10-60k vertex
  samples, this is expected to be net-neutral-to-positive; verify with the
  mesh bench added in task 1.
- Absent-diagonal behavior (light 0) can darken border vertices until the
  neighbor loads; the M05 defer/requeue pipeline already re-meshes on border
  change, so this self-heals the same way face borders do.
