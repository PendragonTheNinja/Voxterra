# Milestone 10 — Terrain & the Deep Vertical

**Goal:** give the world a *shape*. A bounded world with a real latitude axis,
elevation with Earth-like vertical scale — mountains that take real effort to
climb, oceans with real basins and rare trenches — and the streaming rework
that makes a 20 000-block-tall world affordable.

This is the first half of worldgen. Climate and biomes are M12 and depend on
everything here: you cannot compute a rain shadow before there are mountains to
cast it.

## Why this is its own milestone

Two decisions taken in the M10 scoping discussion turn out to be structural
rather than tuning:

**The world becomes finite.** Infinite generation is out. The world is a
bounded box — **200 000 × 200 000 blocks**, square — large enough that no player
reaches an edge in normal play. This is what makes a latitude axis possible at
all, and it simplifies far more than it costs.

Square is not just an aesthetic call. Z *is* the latitude axis, so its extent
sets the climate scale: 200 000 blocks pole-to-pole means **100 000
pole-to-equator**, compressing Earth's climate bands 100× rather than 200×. The
squarer world is also the more realistic one — a temperate belt lands around
15 000 blocks wide instead of 7 500. Total area is ~40 000 km², roughly
Switzerland, and about 13 hours of continuous walking pole to pole. It costs
nothing: bounds are constants, chunks generate on demand, saves stay sparse.

**The world becomes ~80× taller.** Today it is 256 blocks. Supporting Everest
(8 848 m) and ocean trenches means roughly −11 000 to +9 000. Streaming
currently loads a full-height cylinder around the camera; at 625 chunk layers
that is ~78× more chunks and the engine stops. The fix is not a bigger budget
— streaming must **follow the surface**, loading a band of layers around the
terrain height per column instead of the whole column. Once it does, total
world height costs nothing.

LOD needs no equivalent change: it is a heightfield (M09/A1), so it is already
independent of world height. Its *suppression residency check* is not — it
walks every chunk layer in the LOD Y band per node and must be narrowed to the
same surface-following band.

## The scale problem, and the rule that resolves it

Pole-to-equator is 100 000 blocks — against Earth's 10 000 km that is **100×
horizontal compression**, while vertical stays 1:1 (one block, one metre).
Those do not compose. The Himalaya rises ~5 000 m over ~100 km (a 1:20 grade);
compress the run 100× and it becomes 5 000 blocks of rise over 1 000 blocks of
run — a 5:1 wall, untraversable and unbuildable.

**Design rule: height is earned by extent.** A peak's elevation is a function
of the size of the uplift region producing it. Nothing is clamped after the
fact; the geometry cannot cheat. At a 1:4 average flank grade — a real alpine
slope — this world gives:

| Peak | Massif needed | How many fit |
|---|---|---|
| 2 000 blocks | ~16 000 blocks across | many — ordinary ranges |
| 4 000 blocks | ~32 000 blocks across | a handful |
| 8 800 blocks | ~70 000 blocks across | at most one, a third of the map |

Rarity therefore falls out of the geometry rather than a dice roll, and the
rule scales: express peak height as a function of massif extent, **never as an
absolute cap**, so a larger world in future automatically grows larger
mountains with no constant to change.

The consequence to accept honestly: gradients here are steeper than Earth's,
because the world is smaller than Earth. The rule bounds the exaggeration and
makes it consistent, it does not remove it.

## Acceptance scene

Spawn near the equator. The horizon is not uniform any more — there is a
direction where the land rises and a direction where it falls toward water.
Walk toward the high ground for a while and the rise is *gradual*: foothills
first, then a long climb, and it takes real time. Turn around at altitude and
you can see the plain you came from, kilometres of it, receding through fog.

Fly to the coast. The seabed drops away in a shelf, then a slope, then a basin
floor thousands of blocks down — and somewhere in the world, a trench that goes
far deeper still. Fly north far enough and the world ends at a wall, not at a
hang: the bounds are real and handled cleanly.

Through all of it, streaming holds. There is no chunk backlog spiral when you
descend into a trench or climb a summit, because only the layers near the
ground were ever loaded.

## Acceptance criteria

1. **Bounded world.** 200 000 × 200 000 blocks, centred on the origin. Chunk
   requests outside the bounds resolve deterministically (air above, solid
   below) rather than erroring, and the camera cannot pass the boundary.
   Headless: bounds arithmetic is exact at every edge and corner, including
   negative coordinates.
2. **Spawn resolver.** A search outward from a start position returning the
   first position satisfying a **predicate**, deterministic from the seed
   alone. M10 passes "is land above sea level" and starts at the world centre,
   so the first thing a player sees is equatorial and habitable. The predicate
   is the point: M12 passes "is land AND is temperate forest" for
   biome-selected spawns without touching the resolver. Headless: same seed
   yields the same spawn; a centre that falls in ocean still resolves to valid
   land.
3. **Latitude axis.** Z maps to latitude, poles at Z = ±100 000, equator at
   Z = 0. Exposed as a pure function of position so both the chunk generator
   and the LOD sampler read the same value. Nothing consumes it yet beyond
   diagnostics — M12 does — but the axis and its scale are fixed here.
