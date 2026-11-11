# Milestone 16 — Tectonics & Bedrock

**Goal:** the world gets a geological foundation. Plates, their boundaries
and each continent's internal ages are worked out at world creation; the
terrain's continents, ranges and trenches are placed by them; and the
ground under the soil becomes real rock — strata laid in order, tilted and
folded where the land was squeezed, cut through by the valleys and
canyons already there — so that a geologist could map it.

This is the first of five geology milestones. The design all five are
written against is `docs/geology.md`; read it first. This spec is the
part of it built now.

**Status: draft; its deciding questions are answered (2026-11-10);
tasks 1 and 2 are built (2026-11-11) — task 2 awaits the owner's
flight and `--bench` on generator version 9.** The owner chose option C, the plate
framework, and answered every question in `docs/geology.md` §16;
ADR-0024 (block names in saves) is built. How the framework is made is
ADR-0025 (proposed, sections 1–3 built), and the order is the owner's
(2026-11-10): **this milestone's tasks 1 and 2 first, then M15's tasks
2–6, then this milestone's tasks 3–6** — so that landforms are tuned
once, on the terrain they ship on. **Next: the owner's flight and bench
on version 9** (see *Task 2 as built* below), then M15's task 2.

## Why this milestone exists

After M15 the world has Earth's climates, rivers and landforms on terrain
whose large-scale structure is four noise fields with no relation to one
another (`docs/geology.md` §4). Mountain belts are where ridged noise
happens to peak; trenches are somewhere else. Everything below the soil
is one block, "stone". Pillar 2 — "geology-driven worldgen:
tectonic-informed terrain, real stratigraphy" — is the one pillar with
nothing built under it.

Rock cannot be added honestly to terrain that has no cause. So this
milestone does two things in order: it gives the terrain its cause (the
tectonic framework), then lays the rock that cause implies.

## What it builds

From `docs/geology.md`:

- **§4, the framework** — plates, crust, boundaries, continents' internal
  ages, hotspots — as a stage-1 map built before climate.
- **Terrain placed by the framework.** ADR-0010's fields keep their
  shapes, amplitudes and wavelengths; their *placement* comes from the
  framework. New large features the framework makes possible: mid-ocean
  ridges, rift valleys, plateaus behind collisions.
- **§6, provinces** — every place assigned one, blended at edges.
- **§7, the six ages** and sequences.
- **§8 steps 1–4 and 7–8** — datum, structure, units, the surface cutting
  the stack. (Steps 5 and 6, intrusions and metamorphism, are milestone
  17; until then orogen cores and shields use a single crystalline
  "basement" suite of gneiss, schist and granite, by province.)
- **§9, the first rocks**: the sedimentary suite (conglomerate, sandstone,
  red sandstone, siltstone, shale, limestone, dolostone, chalk with
  flint, coal, rock salt, gypsum, banded iron), the basement suite
  (granite, gneiss, schist, slate, quartzite, marble), the ocean-floor
  and volcanic-province rocks (basalt, gabbro, peridotite, serpentinite,
  chert), and till and loess — about thirty; and the fossil-bearing
  versions of limestone, chalk, shale and coal, and stromatolitic
  dolostone, placed by age (`docs/geology.md` §7, *Fossils*).
- **§5, the floor**: the heat gradient per province, and plastic rock
  below it (the heat *mechanic* is the survival milestone's).

It does not yet build volcanoes, intrusions as bodies, metamorphic zones,
caves, ore, or rock's effect on erosion (milestones 17–20).

## Acceptance criteria

1. **ADRs first**: the tectonic framework and how terrain is placed by it
   (superseding the placement parts of ADR-0010, keeping its scale);
   strata in structural coordinates; the block additions. Accepted with
   the retrospective. (The first is drafted: ADR-0025.) Every tunable
   the milestone adds is a per-world option (`docs/geology.md` §3,
   *World options*).
2. **Instruments first**: world maps of plates and boundaries, provinces,
   bedrock at the surface, and a cross-section tool (a vertical slice
   along any line, as a geologist draws one); the readout names the
   province and the rock underfoot.
3. **The framework is tectonics** — as tests, over many seeds and every
   legal size: every high range lies along a convergent boundary and
   every subdued one along an old suture; every trench has its arc on the
   overriding side; every continent has at least one craton; both passive
   and active margins exist; boundary kinds follow from plate motions.
4. **The terrain is still ADR-0010's**: walking-time statistics
   (`terrain_changes_as_you_travel` and its companions), land fraction,
   height distribution and relief distribution within stated tolerances
   of generator version 8's. A range is still crossed in ~21 minutes.
5. **Strata obey the checklist** (`docs/geology.md` §2), as tests:
   superposition in every sampled column; lateral continuity of every
   unit across its province; flat beds on cratons, folds in fold belts,
   tilted blocks in rifts; unconformities where the history has gaps; the
   same bed on both walls of a valley.
6. **Rocks are in their settings**: nothing outside a province that can
   make it; banded iron only of ages 1–2; coal only in beds laid down in
   humid latitudes; outcrop shares near Earth's (about two-thirds
   sedimentary).
