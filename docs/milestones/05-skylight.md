# Milestone 05 — Skylight

- **Status:** Spec accepted, not started
- **Spec written:** 2026-06-15
- **Depends on:** Milestone 04 (block light), ADR-0005 (skylight heightmap).

## Goal

Add **skylight**: ambient illumination from the open sky, so the world is
bright outdoors in daylight and dark in caves / under overhangs / at night
(no day-night *cycle* yet — a static full-day sky). Skylight is the second
light channel; it combines with block light (max of the two) into the mesh
brightness already in place.

Per ADR-0005, skylight is **heightmap-based**, not fixed-ceiling: it tracks
the highest solid block per `(x, z)` column and floods skylight downward from
there. This is height-agnostic — it works identically for a Y30 meadow or a
Y8800 peak — and commits to no world ceiling, keeping the door open for
realistic mountain-scale terrain.

**Acceptance scene:** spawn onto the surface in broad daylight — open ground
is bright, the existing lamp glow still reads against it. Dig a tunnel into a
hillside: it gets dark as you go deeper, with daylight spilling a short way
in from the entrance. Dig a vertical shaft straight down: full daylight
reaches the bottom (skylight doesn't attenuate going straight down). Build a
roof over yourself: the space beneath falls into shade. Break the roof: light
returns. All live, no stalls, fps holding the M04 baseline.

## Acceptance criteria

1. **Skylight channel in the high nibble.** Per-voxel skylight (0..=15) is
   stored in the reserved high nibble of the light byte (ADR-0004/0005). No
   chunk-format change; skylight is derived, recomputed on load.
2. **Per-column heightmap.** The engine tracks the highest sky-occluding
   block per `(x, z)` column over the loaded world. Cells at/above the column
   height are full skylight (15); cells below may be shadowed.
3. **Skylight propagation.** Skylight floods from the open top downward and
   sideways via the M04 BFS machinery, with the ADR rule: **no attenuation
   travelling straight down** through air (a vertical shaft stays 15 to the
   floor); dims by one per horizontal step (and per upward step). Stops at
   solid blocks.
4. **Open-sky top boundary.** A chunk with nothing solid above it in a column
   (per the heightmap) receives full skylight from its top boundary — the
   skylight analogue of a lit neighbor border. A missing chunk *above* means
   open sky (15), NOT darkness (the key asymmetry vs. block light).
5. **Cross-chunk + streaming correctness.** Skylight spreads between vertical
   and horizontal neighbors. When a taller chunk streams in (or is built)
   above a column, the columns' heights rise and the now-shadowed cells below
   re-propagate (via the dirty-set path). When such a chunk unloads/breaks,
   light returns. Convergence within a frame or two is acceptable (bounded,
   like M04 block-light borders).
6. **Combine sky + block light.** The mesher bakes
   `brightness = directional_shade × light_curve(max(sky, block))`. Both
   channels sampled at the face's air-side cell. Greedy merge criterion
   includes the combined level (faces merge only when it matches).
7. **Edits update height + relight.** Placing/breaking a sky-occluding block
   updates its column height and re-propagates skylight (and block light) via
   the existing dirty path. Persists as before (blocks persist; light
   recomputes).
8. **Performance.** Skylight relighting runs on the **parallel** compute path
   established in M04 (compute read-only across cores, apply sequentially) —
   never a main-thread per-chunk loop. fps holds the M04 baseline (~144)
   including during streaming. Surface light/heightmap activity in telemetry.
9. **Re-tune ambient floor.** With daylight now present, adjust the M04
   interim ambient floor (0.06) so unlit/cave/night surfaces vs. full-sky
   surfaces have pleasing contrast (dark but navigable vs. bright).
10. **Headless-testable core.** Skylight propagation (vertical no-attenuation,
    horizontal falloff, open-top boundary, shadow under a cover, the
    heightmap computation) is unit-tested in the headless crates against the
    `LightVolume`/grid harness, independent of rendering.

## Non-goals (explicitly out of scope)

- **Day-night cycle / moving sun / dynamic sky brightness.** Skylight is a
  static full-day value this milestone. Animating it is later.
- **Directional sun / shadows cast by sun angle.** Skylight is ambient
  top-down, not a directional light with angled shadows.
- **Colored skylight, sky color gradients, atmospheric scattering.** A single
  scalar channel, like block light.
- **Smooth lighting / ambient occlusion.** Per-face combined level is enough;
  smooth vertex lighting + AO is a later polish pass (would apply to both
  channels at once then).
- **Transparency-aware skylight** (light through glass/water/leaves at reduced
  falloff). All blocks remain opaque; revisit with a "light opacity" field
  when transparent blocks arrive.
- **World floor / bedrock and realistic tall worldgen.** Independent future
  work (noted in the M04 retro). Skylight is designed to not depend on them.
- **Persisting skylight or the heightmap.** Derived, recomputed on load.

## Key design decisions (settled in ADR-0005; restated)

- Heightmap, not fixed ceiling (height-agnostic; no ceiling committed).
- Skylight in the high nibble; combine with block light via `max`.
- Vertical-down skylight does not attenuate; horizontal does.
- Open-sky boundary: absent chunk above column height = full skylight in.
- Heightmap + skylight are derived/non-persisted; recompute on load.

Still to settle in the spec/early tasks:
- **Where the heightmap lives.** Per-loaded-chunk "is anything solid above me
  in this column?" can be derived from neighbors during the parallel relight,
  OR a separate `World`-level column-height map keyed by `(x,z)`. Decide for
  testability + the streaming-update rule. Lean toward deriving the top-
  boundary per chunk from the vertical neighbor stack during relight (reuses
  the M04 border-input shape: the "+Y neighbor" contributes a skylight border;
  if there is no loaded +Y neighbor, the boundary is "open sky" unless a
  higher loaded chunk in the column occludes — which requires knowing the
  column above, i.e. some column-height tracking). Resolve in task 1.
- **Vertical relight propagation distance.** A newly-built tall tower must
  shadow cells far below. Bound how far skylight re-propagation cascades
  downward per frame within the dirty budget.

## Suggested task breakdown

1. **Skylight storage + heightmap model (headless).** Sky nibble accessors on
   `Chunk` (`sky_light`/`set_sky_light`, masking the high nibble). Decide and
   build the heightmap/top-boundary representation. Unit-test nibble
   independence (sky and block light don't clobber each other) and heightmap
   computation. ADR-0005 already written.
2. **Skylight propagation (headless).** Extend the BFS for the no-downward-
   attenuation rule + open-top boundary, over the grid harness. Unit-test:
   vertical shaft stays 15 to the floor; horizontal falloff; shadow under a
   cover; cave darkens with depth; open flat ground = 15 everywhere on top.
3. **Two-channel chunk relight (headless).** Compute both channels into the
   chunk (sky high nibble, block low nibble) in the parallel-friendly
   read-only form (M04 `compute_chunk_light` shape). Feed the top-boundary
   from the vertical neighbor / column height. Unit-test cross-chunk vertical
   skylight (shaft through two stacked chunks) and that block light is
   unaffected.
4. **Mesh + app integration.** Mesher samples `max(sky, block)` → brightness;
   greedy merge on the combined level. Wire skylight into the parallel relight
   path + dirty-set (height rises on streamed/placed cover → re-propagate
   below). Telemetry. Visual result: daylight outdoors, dark caves, shafts lit
   to the bottom.
5. **Polish + baseline.** Re-tune ambient floor for day/night contrast;
   confirm fps holds through streaming and big vertical edits; measure
   baseline; retrospective.

## Notes for whoever builds this (human or model)

- Reuse everything from M04: the `LightVolume` BFS, the parallel
  compute/apply, the dirty-set + neighbor-expansion path. Skylight is "block
  light with two rule changes (down-no-attenuate, open-top=15) and a
  heightmap." Do NOT build a parallel lighting system.
- **Performance is a graded criterion, not an afterthought** (learn from M04
  task 4): relight stays on the parallel read-only path from the start. Test
  fps *under streaming*, not just standing still — that's where the M04
  regression hid.
- The open-top asymmetry is the #1 correctness trap: block light treats a
  missing neighbor as dark; skylight treats a missing chunk *above the column
  height* as full daylight. Get the boundary direction right and test it
  explicitly (a single isolated surface chunk in the void should have a fully-
  lit top, not a dark one).
- Vertical no-attenuation only applies **straight down through non-solid
  cells**. The moment skylight turns a corner (horizontal step) it attenuates
  like normal. A deep shaft is bright at the bottom; a horizontal tunnel off
  the bottom of the shaft darkens along its length.
- When accepted: update CLAUDE.md status, write the retrospective with
  baseline numbers, re-confirm the ambient-floor value, then decide the next
  milestone (candidates: smooth lighting/AO, transparent blocks, the
  seed-driven LOD/far-view system from the M04 retro, or realistic worldgen +
  world floor).
