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

## Deferred to the day/night milestone (captured here so it isn't lost)

The ambient floor (`light_curve_f`'s `AMBIENT`, set to 0.035 in task 5) is the
brightness of a cell that receives **no light at all** — sealed cave, overhang
underside. It is pure geometry (sky cannot reach the cell) and is therefore
*constant across time of day*: a cave is equally dark at noon and midnight.

Distinct from that is **night-under-open-sky**: a cell that *does* see the sky,
where the sky itself is dim. That is a time-of-day scaling of the SKY light
channel, not the ambient floor. The two darknesses look similar but are
different mechanisms, and the correct model keeps them separate:

    final = ambient_floor  ⊕  (sky_reach × sky_brightness(time, moon_phase))
                                ⊕  block_light

Design sketch for when we build it:
- `vox-core` light is already 2-channel (sky + block). The mesh currently
  collapses to one scalar per vertex (`max`). To dim only the sky at night, the
  **vertex must carry sky and block separately** so the shader can scale the
  sky channel by a time-of-day factor while leaving block (torches) alone.
  This is the "two-channel vertex light" the build notes above flag — it is the
  gating piece and belongs to this milestone, not M06.
- Moon phase: scale the night sky floor by phase (new moon ≈ near-floor, full
  moon ≈ a few % brighter). Open ground at full-moon midnight should read
  *above* the cave ambient floor — which it will automatically, since the floor
  is the geometric minimum and moonlit sky-reach sits on top of it. Because of
  that ordering, the 0.035 floor chosen now stays correct after day/night lands;
  no re-tuning needed.
- Keep it a smooth curve over the day, not stepped; dusk/dawn are the
  interesting readability moments.

## Retrospective (2026-07-03) — COMPLETE

Shipped: per-vertex smooth lighting (4-cell corner averaging with the
occluded-diagonal exclusion), classic 3-neighbor vertex AO with a tunable
strength, quad-flip anti-anisotropy, a 26-neighbor mesh snapshot (ADR-0006),
and a resolved ambient floor. In-game: cave mouths fall off in smooth
gradients, creases and overhangs read with soft contact shadows, the terrain
has real depth, and there are no visible chunk-boundary seams or butterfly
triangles. Accepted first-try in-game on the smooth-light pass (no shader
change needed — the per-vertex brightness attribute already interpolated).

### Numbers

- **Mesh bench** (realistic surface chunk, x300, release):
  - Pre-M06 per-face baseline: **2736µs**
  - Task 1 (snapshot, still per-face light): **1876µs**
  - Task 2 (per-vertex smooth light): **2236µs**
  - Task 3 (+ vertex AO): **2420µs**, 84 quads
  - Net: smooth light + AO added, yet meshing is **~12% faster than the
    pre-M06 baseline** — the snapshot architecture (one gather up front,
    branch-free hot loop) more than paid for the lighting work. AO's cost is
    the extra ~184µs and the higher quad count (AO breaks merges the light
    field alone wouldn't); bounded and acceptable.
- **Relight:** untouched this milestone (mesh-side work); stays at the M05
  447µs.
- **Sprint-fly, radius 8, dev machine:** steady **144 fps** (vsync cap);
  streaming burst floor **92 fps** during a heavy load spike (dirty ~1900,
  msh ~500ms). Criterion 6 (≥90) holds — a narrow but real pass. The M05
  streaming pipeline (defer-until-lit, per-face requeues, small batches) is
  what keeps the risen mesh cost invisible; without it the AO quad-count bump
  would show.

### Decisions recorded

- **Ambient floor = 0.035** (`light_curve_f`). The brightness of a cell that
  receives no light at all — sealed cave, overhang underside. Chosen darker
  than Minecraft's floor so enclosed dark reads genuinely dark, but not pure
  black. It is a *geometric* baseline (independent of time of day) and is
  forward-compatible with a future day/night cycle — see the day/night section
  above. AO is what made this low end visible enough to tune; strength left at
  the `[0.55, 0.70, 0.85, 1.0]` default, which read well in-scene.
- **Differential oracle scoped to geometry.** The `greedy == naive` oracle now
  compares the *covered face-cell set + layers* only, not interpolated
  brightness. Smooth light is vertex-shared and reconstructs affinely across a
  merged rectangle, so it would match — but classic per-corner AO is
  cell-anchored (each corner excludes its own quadrant), so a greedy run
  legitimately stretches an AO gradient the naive mesher steps per-cell. That
  is an accepted property of AO-on-greedy-meshes (greedy is the shipping
  mesher; its interpolation is the intended result), not a meshing bug. Light
  and AO correctness are pinned directly instead: smooth-light gradient/seam
  tests, and a `corner_ao` truth table + an end-to-end "AO reaches vertex
  brightness through the mesher" test. See the `CellKey` doc comment in
  `vox-mesh/src/lib.rs`.

### Lessons

- **The oracle earned its keep by failing.** When AO went in, the differential
  test failed exactly as it should — it caught that AO is not vertex-shared.
  The failure was the *diagnosis*, not a bug: it forced the (correct) call to
  scope the oracle to the invariant greedy actually preserves (geometry) and
  test the rest directly. Believe the oracle; when it fails on a sound change,
  the fix may be understanding what invariant it really encodes.
- **Diagnose without the dump.** The oracle prints its full coverage map on
  failure — huge. Diagnose narrowly (a constant-AO probe isolated the cause in
  one cheap run) rather than reading the dump.
- **Clippy lints the sandbox can't see are real.** `doc_lazy_continuation`
  fired on a doc-comment list that headless `cargo test` accepts fine. Always
  run the clippy gauntlet on the native machine before compiling; the sandbox
  clone cannot install clippy.
- **Two darknesses are two mechanisms.** "No light reaches here" (cave, the
  ambient floor) and "the light source is dim right now" (night sky) look
  similar but are separate multipliers. Recognizing that kept the ambient-floor
  decision clean and deferred night visibility to the milestone that has the
  infrastructure for it (two-channel vertex light).

### Next-milestone candidates

1. **LOD / far-view-distance + async pipeline** (headline; seed-driven far
   chunks — noted since M04).
2. **Day/night cycle** — two-channel vertex light, time-of-day sky scaling,
   moon-phase night brightness. Design sketch recorded above.
