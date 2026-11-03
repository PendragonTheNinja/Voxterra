# ADR-0019: Stage-1 drainage and the erosion stage

- **Status:** Accepted (2026-11-03, with M14's retrospective; proposed
  2026-11-01, M14 tasks 1–2). The owner's three flights stood in for the walk.
- **Context:** M14 needs rivers that know their upstream, lakes that know
  their outlet, closed basins that know they have none, and valleys cut by
  the water that runs in them (`docs/milestones/14-rivers-lakes-and-valleys.md`).
  None of that is local: a river's size is the rain on its whole catchment,
  and a hollow fills or stays dry by what flows into it from far away. Stage
  2 must stay per-chunk independent (CLAUDE.md), so like climate (ADR-0017)
  this is a stage-1 grid over the whole torus, built from the seed and size.
  Unlike climate, it **changes the terrain**: erosion is the elevation
  field's erosion stage, the slot M10 left for it.

## Decision

### 1. A grid of 128-block cells; erosion on 256

`vox_worldgen::drainage::DrainageMap::build(elevation, climate, shape)`
samples the base elevation at cell centres: 128-block cells, 256 to 2 048
per axis (1 600×1 600 at the default size; 64-block cells in the smallest
world, 1 024-block in the largest). Rivers need cells finer than climate's
256. Erosion runs on 2×2 of them (256-block cells), four times cheaper; the
valleys it cuts are wider than that anyway once their sides retreat (§3).
Cell sizes are powers of two, so every lap of the torus samples the grids
at exactly the same fractions.

### 2. The pipeline

1. **Sea** is ground below sea level in bodies of at least 2 048 cells
   (~30 km²); smaller ones are inland pits, land like any other.
2. **Water balance**, per cell, from the climate map: potential evaporation
   from temperature (`climate::potential_evaporation`: zero below −6 °C,
   ~720 mm at 10 °C, ~1 400 at 25 °C); runoff by the Budyko curve in Pike's
   form (`climate::runoff`); a lake's cost, 1.2× potential evaporation less
   the rain on it, plus the runoff the cell no longer sheds. Biomes keep
   Köppen's threshold (ADR-0017); this is the water's own budget.
3. **Erosion** (`drainage::erosion`, §3): the cut, interpolated, is added to
   the base heights; everything after reads the eroded ground.
4. **Hollows** (`drainage::hollows`): a merge tree of the depressions
   (Barnes, Callaghan & Wickert, 2020), poured in the steady state of their
   Fill-Spill-Merge: runoff runs steepest-down to the hollow it ends in; a
   hollow that pays its lake's evaporation is full and spills to its
   sibling, their shared pool, or — at the top — downhill from its saddle
   into whatever lower hollow lies there; one that cannot pay settles where
   evaporation meets inflow, as a **salt lake or salt flat**, a closed
   basin. A hollow on ground whose aridity index (P/PET) is 0.3 or more
   never closes: Earth's endorheic basins lie almost all in arid land
   (the Great Basin ~0.2–0.3, the Aral ~0.1); wetter ones were cut through
   long ago. Full hollows hold fresh lakes only if deep
   basins (mean depth ≥ 60 blocks over ≥ 60 cells) or small ice-scoured
   hollows (below 2 °C, 3–200 cells, 10+ deep); the rest are breached.
5. **Routing**: one priority flood (Barnes et al., 2014) from the sea and
   the closed basins over the water surface. Lakes are entered last, and
   lakes and the hollows the flood fills are crossed first in, first out —
   the shortest way to their outlet, not down to a hollow's bottom and back
   up, nor snaking along rows in index order. Everywhere else each cell
   takes its lowest neighbour among the cells reached before it, so water
   runs the steepest way down and still forms a tree to an outlet.
6. **Channel levels** never rise downstream: a river cuts through a
   hollow's rim by at most 8 blocks (erosion did the deep cutting), and is
   graded to the sea's surface, level 0, never below it. Ground below the
   grade that results is filled — with a lake's backwater where it backs
   up from a lake, else with sediment to the grade, a flat alluvial floor
   (ADR-0020). Then each stream's level is **seated** in the
   full-resolution ground (`GENERATOR_VERSION` 5, after the owner's first
   flight found rivers on walls): outlets first, lowered until the line to
   its receiver runs a block under the lowest ground across its meander
   belt, sampled every 8 blocks; never below its receiver. A waterfall's
   lip is stored per cell (where the ground falls below its pool; cache
   format 3). ADR-0020 §9.
7. **Discharge** accumulated down the tree, lakes losing their
   evaporation, closed basins keeping what reaches them.

### 3. Erosion: implicit stream power, then walls retreat

