# Milestone 04 — Block Light

- **Status:** Spec accepted, not started
- **Spec written:** 2026-06-15
- **Depends on:** Milestone 03

## Goal

Build the **lighting engine** and prove it on **block light** — light that
emits from placed blocks (torches, glowstone-likes) and floods outward
through air, dimming with distance and stopping at solids. This delivers the
entire reusable light system — per-voxel light storage, BFS flood-fill
propagation (add and remove), registry-driven emission, and light baked into
the mesh — on the problem that carries **no skylight/cubic-chunk
architectural risk**.

**Skylight is Milestone 05**, deliberately. M05 will add a second light
channel and solve the hard "no world ceiling" propagation question on top of
the machinery proven here. This milestone must leave the design ready for
that (a second channel slots in cleanly), but builds none of it.

**Acceptance scene:** place a light-emitting block in a dark area (a cave, or
night/the underside of terrain) and watch a smooth radial gradient of
brightness spread from it across nearby surfaces; break it and watch the
light correctly retract to darkness; the lit region updates live as you
add/remove sources, with no full-world rebuild and stable framerate.

## Acceptance criteria

1. **Per-voxel light storage.** Each chunk stores a block-light level per
   voxel (0..=15). Storage mirrors the block-data design sensibly (a uniform
   all-dark chunk — the common case — must stay cheap; do not balloon memory
   for the empty majority). Light is runtime-derived state.
2. **Registry emission.** `BlockType` gains a `light_emission: u8` (0..=15).
   A starter emitter exists (e.g. a "lamp"/"glowstone" block, emission ~14),
   added to the placeable set so it can be placed in-world.
3. **Flood-fill propagation (add).** Placing/among an emitter floods light
   via BFS: a source seeds its emission level; each step into an adjacent
   non-solid voxel is one level dimmer; propagation stops at 0 and does not
   pass through solid blocks. Correct across chunk boundaries (light spreads
   between neighboring chunks).
4. **Flood-fill removal.** Removing a light source (break the emitter, or
   place a solid that occludes) correctly *unlights* the affected region —
   the standard two-phase removal (clear the source's contribution, then
   re-propagate from remaining sources at the borders). No stale light left
   glowing, no over-darkening.
5. **Light drives mesh brightness.** The mesher bakes the light level at each
   face into the existing `brightness` vertex scalar (combined with, or
   replacing, the current fixed directional shading — decide and document).
   Brighter light → brighter face. Re-meshing a chunk picks up current light.
6. **Updates via the dirty-set path.** Light changes mark affected chunks
   (and neighbors light reached into) dirty, re-meshing through the existing
   M02/M03 path. Budget propagation work per-frame like meshing/gen if it can
   be large; surface light-update activity in telemetry.
7. **Persistence interaction.** Light is *derived* from blocks, so it need
   not be serialized — it can be recomputed on load from the (persisted)
   blocks. Decide explicitly: recompute-on-load vs. persist-light. Recompute
   is simpler and keeps the chunk format unchanged; prefer it unless there's
   a measured reason not to. If light is NOT persisted, loading a modified
   chunk must trigger (re)lighting.
8. **Headless-testable core.** The propagation engine — seed, flood, dim,
   stop-at-solid, cross-chunk spread, and the removal/re-propagation — is
   unit-tested in the headless crates against a controllable block/solid
   oracle, independent of rendering. Include the classic cases: single source
   in open space (radial falloff), source in a corridor (linear falloff),
   removal restoring darkness, two overlapping sources (max, not sum).
9. `cargo test` green workspace-wide; clippy/fmt clean.

## Non-goals (explicitly out of scope)

- **Skylight / sun / daytime** — all Milestone 05, including the cubic-chunk
  no-ceiling propagation problem and the second light channel.
- Colored light (RGB light levels). Light is a single scalar 0..=15 this
  milestone. (Colored light is a possible far-future channel; not now.)
- Smooth per-vertex light interpolation / ambient occlusion. Per-face light
  is enough here; smooth lighting (averaging light across the 4 corners of a
  face, AO darkening in crevices) is a polish pass that can come later. If
  per-face looks blocky, that's acceptable for this milestone.
- Directional sunlight shadows, shadow maps, global illumination, any
  screen-space technique. This is voxel flood-fill light only.
