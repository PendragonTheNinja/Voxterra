# ADR-0021: Landforms as height layers, weighted by stage-1 processes

- **Status:** Proposed (2026-11-04, M15 task 1). Accepted with M15's
  retrospective, after the landform layers and the owner's walk or flights.
  Sections 1–3 are built (task 1); sections 4–5 record the order the later
  tasks must follow. Dunes (task 2a, version 9) are the first layer built;
  see §4's note.
- **Context:** After M13 and M14 every climate has the same ground: one
  elevation field, coloured by biome and cut by rivers. M15 gives each
  climate its own landforms (`docs/landforms.md`, approved 2026-11-04):
  dune seas, mesas and canyons, glacial valleys and fjords, inselbergs,
  dissected humid hills. Three constraints from earlier decisions shape how:
  any chunk generates alone and the LOD samples the same ground (CLAUDE.md,
  ADR-0008); stage 2 reads stage-1 maps but a coarse grid must never show
  through as squares (M14's lesson); and water must fit the ground, never
  the reverse (M14, ADR-0019).

## Decision

### 1. Five process weights per column, from the base ground and stage 1

`Generator::processes(x, z)` gives each column a weight in `[0, 1]` for
each of the catalogue's five processes — wind, flash flood, ice, deep
weathering, rain (`vox_worldgen::landform`). A landform layer reads only
its own process's weight; no landform is placed by a noise field of its
own (the catalogue's rule: made by processes, never scattered).

The inputs are the **elevation field's own surface** (height, relief,
uplift), the stage-1 climate (with M13's boundary noise, through the
biomes' own jitter), the latitude, and two stage-1 fields of the
landforms' own (§2). Never the landformed ground, and never the drainage
network's erosion, which is built on that ground: a mesa must not raise
its own flash-flood weight. Weights are therefore fixed before any
landform exists, and every layer can be computed in any order.

Each weight is a product of smoothsteps over continuous inputs — the
climate and process maps bilinear between cell centres, the surface the
elevation field's — so no edge follows the grid. The thresholds are the
catalogue's table, tuned in task 1 (§3).

### 2. Stage 1 adds basins and sand on the climate's grid

`ProcessMap` (built at world load after the climate, ~80 ms at the default
size, shared by every clone of the generator). Per climate cell, carried
along each row with the climate's own winds (westerlies toward +x where
`eastward_share` is 1, easterlies where 0, blended between):

- **basin** — how far the cell lies below the mean ground upwind
  (exponential over 6 km at the default world's continent size, sea
  counting as sea level), full at 60 blocks: the dune sea's "lower than the
  ground upwind".
- **sand** — the sand source (dry lowland: aridity under 0.3–0.6, under
  100–200 blocks) carried downwind over 10 km: dune seas lie downwind of
  their source.

Both scale with `Elevation::continent_scale` as the climate's distances do
(ADR-0017 §5), and are blurred as the climate's fields are. Desert rivers'
beds, the catalogue's other sand source, are not an input: the network is
built later, on landformed ground (§4). The map is rebuilt each load (it is
cheap), not cached.

### 3. The ice-age climate, with snow

Ice's weight comes from the **last ice age's** mean annual temperature:
today's less ΔT, with ΔT 4 °C up to 20° of latitude rising smoothly (in
`sin(latitude)`) to 11 °C at 50° and beyond. At a reference 1 000 mm of
precipitation ground is glaciated (weight 0.5) a little above 0 °C, which
reproduces the catalogue's table exactly (ice-age ice above ~720 blocks at
the equator, ~410 at 30°, ~165 at 40°, sea level from ~47°; a test pins
it).

Task 1 added what the catalogue did not have, because the first map
glaciated 54% of land with every cold interior under ice: **snow decides
where ice lies as much as cold does.** A glacier's equilibrium line sits at
warmer temperatures where more snow falls (Ohmura et al. 1992), so the
glaciation threshold shifts with today's precipitation (−8 °C at 100 mm,
0 at 1 000, +2.5 at 4 000), and below 200–450 mm no ice sheet grows however
cold it is — Siberia and Beringia stayed ice-free. Where ice lies **today**
(M13's ice sheet, below −14 °C at the column's height) it lay then too,
however little snow falls (Antarctica, Greenland), and under it the other
four weights are zero.