Incision `E = K·Q^½·S`, solved implicitly outlets-first (Braun & Willett,
2013) — unconditionally stable, a knickpoint crosses the grid in one pass —
for 8 passes, re-routing between them. Driven by **discharge**, so wet
ground is cut deep and deserts hardly at all, with a floor of 100 mm/yr of
runoff everywhere: valleys are older than today's climate (the Sahara's
wadis ran in the Holocene). It never fills, never lowers a bed below the
one it flows into or the sea. Where a cut leaves a side steeper than 0.6
(and steeper than the ground was), the side retreats to it, so valleys
widen as they deepen and stay walkable (ADR-0010); slopes erosion did not
make are untouched.

The result is a cut per erosion cell. `Generator::surface` adds it,
bilinear, to the base `Elevation::surface`: **chunks, the LOD sampler and
every readout read eroded ground**; climate alone reads the base, as
ADR-0017 built it, so climate does not depend on the network built from
it. `GENERATOR_VERSION` 4.

### 4. Deterministic on every machine

Integer heights; one sequential flood and one sequential merge, ties
broken by cell index; float sums in a fixed order; a square root (exact in
IEEE 754) the only function beyond `+ − × ÷`. `rayon` only maps cells
independently and collects in order, as in ADR-0017.

### 5. Cost

Built in ~5–7 s at the default size in a dev build in the cloud
container, once per world: after that the cache (§6) loads it in ~50 ms.
Packed to ~11 bytes a cell — heights and levels as `i16` (the world's
vertical range fits), the receiver as one of eight directions, cover and
the sink flag in a byte, discharge as `f32`, plus the erosion grid at a
quarter of the cells: ~28 MB at the default size, in memory and on disk.

### 6. Stored as a cache, never the truth

The network is a pure function of `(seed, size, GENERATOR_VERSION)`. Per
the owner's decision (M14 spec, decision 3) it is cached in the world
folder as `drainage.cache` (`Generator::drainage_cached`): the magic
`VXDR`, its own format version, the generator version, seed and size it
was built for, the grids' dimensions, the arrays little-endian, and an
FNV-1a checksum. A file that is missing, of another format, built for
another world or generator, the wrong size for this world's grid, damaged
or truncated is rebuilt and rewritten (via a temporary file and a rename,
so a crash never leaves half a cache); one that cannot be written is
logged and costs the next load a rebuild. Never trusted over the
generator: it can only ever hold what the generator would build.

## Measured (default size, before the owner's walk)

Seed 0x7e22ac0de:

| | |
|---|---|
| Deepest cut | 496 blocks |
| Land cut 10+ / 50+ blocks | 20% / 4% |
| Valley depth by discharge (0.001 / 0.01 / 0.1 / 1+ m³/s) | 5 / 17 / 29 / 50 blocks |
| Steepest slope erosion adds (16-block step) | 0.82 |
| Lakes / salt lakes and flats | 11.4% / 0.8% of land |
| Largest river | 16.7 m³/s |

Six seeds (`drainage_survey`), against Earth (criterion 4):

| | Here | Earth |
|---|---|---|
| Hack's exponent | 0.54–0.55 | 0.5–0.6 |
| Lakes on ice-scoured land / elsewhere | 14–20% / 2–6% | Finland ~10%, the Shield more / a few % |
| Stream cells, arid / subhumid / humid | 0.6–1.0% / 2.4–3.1% / 3.3–3.9% | densest where wet |
| Land draining to closed basins | 16–36%, mean ~29% | ~18% |
| Longest flow path (straight source to mouth) | 95–204 km (31–93 km) | — |

The closed share is high because arid land is closed more often than
Earth's (62–72% of it here, about half on Earth) while being as common
(27–29% of land): the base terrain's continental interiors are enclosed.
The longest rivers exceed the spec's 40–70 km because this world's main
continents are ~100 km across, not the 34 km the estimate assumed.

## Consequences

- **Every surface query costs one bilinear lookup more** and requires the
  network: the first query builds it (seconds), so world load must, as it
  does for climate.
- **Tests that generate terrain build the network**: the worldgen suite
  takes ~45 s in a dev build in the cloud container.
- **The discharges are honest for our areas**: a large river is ~25 m³/s,
  because our catchments are ~10 000× smaller than Earth's. Widths and
  depths are scaled from discharge for play (task 3); erosion's reference
  discharge is set for these numbers, not Earth's.
- **What the base terrain lacks, the network shows.** The pre-erosion
  network drained 78% of land to closed basins, because the continental
  interior is a flat noise-pitted plateau at `LOWLAND_RISE`. Most of the fix
  was the budget; if closed basins or lakes stay off Earth's shares over
  many seeds (criterion 4), the elevation field's interiors are the next
  place to look, not the network's constants.