4. **Surface-following streaming.** `Streamer` loads a band of chunk layers
   around each column's terrain height rather than the full Y band. Headless:
   for a synthetic heightfield the resident set tracks the surface, contains no
   chunk more than N layers from it, and the load/unload hysteresis still holds
   with no thrash. The LOD suppression residency check narrows to the same
   band.
5. **Deep vertical.** World Y range opens to approximately −11 000 … +9 000.
   Storage, coordinates, camera far plane, and the LOD Y band all handle it.
   Headless: a column at extreme height and one at extreme depth both round-trip
   through generation, meshing, and save/load.
6. **Elevation field.** Continental structure — landmasses, shelves, basins —
   with mountain ranges that run in coherent lines rather than as isolated
   noise bumps, and a sea level. Peak height obeys the earned-by-extent rule.

   **An explicit relief control is required**, separate from amplitude. Fractal
   noise defaults to rough *everywhere*, which produces medium-relief hills wall
   to wall and no true plains — and then the mountains mean nothing, because
   there is nothing dull to contrast them against. Earth is mostly flat: abyssal
   plains, continental shelves, the Great Plains, the Siberian lowlands. A
   low-relief region here must be genuinely, boringly flat across tens of
   thousands of blocks.

   Headless: sampled over a large area, the elevation histogram is plausible
   (most land low, mountains rare, ocean basins the largest single band); a
   measurable fraction of land area has near-zero local gradient; and the field
   is deterministic and identical between the chunk generator and the LOD
   sampler.
7. **Water.** Oceans and seas fill to sea level. Static water only — no flow,
   no rivers, no lakes above sea level (those need hydrology). Beaches and
   shelves where the elevation field produces them.
8. **Performance.** With the deep world and surface-following streaming,
   steady fps within the M09 envelope and no worse dirty-backlog behaviour than
   M09's (peaks ≤ ~410, drains to 0). Report before/after.
9. **Worldgen version stamp.** Saves record the worldgen version; loading a
   world generated by a different version is detected and reported rather than
   silently corrupting terrain. Flagged in the M08 retro and now unavoidable —
   this milestone invalidates every existing save.
10. **Bookkeeping.** ADR for the world model (bounds, latitude, vertical range,
   surface-following streaming). Retro with numbers; CLAUDE.md status.

## Tasks

1. **World bounds, latitude, spawn (`vox-core`, headless).** Extents, the
   coordinate↔latitude mapping, out-of-bounds resolution, the predicate-based
   spawn resolver. Pure functions, exhaustively tested at edges and corners.
2. **Surface-following streaming (`vox-core`, headless).** Generalize
   `Streamer` from a full-height cylinder to a surface-tracking band; narrow
   the LOD suppression band to match. **Do this before the vertical range
   opens** — it is the change that makes the range affordable, and landing it
   first keeps every intermediate state playable.
3. **Deep vertical (`vox-core` + wiring).** Open the Y range; audit storage,
   the camera far plane, LOD node spans and AABBs.
4. **Elevation field (`vox-worldgen`, headless).** Continents, shelves, basins,
   coherent ranges, the earned-by-extent height rule, sea level. Expressed as a
   named pipeline stage so an erosion pass can be inserted later without
   touching anything downstream.
5. **Water + version stamp (wiring).** Fill to sea level; stamp and check saves.
6. **Tune + retro.** Landform scale, band width for streaming, backlog numbers,
   fps; retrospective; close-out.

## Non-goals

- **No climate, no biomes, no vegetation.** M12. Surface materials stay as they
  are — the world will still be green. Judge terrain *shape* here.
- **No rivers, lakes, or erosion.** These need a cached region-simulation pass
  (a second storage system), because a river must know its upstream. Its own
  milestone.
- **No caves, strata, or ores.** Separate, and they interact with hydrology.
- **No variable world size at runtime.** One baseline size, a constant. Player-
  selectable sizes are a game-layer feature for much later.
- **No new crate.**

## Notes for whoever builds this

- **The LOD sampler and the chunk generator must agree exactly.** M09 spent a
  long time on ghost blocks caused by two paths disagreeing about terrain
  height. That was over player edits; here it would be over the elevation field
  itself, at continental scale. Elevation must be one function with one answer,
  and the headless test for criterion 5 should assert it directly.
- **Watch the per-column cost.** `surface_height` is called once per column per
  chunk *and* once per LOD cell — on the order of a million times per second
  during flight. M09's telemetry already shows meshing at 500–590 ms/s. Every
  octave added to the elevation field multiplies through both paths. Budget it,
  and consider whether the field wants a coarse cached tier.
- **Land task 2 before task 3.** Opening the vertical range first produces a
  build that cannot stream, and you will be debugging worldgen through a broken
  engine.
- **The world gets less green, not more, this milestone.** Terrain shape is
  genuinely hard to evaluate against uniform grass — the same problem that made
  M09's geomorph hard to judge. Consider a temporary elevation-tint debug view
  (a toggle, not a shipped feature) so the field can be seen while it is tuned.

