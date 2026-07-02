# Milestone 06 — Smooth Lighting & Ambient Occlusion

**Goal:** turn M05's *correct* lighting into *beautiful* lighting. Per-vertex
smooth light (gradients across faces instead of flat per-face banding) and
Minecraft-style vertex ambient occlusion (soft darkening in creases, corners,
and under overhangs), with no seams or artifacts at chunk boundaries.

**Acceptance scene:** stand at a cave mouth at midday. The cave interior
falls off in smooth gradients rather than flat-shaded steps; every crease,
corner, and block junction shows soft AO shading; the terrain reads as having
depth. Fly along chunk boundaries: no visible seams, no lighting
discontinuities, no diagonal "butterfly" triangles on AO gradients. Sprint-fly
streaming still holds the M05 baseline feel.

## Acceptance criteria

1. Per-vertex smooth light: each face vertex is the average of the 4
   face-plane-adjacent cells' light (occluded diagonal cells excluded from the
   average per the standard rule). Flat banding is gone on lit gradients.
2. Vertex AO: classic 3-neighbor rule per vertex — `side1 && side2 → 0`, else
   `3 - (side1 + side2 + corner)` — mapped to a tunable darkening factor.
3. Quad triangulation flips based on AO/light comparison across the diagonal
   (`a00 + a11 vs a01 + a10`) so interpolation never produces the butterfly
   anisotropy artifact.
4. Chunk-boundary continuity: vertices on chunk borders sample identical
   values from either side (via the ADR-0006 snapshot). Explicit seam tests.
5. Greedy meshing keys on (block, per-vertex light+AO); output is correct
   first, merge ratio measured second.
6. Performance: mesh bench (yardstick pattern) before/after; sprint-fly burst
   floor stays ≥ 90 fps at radius 8 on the dev machine; steady 144.
7. Ambient floor decision resolved and recorded (the deferred 0.06 question —
   AO makes the low end visible, so tune both together).

## Tasks

1. **Mesh input snapshot (ADR-0006).** `vox-mesh` gains a `MeshInput` padded
   34³ snapshot (solidity bits, packed light, block id) + a builder the app
   fills from the chunk + up to 26 neighbors. Port `is_air_at`/`light_at` to
   it; behavior identical to today (per-face light). Add a mesh bench
   yardstick BEFORE the visual work so every later task has a number.
   App side: extend the border-change re-dirty rule to edge neighbors per the
   ADR. Everything headlessly testable.
2. **Per-vertex smooth light.** 4-cell vertex sampling with the occluded-
   corner exclusion rule; vertex format grows a per-vertex light value;
   greedy mask extended to the vertex tuple. Seam-continuity tests (two
   chunks, gradient across the boundary, assert identical border vertex
   values from both sides).
3. **Vertex AO + quad flip.** The 3-neighbor rule, AO strength constant,
   diagonal flip. Tests: corner block → expected AO pattern; flip chosen
   correctly on an asymmetric case.
4. **Shader/vertex pipeline (vox-app/vox-render, review-only in sandbox).**
   Per-vertex brightness attribute replaces per-face; `light_curve` moves or
   is applied per-vertex; wgsl interpolation. Nathan is the compile check.
5. **Polish + baseline.** Tune AO strength and the ambient floor together in
   real scenes (cave mouth, overhang, room interior); re-run mesh + relight
   benches and the sprint-fly log; retrospective with numbers.

## Notes for whoever builds this (human or model)

- Read CLAUDE.md's invariants + lessons first. This milestone lives in the
  exact territory the M05 saga mapped: cross-chunk sampling, stale-mesh
  dependencies, hot inner loops. Failing-test-first applies to visual math
  too — the AO rule and vertex sampling are pure functions; test them as
  tables before any rendering.
- The two known traps, named up front: (a) **seams** — border vertices must
  sample the same cells from both sides, which is the whole point of the
  snapshot; write the two-chunk seam test before the sampling code. (b)
  **anisotropy** — the quad flip is not optional; implement it with AO from
  day one.
- Keep sky and block light as one scalar per vertex for now (`max`, as
  today). Two-channel vertex light is a day/night-milestone concern; note it,
  don't build it.
- Mesh cost will rise (more vertices where merges break, snapshot gather).
  That's acceptable if measured and bounded — the M05 streaming pipeline
  (defer-until-lit, per-face requeues, small batches) is what keeps it
  invisible. If the mesh bench regresses badly, the greedy mask tuple is the
  first suspect (consider quantizing AO into the light value).
- When accepted: update CLAUDE.md status, retrospective with numbers,
  ambient-floor decision recorded, next-milestone candidates re-listed
  (LOD/far-view + async pipeline remains the headline candidate).
