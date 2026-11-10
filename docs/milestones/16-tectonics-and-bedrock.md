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

**Status: draft; its deciding questions are answered (2026-11-10).**
Nothing is built. The owner chose option C, the plate framework, and
answered every question in `docs/geology.md` §16; ADR-0024 (block names
in saves) is built. How the framework is made is ADR-0025 (proposed),
which also proposes an order: this milestone's tasks 1 and 2 *before*
M15's tasks 2–6, so that landforms are tuned once, on the terrain they
ship on. That reordering awaits the owner's word; the spec as a whole
awaits his approval.

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
2. **Terrain placed by the framework.** Continents, belts and trenches
   move; ridges and rifts appear. The ADR-0010 statistics are held. This
   is the task that bumps the generator and re-runs M13–M15.
3. **Provinces and ages**; the bedrock map; the cross-section tool.
4. **Strata in the chunks**: datum, structure, units; the engine's
   hidden-opaque-chunk shortcut; the bench.
5. **The surface**: soil profiles over real rock (M13's "stone beneath"
   becomes the unit there); bare rock on cliffs, peaks and deserts; the
   LOD.
6. **Every world, the walk, the retrospective.**

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