7. **Every world, every size**: a stated minimum set of provinces in the
   smallest legal world; all of them from a stated size up.
8. **The earlier milestones still hold**: every biome in every world
   (M13); every M14 water test; M15's landforms where its catalogue puts
   them. Each is re-run and its tuning re-checked on the new terrain;
   closed basins are re-measured (they should fall — `docs/geology.md`
   §4).
9. **Near and far agree**: the LOD shows bare rock in its own colour and
   beds as bands on cliffs; no seam.
10. **Sound and fast**: determinism and per-chunk independence;
    `GENERATOR_VERSION` bumped, fingerprint re-pinned; world creation
    within a stated budget; the mesher's hidden-chunk shortcut extended
    to fully opaque mixed chunks; `--bench` within ~10% of M15's close,
    with underground chunks no longer uniform.
11. **The owner's walk** — a canyon whose walls show the same beds on
    both sides; a fold belt's ridges; a shield; a coast of each kind —
    and a retrospective with maps and cross-sections.

## Tasks (proposed order)

0. **Decisions and catalogue.** The owner's answers to `docs/geology.md`
   §16. The rock catalogue's first thirty, with textures. ADR-0024 built.
1. **The framework** in stage 1, with its maps and its tests. No terrain
   change yet: the framework is drawn beside the existing terrain.
   **Done 2026-11-11.**
2. **Terrain placed by the framework.** Continents, belts and trenches
   move; ridges and rifts appear. The ADR-0010 statistics are held. This
   is the task that bumps the generator and re-runs M13–M15.
   **Built 2026-11-11 (`GENERATOR_VERSION` 9); the owner's flight and
   bench to come.**
3. **Provinces and ages**; the bedrock map; the cross-section tool.
4. **Strata in the chunks**: datum, structure, units; the engine's
   hidden-opaque-chunk shortcut; the bench.
5. **The surface**: soil profiles over real rock (M13's "stone beneath"
   becomes the unit there); bare rock on cliffs, peaks and deserts; the
   LOD.
6. **Every world, the walk, the retrospective.**

## Task 1 as built (2026-11-11)

`vox_worldgen::tectonics` — `Tectonics`, held by the `Generator`
(`Generator::tectonics()`), read by nothing in generation, so
`GENERATOR_VERSION` is still 8 and no world changes. What it holds, what
the building corrected in the design, and the survey numbers are in
ADR-0025's *As built*; the map is `docs/gallery/m16/`.

What a caller can ask:

| Call | Gives |
|---|---|
| `terranes()`, `plates()`, `hotspots()` | The tables: each terrane's plate, crust, continent, craton flag, accretion step and age; each plate's drift |
| `boundary(a, b)` | What the boundary between two terranes is: quiet, passive margin, suture or seam (with age), convergent (collision, continental arc, island arc, and which side is over), ridge, rift, transform |
| `site(x, z)` | A column's terrane, the terrane across its nearest boundary, the distance to it and its kind — what task 3's provinces are read from |
| `column(x, z)` → `continentalness()`, `belt()` | The two fields terrain is placed by: signed distance to the coast (clamped at ±3 500) and the mountain-belt envelope in `[0, 1]` |

