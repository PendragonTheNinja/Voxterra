# ADR-0023: Landform processes — a fourth stage-1 map, and where landforms sit in the pipeline

- **Status:** Proposed (2026-11-10, M15 task 1). The map and its tests are
  built (`vox_worldgen::process`); nothing in generation reads it yet, so
  `GENERATOR_VERSION` is unchanged. To be accepted with M15's
  retrospective, as ADRs 0019 and 0020 were with M14's.
- **Context:** M15 gives each climate its own landforms
  (`docs/landforms.md`, approved 2026-11-10: "following closely to Earth's
  landforms, just on our scale"). Two things must be decided before any
  landform is built: how the generator knows *where* each process acts,
  and *when* in the pipeline a landform reshapes the ground — because
  M14's lesson was that water must fit the ground the drainage network
  saw, and a landform added afterwards moves the ground out from under
  the rivers.

## Decision

### 1. One weight per process, per place

Five processes shape the land: **wind**, **flash flood**, **ice**, **deep
weathering**, **rain**. Each landform layer (dunes, benches, troughs,
inselbergs, dissection) is multiplied by its process's weight, in `[0,
1]`. A layer never decides for itself where it applies; a place with no
weight for a process has none of its landforms.

### 2. The weights are a stage-1 map

`ProcessMap`, built at world load after the climate, on the climate
map's own grid (256-block cells at the default size), 5 bytes a cell.
Four of the five weights could be computed per column from maps that
already exist; wind cannot — see 4 — and one map keeps them consistent.

They are sampled bilinearly, at a column displaced by the biome-boundary
noise (`sample_wandering`), so an edge neither steps at a cell boundary
nor runs straight between cell centres (M14: anything read from a coarse
grid shows through as squares).

### 3. Each weight is its climate and its landscape

In the terms the biomes already use — aridity is precipitation over
Köppen's threshold; heights and relief are the *uneroded* base, as
climate reads them:

| Process | Weight |
|---|---|
| Flash flood | dry (aridity under ~0.75) × upland (above ~105 blocks) × off the mountains × thawed and never glaciated; stronger the rougher the ground, to badlands |
| Ice | ground whose **ice-age** mean annual temperature was below 0 °C (see 5) |
| Weathering | hot (≥ 20 °C) × aridity 0.65–2 (savanna, dry forest) × a plain × off the mountains × never glaciated |
| Rain | humid (aridity 0.8 → 3) × off the mountains |
| Wind | sand at rest (see 4) × desert-dry × lowland × off the mountains × thawed |

Wind and flash flood divide the dry lands by height (basins take dunes,
uplands take benches); weathering and rain divide the tropics by
rainfall; ice is independent of the other four.

### 4. Sand is carried, as rain is

A dune sea is where sand *ends up*. Along each row of the grid, in the
climate's own winds, sand is taken up from bare dry ground, carried, and
set down where the ground falls below what the wind has just crossed and
is flat enough to hold it; it crosses level ground without settling,
stays aloft over rough ground, is held by vegetation, and is lost at the
sea. This is ADR-0017's moisture sweep with a different cargo.

It is what makes dune seas patches in desert basins downwind of their
sand, rather than a noise mask over "desert": the catalogue's coverage —
about a fifth to a third of hot desert, a tenth of cold — comes out of
the mechanism with three constants tuned (measured over six seeds:
20–24% and 9–14%).

### 5. The ice age is today's climate, colder — where it also snowed

Ice-age temperature = today's − (4 °C to 20° latitude, rising to 11 °C at
50° and beyond). Ground is glaciated where that is below 0 °C: glaciers
flow below their snowline. This reproduces the catalogue's table to
within 30 blocks at every latitude (a test holds it there).

Two refinements beyond the catalogue's text, both Earth's:

- **Ice needs snow.** Cold alone built no ice sheet over Siberia. The
  weight falls away under ~275 mm of annual precipitation…
- **…unless nothing ever melts.** Antarctica's interior is drier than the
  Sahara and under four kilometres of ice. Below an ice-age mean of about
  −12 °C cold alone suffices — which also keeps the weight at 1 wherever
  M13's ice-sheet biome stands today.

In worlds so far the mid-latitude storm track is wet enough that the
first refinement removes little; it is there for the rain-shadowed
interior that a seed will eventually produce.

### 6. Landforms come before the drainage network; ice works inside it

The order of generation becomes:

1. base elevation (ADR-0010);
2. **landform layers** weighted by this map — dunes, benches and mesas,
   inselbergs, dissection — as *heights* added to or cut from the base
   (`docs/vision.md` §10.1: blend heights, never densities);
3. the drainage network and stream-power erosion, built on that ground
   (ADR-0019), so rivers cut canyons through the benches, flow round the
   inselbergs and die in the dune seas;
4. **glacial reshaping inside the erosion stage**, weighted by the ice
   weight: the network's own valleys widened to troughs, cirques at their
   heads, basins overdeepened — ice occupied the valleys rivers had made;
5. rivers, lakes and wetlands (ADR-0020), unchanged.

Every layer in step 2 is a pure function of `(x, z)` and stage-1 maps, so
any chunk still generates alone and the LOD samples the same ground.

## Consequences

- **Stage 1 gains a dependency edge**: process map ← climate ← elevation,
  and (from task 2) drainage ← landform-shaped ground ← process map. The
  drainage cache's key must then include whatever the layers depend on;
  it already carries the generator version, which every landform change
  bumps.
- **The drainage network gets finer ground to read.** Its cells are 128
  blocks (to ~1 000 in the largest worlds); a dune ridge is 100–250 apart
  and a bench riser is a step. The network must see landforms at the
  scale it can — the dune sea as a low sandy plain, the plateau with its
  benches averaged — or it will route rivers along artefacts. Task 2's
  first job is to decide what the network samples: the layered ground
  smoothed to its cell, most likely.
- **World load pays ~0.3 s more** at the default size (measured in a
  release build; dev builds a few times that).
- **Climate bands show.** The weights inherit M13's zonal climate, so
  processes lie in latitude belts broken by mountains and coasts. That is
  Earth's pattern too; if it reads as stripes on foot, the fix belongs in
  the climate (ADR-0017), not here.
- **The catalogue is amended** with the two ice refinements and with
  "thawed" for the dry-land processes (a polar desert has no flash floods
  and no dunes), under *As built*.

## Alternatives considered

- **Weights per column, no map.** Simpler, and exact rather than
  interpolated — but wind has no local formula, and four local weights
  beside one mapped one would disagree at their edges.
- **A noise mask for dune seas.** The usual way. It puts sand anywhere in
  a desert with equal likelihood; Earth puts it in basins downwind of its
  source, and a geologist would see the difference on the map.
- **Landforms after the network.** Cheaper (the cached network survives a
  landform change) and what M14's first river placement did; M14 spent
  four generator versions undoing its consequences.