- Light-update animation/flicker, day/night cycle, dynamic emitters.
- Transparency-aware light (light passing through glass/water at reduced
  falloff). All blocks remain opaque; light passes only through air. (Glass
  etc. is later, and will want a "light opacity" field then.)

## Key design decisions to make (in the spec/ADR, before coding)

- **Where light storage and the propagation engine live.** Light data is
  per-chunk, so storage likely lives on/near `Chunk` in vox-core; the
  propagation engine is pure logic operating over chunks + neighbors (same
  borrowed-access shape as meshing). Keep it headless and testable. An ADR is
  warranted for the light-storage representation and the propagation
  algorithm (BFS add + two-phase remove), and to **explicitly note how a
  second channel (skylight, M05) will slot in** — e.g. two arrays, or a
  packed byte (4 bits sky + 4 bits block) — decided now so M05 doesn't
  reshape storage.
- **Recompute-on-load vs. persist light.** Per criterion 7 — prefer
  recompute (format stays at current version, light is never stale vs.
  blocks). Confirm the chunk format does NOT need a version bump this
  milestone (it shouldn't, since light isn't serialized).
- **Mesh brightness combination.** How baked light interacts with the
  current fixed directional face shading: replace it, multiply with it, or
  use directional shading only as a floor. Decide so faces in full light
  still have *some* face-to-face contrast (pure flat light looks
  shapeless), but dark areas actually go dark.
- **Propagation scope / budget.** A single placed torch touches up to a
  ~15-radius sphere of voxels, potentially several chunks. Decide how
  propagation is bounded and scheduled relative to the per-frame budgets so a
  burst of edits can't stall the frame.

## Suggested task breakdown

1. **Light storage + registry emission (headless).** Per-voxel light on the
   chunk (cheap uniform-dark case), `light_emission` on `BlockType`, a lamp
   block. Unit-test storage get/set + uniform compactness. No propagation
   yet. ADR for storage + algorithm + the M05-channel note.
2. **Flood-fill add (headless).** BFS propagation from sources within a
   chunk + across neighbors. Unit-test radial/corridor falloff, stop-at-
   solid, cross-chunk spread, overlapping-sources-take-max.
3. **Flood-fill removal (headless).** Two-phase removal; unit-test that
   breaking a source restores darkness and re-propagation from other sources
   is correct.
4. **Mesh integration.** Bake light into the `brightness` vertex scalar
   (combination decided per ADR). Wire light (re)computation into the
   dirty-set path; relight-on-load for modified chunks. Telemetry for light
   updates. Visual result: placing/breaking the lamp lights/darkens the
   world live.
5. **Polish + baseline.** Tune the brightness curve (a 0..15 level → a
   pleasing 0..1 multiplier, usually non-linear), confirm no frame stalls on
   bursts of edits, measure baseline (fps, light-update cost). Retrospective.

## Notes for whoever builds this (human or model)

- Reuse the dirty-set + neighbor-expansion path for relighting — it already
  handles cross-chunk re-meshing and persistence. Light propagation should
  feed it, not duplicate it.
- Keep the propagation engine headless and registry-agnostic in the same
  spirit as the mesher: it needs "is this voxel solid?" and "what does this
  voxel emit?" — supply them as borrowed accessors, don't bake in the
  registry or World type.
- The classic block-light removal algorithm is two-phase BFS: (1) from the
  removed source, walk outward clearing any voxel whose light could only have
  come from it (light value exactly one less than the current cell), queuing
  the borders where higher light remains; (2) re-propagate from those
  borders. Implement and test this carefully — naive "just re-flood
  everything nearby" is acceptable for a first cut if bounded, but the
  two-phase method is the correct, efficient one and worth doing right.
- **Decide the storage byte layout with M05 in mind now.** A packed
  `u8` per voxel (high nibble sky, low nibble block) means M05 adds skylight
  with zero storage reshape — just use the other nibble. Strongly consider
  this even though only the block nibble is used this milestone.
- When accepted: update CLAUDE.md status, write the retrospective with
  baseline numbers, then write `05-skylight.md` (with its ADR for cubic-chunk
  skylight propagation — the heightmap-vs-boundary-propagation question)
  before any M05 code.