Fifteen tests hold it: the same seed and size give the same framework;
it is periodic to the bit on every lap; the lattice fits every legal
size (and the search window is proven wide enough for every one);
plates and continents are each one piece; continents are grown outward
from one to three cratons; every boundary follows from crust and drift
(`docs/geology.md` §4's table, pair by pair, both ways round); active
margins are Orean; a default world has both kinds of coast, an arc, a
ridge and sutures; the smallest world has land, sea and a plate
boundary; and — walking lines across whole worlds — a terrane changes
only at its edge, the belt is full on a boundary and nothing beyond its
half-width, and neither field ever jumps.

**Where task 2 starts** (ADR-0025 *As built* has the reasoning):

1. **Cost first.** One search per column is ~165 ns against ~112 ns for
   today's whole height; called naively, height would cost about twice
   what it does. Do the search once per patch (chunk columns, LOD grids
   and stage-1 maps are all patches) and measure with `tectonics_cost`,
   the generation benches and `--bench` before any terrain is judged.
2. **Then placement**, one input at a time, each behind the ADR-0010
   guard: the continent field (`continentalness` scaled into the coastal
   profile, plus the fine coastal octaves, then the 42% calibration);
   the belt gate (`belt`, with strengths calibrated — the envelope is
   much narrower than version 8's gate, so expect to widen belts by
   kind, not only scale them); trenches along the subducting side of
   convergent boundaries.
3. **Then the new shapes**: mid-ocean ridges, rift valleys, foreland
   sags.
4. **The smallest world** has no guaranteed passive margin or collision;
   criterion 7's "stated minimum" should be: land and sea, one plate
   boundary with a belt, one craton.

## Task 2 as built (2026-11-11)

Generator version 9: continents are continental crust, ranges stand on
collisions, arcs and old welds, trenches lie off subduction zones, ocean
ridges rise and rifts drop. ADR-0025's *As built: task 2* has the
numbers; the gallery (`docs/gallery/m16/`) has version 8 and 9 side by
side.

**Criterion 4's tolerances**, stated now that both versions are
measured (`terrain_stats`, six seeds, default size): land within a
point of 42%; land under ranges within ±25% of version 8's 5.0–7.6%;
median, 99th-percentile and highest land heights within ±15%; near-flat
land within 5 points; the walk spreads no lower than version 8's (more
variety is not a failure). Version 9 meets all of these. Two numbers
moved that criterion 4 did not name, and they are the owner's to judge:
**shelves are 4–6× narrower** and **landmasses ~30% smaller** (ADR-0025
lists both, and what would change them).

**Criterion 3**, as tests: `mountains_and_trenches_are_where_the_plates_put_them`
(no uplift off a belt; every massif of 700+ on a strong boundary; every
trench on the plate going under), `ridges_rise_and_rifts_drop`, and
task 1's framework tests.

**Criterion 8**, re-run: every biome in every world; every M14 test
(four found real flaws in M14's water code on the new ground, fixed;
two were re-anchored from places version 9 moved — ADR-0025 says which
and why); M15's process survey, nothing misplaced. Closed basins fell
from 29% to 22%.

**Criterion 10** so far: deterministic, pinned, chunk generation at
version 8's cost, world creation ~10–15% dearer (6.6–7.4 s). The
`--bench` is the owner's.

**What the owner should look at** in flight: coasts (narrow shelves);
continent sizes; ranges — cordilleras on arcs and collisions, worn
massifs on old welds; trenches and ridges from the air; a rift valley;
an enclosed basin (rhombic, filled flat, `docs/gallery/m16/task2-basin-close-up.jpg`).

## Non-goals

- **Intrusions, metamorphic zones, volcanoes** — milestone 17.
- **Rock resistance shaping the land; karst** — milestone 18.
- **Caves, groundwater** — milestone 19.
- **Ore, prospecting** — milestone 20.
- **Moving plates, earthquakes, eruptions** as events in play.
- **Mining mechanics** (collapse, support, heat) — the gameplay track.

## Risks

- **Task 2 is a second M10.** M10 tuned the terrain twice before it was
  walkable (ADR-0010). Changing placement without changing shape is meant
  to avoid a third time, and criterion 4 is the guard; if the statistics
  cannot be held, stop and say so before M13–M15 are re-tuned around a
  worse world.
- **The smallest world** (32 km) has room for perhaps three plates. The
  framework must degrade to "one continent, one active margin, one
  passive" rather than fail.
- **Cost.** Mixed underground chunks cost memory and meshing that uniform
  stone never did. Measure at task 4, not at the end.