Measured (land with ice weight ≥ 0.5): 23–41% over four seeds at the
default size, 27–32% in the smallest worlds, 34% at twice the default —
Earth's last glacial maximum ~25–32%. M13 already puts 12% of land under
ice today (Earth ~10%).

### 4. The order: landforms, then the network, then ice inside erosion

1. base elevation → **landform layers** (dunes, terraces and mesas,
   inselbergs, dissection), each a height added to the base surface,
   weighted by its process; layers blend as heights, never densities
   (`docs/vision.md` §10.1);
2. → the **drainage network and erosion stage** (ADR-0019), built on that
   ground — rivers cut canyons into mesas, flow round inselbergs, die in
   dune seas;
3. → **glacial reshaping inside the erosion stage**: ice widens and deepens
   the network's own valleys into U-shapes, cuts cirques at their heads,
   overdeepens basins that hold tarns and fjords — on the valleys it
   reshapes, which only exist after step 2;
4. → M14's rivers, lakes and wetlands, unchanged.

**Dunes are the exception to step 1 (task 2a).** Ridges 100–250 blocks
apart are finer than the network's 128-block cells, so the network could
only see them as noise. They are laid in stage 2 after the water, on dry
ground only, faded out toward each river's bank: they only ever raise
ground the water was fitted to, so nothing is held by air, and rivers keep
to the interdune corridors. Landforms at or above the network's scale
(benches, mesas, inselbergs, dissection) are still applied before it.

**Mesas raise, they never dig (task 2b).** The network sees the
landformed ground (`Generator::landformed`, `DrainageMap::build_on`).
A first draft stripped the floors between mesas down to 120 blocks below
the plateau; the network filled those bowls, and 2–6 more points of land
drained to closed basins (criterion 6 asks for fewer). Raising the
caprock over unchanged floors left the share as it was. Terracing keeps
heights in order, so it adds no pits either. Every later layer before the
network should hold to the same rule: raise, or reorder nothing.

**Glacial troughs are lines, not a grid (task 3a).** Step 3 runs inside
the network's build, after the rivers' erosion and before routing, so
lakes fill the overdeepened basins and long profiles are seated on the
trough floors. A trough is stored per glaciated drainage cell as a line
along the network's own jittered nodes and evaluated at full resolution
(the ground is the lower of its own and the U's), never interpolated
from a grid of carved heights: at 128 blocks that grid showed through as
squares. Troughs only where valleys have relief to channel the ice.

The climate map and the process weights read the base elevation (step 0),
never steps 1–3: landforms are smaller than climate cells and would only
add noise to rain, and reading them back would make weights depend on
what they shape.

### 5. Pure functions of `(x, z)`

Each landform layer is a pure function of the column and the stage-1 maps
(weights, climate, wind), tiled with the world's period, so the chunk
generator, the LOD sampler and every readout see the same ground, and any
chunk still generates alone. A landform that needs neighbours (a dune's
lee face, a mesa's caprock edge) computes them from `(x, z)` as the
meander warp does — never from generated chunks.

## Consequences

- Process weights are an instrument before they shape anything: the world
  map's `processes-*` and `ice-age-*` layers (`world_map`, `process_map`)
  show where each landform may form before any is built, and every
  where-Earth-puts-it test (criterion 3) conditions on them.
- Task 1 changes no terrain: nothing reads the weights yet, so
  `GENERATOR_VERSION` stays 8 until the first layer (task 2) bumps it.
- The drainage network must be rebuilt when landforms change (it already
  is per generator version: its cache key carries the version).
- "Mountains" for the processes means real ranges (uplift 200–400+ blocks),
  not any uplift: almost all high ground in this generator is raised by
  orogeny, so excluding any uplift excluded nearly every dry upland from
  flash floods (1.4% of land; 2.5% after).
- Rejected: a landform noise field per landform (scatter, against the
  catalogue's rule); landforms after the network (the network would not see
  them — M14's walls and causeways again); glaciation by temperature alone
  (54% of land under ice, every cold desert glaciated).