## Amendments (owner-approved, added mid-milestone)

- **A1 — Transparency and water (ADR-0011).** The engine had no transparency,
  and oceans cover ~58% of the world. `solid` splits into physical `solid` and
  visual `opaque`/`renders`; the mesher culls a face when its neighbour is
  opaque or the same block; transparent geometry draws in a second pass.
  Supersedes the implementation approach of criterion 7; its intent stands.
- **A2 — The world is a torus (ADR-0012).** Both horizontal axes wrap, latitude
  loops and is equal-area, and world size becomes a per-world value quantised to
  8 192 blocks (default 204 800). **Supersedes criteria 1 and 2.** The seam
  lives only where content is addressed — generation, saves and the LOD edit
  overlay — while the player's frame never wraps, so streaming, LOD, rendering
  and physics are untouched (ADR-0012 §4). Each world also calibrates its own
  coastline so land fraction no longer depends on the seed.
- **A3 — Audit fixes (2026-09).** Found in a whole-codebase audit, each either a
  live bug or a cost that grows with the world:
  - **World positions become `f64`.** At the world's edge an `f32` camera can't
    move at 2 000 fps and walks 81% fast at 1 000; walking is already 10% slow
    at the default spawn. Done together with A2, which touches the same code.
  - **`column_heights` is pruned when a chunk column unloads.** It was never
    freed — ~0.26 GB per 10 km flown. The level-0 LOD real-height gather that
    also read it is removed: seed heights plus `EditedColumns` are identical,
    and it cost ~4 000 main-thread lookups per level-0 node.
  - **`world.meta` records the generator version and world size.** This is
    criterion 9, widened: a world is meaningless without its period.
  - **The LOD sampler goes sparse beyond level 0.** Exact minimum over every
    block is needed only where LOD overlaps full-resolution terrain; elsewhere
    a 4×4 sample per cell costs ~3 ms per node at any stride. This is the cause
    of the ~60 ms spikes that currently fail criterion 8, and the enabling
    change for M11's horizon. Narrows ADR-0008's never-exceed contract, so it
    gets an ADR amendment.
  - **LOD suppression waits for chunks to be DRAWN, not just resident.**
    (Done.) Suppressing a node over chunks whose first mesh was still queued
    showed sky through the gap: a flash at the edge of the full-res region at
    radius 8, a band hundreds of blocks wide at radius 24.
  - **Streaming follows the camera as well as the ground.** Surface-following
    loads only a few chunk layers below the surface, so a player who digs more
    than ~100 blocks down walks out of the loaded world. The vertical window
    must also cover the camera's own neighbourhood. Done with the `f64`
    rework, which is the same question — where the player actually is.
  - **No faces toward chunks that will never load.** The mesher reads an
    unloaded neighbour as air, so every column's lowest loaded chunk draws a
    black floor into the void beneath it — invisible from above, but real
    geometry, and plainly visible from underground in spectator.
  - Cleanup: delete the orphaned `vox-core/src/downsample.rs` (no `mod`
    declaration anywhere) and the stray `docs/decisions/voxterra.code-workspace`;
    deduplicate the surface-span sampler in `vox-app`; replace
    `LOD_WORLD_Y_BLOCKS` with the planet constants it now duplicates.

**Order of remaining work:** A2 with the `f64` rework → the rest of A3 → A1 →
tuning and retro. Water goes last so the criterion-8 performance numbers are
taken with oceans actually meshed.

## Scale correction, mid-milestone

The first implementation of task 4 used literal Earth relief — 4 300-block ocean
basins, peaks near 8 300. It passed every test written for it and was unplayable:
a 200 km world holds one continent's corner at those dimensions, and flying for
minutes found a continental margin mistaken for a mountain, an abyssal plain,
and one hill.

**ADR-0010** records the correction and the reasoning: mirror Earth's systems,
scale its dimensions. A first pass (vertical ÷5, horizontal ÷3) was still wrong
because it was judged from spectator flight; the field is now tuned to ground
walking time — detail every ~2.4 minutes, a range crossed in ~21, the next
mountain belt in ~39. Criteria 5 and 6 are unchanged in intent; only the numbers
they produce moved.

## Decisions taken during scoping

- **Finite, square, 200 000 × 200 000.** A rectangle was considered and
  rejected: being able to travel meaningfully further along one axis than the
  other reads as arbitrary. Square costs nothing here because Z's extent is
  the latitude scale, and 100 000 pole-to-equator sits at the realistic end of
  the range under consideration anyway.
- **1 block = 1 metre, vertically.** Peaks and depths are quoted in real units
  and mean it. Horizontal scale is compressed ~100×; vertical is not. The
  earned-by-extent rule is what keeps that from producing walls.
- **Spawn at the world centre, on land.** Equatorial and habitable. Delivered
  through a predicate-based resolver so biome-selected spawns are an M12 game
  feature rather than an M10 rewrite.
- **Split from the original M10 scope.** Climate, precipitation, biome
  classification and surface materials moved to M12. The vertical range
  decision turned out to require a streaming rework, which is a milestone's
  work on its own, and climate depends on elevation existing first.
