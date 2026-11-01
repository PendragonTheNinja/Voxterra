# Milestone 14 — Rivers, Lakes & Valleys

**Goal:** water that behaves as it does on Earth. Rain that falls on the land
(M13's precipitation) drains downhill, gathers into streams and rivers that
grow as they go, carves the valleys they run in, fills lakes in the hollows,
spreads into wetlands where the ground is flat and wet, and reaches the sea
through deltas and estuaries — each climate zone with Earth's own kind of
water: the Amazon's slow giants, the desert's dry wadis and salt pans, the
north's ten thousand lakes, the mountains' torrents and tarns. At our scale
(ADR-0010): mechanisms real, dimensions chosen for walking.

Depends on M10 (the elevation field, ranges), M13 (precipitation and
temperature per column, the stage-1 pattern of ADR-0017). Written 2026-11-01
from `docs/vision.md` §1.3 and §9, at the owner's request.

**Status: spec approved (2026-11-01)** — decisions at the end. Task 0
includes a water-physics design document, which the owner decides before
task 3 (see "Water physics: decided later, constrained now").

## Why this milestone exists

The world has continents, ranges and climates, and no water on land. Every
hollow below sea level is sea; every hollow above it is dry. Rivers are the
largest single thing missing between this world and Earth — they decide
where the valleys are, where the flat fertile ground is, where wetlands and
deltas form; later, where settlements, placer gold (geology), river travel
and crops go. And a river is the textbook case CLAUDE.md built two-stage
worldgen for: **it must know its upstream**, which no chunk can see.

## Earth, zone by zone — what "right" looks like

The yardstick for every acceptance criterion. Earth's figures are given, then
what they become at our scale (~100× horizontal compression, walking-time
dimensions — ADR-0010; continents here are 30–70 km across).

| Zone | Earth's water | Here, at our scale |
|---|---|---|
| **Tropical rainforest** | The densest river networks on Earth; huge, slow, meandering rivers with wide floodplains, oxbow lakes, seasonally flooded forest (várzea); muddy brown water | The widest rivers in the world (30–60 blocks), sinuous, slow; floodplains of silt; oxbow lakes; many tributaries |
| **Savanna, dry forest** | Fewer, seasonal rivers; wide sandy beds, gallery forests along them | Wide shallow sandy channels, sparse network |
| **Hot desert** | Few rivers rise there: dry wadis that run only after storms; **endorheic basins** that never reach the sea, ending in salt pans and salt lakes (Chad, Aral, Lop Nur); "exotic" rivers fed from wetter lands that cross deserts as green ribbons (the Nile); canyons in plateaus | Dry channels of sand and gravel; closed basins with salt flats; any river crossing from a wetter zone keeps flowing, losing water, and may end in a salt lake |
| **Mediterranean** | Short, steep rivers; gorges; many dry in summer | Short steep streams in narrow valleys |
| **Temperate** | Meandering rivers in broad floodplains on lowland; V-shaped valleys in hills; lakes behind moraines | Moderate rivers (10–30 blocks) meandering across plains, V-valleys in hill country |
| **Boreal** | Deranged drainage on ice-scoured ground: lakes everywhere, connected by short rivers; bogs and fens | Lakes as common as land in places; bogs on flat wet ground |
| **Tundra** | Braided rivers on gravel; countless shallow thaw ponds | Braided channels; shallow ponds scattered across flat ground |
| **Ice sheet** | No rivers on the ice; meltwater at the margins | None on the ice |
| **Mountains** | Steep torrents in V-valleys, waterfalls and rapids where rock steps; alpine lakes (tarns) in high hollows | Narrow fast streams, waterfalls at steps, small high lakes |
| **Coasts** | Big sediment-laden rivers build **deltas** (Nile, Mississippi, Ganges); drowned river mouths become **estuaries** (Thames, Chesapeake) | Large rivers fan into deltas of mud and sand with distributary channels; others meet the sea in wide estuaries |

**River size at our scale:**

| | Earth | Here |
|---|---|---|
| Longest rivers (Amazon, Nile) | 6 400–6 650 km | ~40–70 km — the length of a continent, an hour or two on foot |
| Large river (Rhine, Danube) | 1 200–2 800 km | 10–30 km |
| Small river (Thames) | 350 km | 3–5 km |
| Width of a great river's main stem | 1–5 km | 30–60 blocks |
| Width of a large river | 200–800 m | 10–25 blocks |
| Width of a stream | 2–10 m | 1–3 blocks |
| Depth | 5–50 m | 2–12 blocks (deep enough to swim; boats later) |

Width and depth follow Earth's hydraulic geometry — width grows as the
square root of discharge, depth as its 0.4 power — scaled so the largest
river is the 30–60 above, and discharge is M13's precipitation summed over
the river's whole catchment, minus evaporation (dry climates lose water;
exotic rivers shrink as they cross a desert).

## How it works

### Stage 1: the drainage network (world creation)

A drainage grid over the whole torus, finer than the climate grid (rivers
need it — target 64–128 blocks per cell; the ADR fixes it by measurement):

1. **Fill depressions** (priority-flood): every hollow above sea level gets a
   spill level. Hollows whose water budget (precipitation in, evaporation
   out, M13's climate) overflows become **lakes** with an outlet; those that
   cannot fill — arid **endorheic basins** — become **salt lakes or salt
   flats** at the level their budget reaches.
2. **Route flow**: every cell drains to its steepest downhill neighbour
   (through lakes, via their outlets), the flow direction forming a tree that
   ends at the sea or in a closed basin.
3. **Accumulate discharge**: precipitation × area, summed downstream, less
   evaporation by climate. Cells above a threshold of discharge are
   **rivers**; the threshold sets drainage density, varied by climate as on
   Earth (dense in the wet tropics, sparse in deserts).
4. **Carve valleys** (stream-power erosion): incision proportional to
   discharge^m × slope^n, run for a few iterations on the coarse grid, so
   rivers sit in valleys whose width and depth grow downstream, and
   mountains are cut by them. Erosion is the one stage that **changes the
   elevation field** (`Elevation` gains its "erosion stage" — M10 left the
   slot); climate keeps reading the uneroded base (ADR-0017).
5. **River mouths**: a large river reaching a gentle coast with abundant
   sediment builds a delta (distributaries fanned over a lobe of new land); a
   steep or drowned mouth stays an estuary.

Deterministic like the climate map (basic arithmetic, no reductions across
threads); built at world creation, budgeted (open question 3), rebuilt on
load or cached as a derived, versioned file — the ADR decides.

### Stage 2: rivers and lakes in the chunks

Each column asks the network what is near it — **pure functions of (x, z)**,
so chunks, LOD and every reader agree:

- **Channels follow the network's polylines, meandering** by a deterministic
  curve whose wavelength scales with width (real meander wavelength ≈ 10–14×
  channel width); a column within a channel's half-width is river bed, its
  depth from the hydraulic geometry, its banks shaped.
- **Water fills to the local water level**, not to sea level: a river's
  surface follows its long profile down to the sea; a lake's is its spill
  level. This is the biggest change to the generator's fill rule since M10
  added the sea.
- **Floodplains**: flat ground beside lowland rivers, silt and clay over
  gravel; **oxbow lakes** in old meanders of large rivers.
- **Waterfalls and rapids** where a river crosses a step in its long profile.
- **Materials**: river gravel and sand in channels, silt on floodplains, mud
  in deltas, salt on salt flats, peat in bogs (new blocks — open question 2).

### Kinds of river

Earth's rivers differ because their slope, discharge, sediment and climate
differ — and the network knows all four for every reach, so the kinds fall
out of the same numbers rather than being placed:

| Kind | When (per reach) | Earth | What it means here |
|---|---|---|---|
| Mountain torrent | steep, small | Alpine streams | narrow, shallow, fast; rapids where it steps, waterfalls at cliffs |
| Meandering | gentle slope, cohesive banks, lowland | Mississippi, Amazon | sinuous loops ~10–14× its width, oxbow lakes, floodplain |
| Braided | steep-ish, much coarse sediment, weak banks — glacial, arid, tundra | Rakaia (NZ), Alaskan rivers | many channels splitting and rejoining over a wide gravel bed |
| Slow, lake-like reach | almost flat — behind a natural dam (a landslide, a moraine, a tributary's fan, a beaver dam in the boreal), or a river widening into a lake | Lake Pepin on the Mississippi, the Lake District | wide, deep, nearly still water that keeps draining at its outlet |
| Exotic / wadi | crossing or within a desert | Nile; dry wadis | shrinking as it evaporates; dry channels of sand and gravel |

Speed comes from slope and depth (Manning's relation, roughly speed ∝
depth^⅔ × slope^½): a torrent runs fast and shallow, a lowland giant deep and
slow. Depth and width come from discharge. The network stores speed and
direction per reach — the hybrid needs them anyway.

**Water that looks like it flows**: the water surface's texture drifts along
the river's direction at its speed — a slow lowland river barely moving, a
torrent racing, foam where it is fast or steps — read in the shader from a
flow map of the network, the way grass reads the climate map (ADR-0018), so
it costs one texture sample per water pixel, re-meshes nothing, and is the
same near and far. Still lakes and the sea stay still.

### Wetlands and water biomes

Hydrology makes the biomes M13's catalogue deferred possible. Proposed
additions to `docs/biomes.md` (open question 2):

| Biome | Where | Earth analogues |
|---|---|---|
| River | the channel | — |
| Freshwater lake | filled basins | Great Lakes, Baikal, tarns |
| Salt lake / salt flat | endorheic basins in dry climates | Great Salt Lake, Salar de Uyuni, Lop Nur |
| Floodplain | flat ground beside lowland rivers | the Nile valley, the Mississippi bottomlands |
| Marsh | flat, waterlogged, warm or temperate, open | the Camargue, the Everglades' sawgrass |
| Swamp | flat, waterlogged, wooded, warm | the Atchafalaya, the Pantanal |
| Bog / fen | flat, waterlogged, cold: peat | the Hudson Bay lowlands, Irish bogs |
| Delta | large river mouths on gentle coasts | Nile, Mississippi, Ganges, Okavango (inland) |
| Mangrove coast | tropical delta and estuary shores | the Sundarbans (mangroves themselves with vegetation) |

### Distant water

The LOD must show rivers, lakes and salt flats to the horizon: lake and river
surfaces at their own level (the LOD sea surface generalised to a per-cell
water level), large rivers visible as ribbons for kilometres, small streams
fading out with distance as they do on Earth.

## Water physics: decided later, constrained now

How water *behaves* — whether it flows when a dam is broken, how far, whether
it is finite — is a game-defining choice the project has not made (owner,
2026-11-01: "as simple as Minecraft's, or something much more complex").
It is a simulation (`vox-sim`, later), so M14 builds none of it. But M14
fixes how worldgen water is **stored**, and that must not close any door:

- **Minecraft-like:** each water block is a source or a flowing level
  (0–7); sources are infinite; a river is a staircase of sources. Simple,
  cheap, familiar; water never runs out, a bucket can drain nothing.
- **Finite volume** (cellular automaton): each cell holds an amount of
  water that moves to neighbours by pressure and gravity; water is
  conserved, levels are fractional, a breached lake drains into the valley
  below. Realistic and spectacular, costly, and hard at 64 km of view.
- **Hybrid** (the likely answer, `docs/vision.md` §1.3): worldgen water
  sleeps until disturbed; near a disturbance a local finite simulation
  runs; seas, rivers and large lakes act as reservoirs that refill from the
  network (a river keeps flowing because the drainage network says so).

What M14 must do so any of them stays possible:

- store worldgen water as **full, settled blocks** (no partial levels yet),
  with surfaces stepping down a river's long profile in whole blocks —
  small rapids where it steps;
- keep, for every river and lake, its **water level, flow direction,
  speed and discharge** in the drainage network (stage 1), so a later
  simulation knows which water is a reservoir and where it is going;
- add no per-block water state that one model would need to undo.

**Task 0 therefore includes a design document, `docs/design/water.md`,**
comparing the three models on realism, fun, cost at our view distance,
interaction with lighting, meshing and the LOD, and the work each implies —
for the owner to decide before task 3 fixes the fill rule. The decision
becomes an ADR; the simulation itself remains its own milestone.

## Acceptance criteria

1. **Spec, catalogue additions and ADRs first**: the biome additions
   approved; an ADR for the drainage stage (grid, storage, cost) and one for
   water levels above the sea (the fill rule, lighting and the LOD).
2. **Instruments first**: the world map gains drainage, discharge, rivers,
   lakes and basins; the readout names the river or lake the player is in.
3. **Water runs downhill, always**: every river cell's surface is at or
   below its upstream neighbour's; every river reaches the sea, a lake, or a
   closed basin; no lake above its outlet; tested on many worlds.
4. **Earth-like networks**, measured over many seeds and compared to Earth
   at our scale: drainage density by climate (tropics densest, deserts
   sparsest); the river length–catchment relation (Hack's law, length ∝
   area^~0.6); a longest river per world in the range above; a share of
   land in closed basins near Earth's (~18% of Earth's land drains to no
   ocean); lakes most common on boreal and tundra ground.
5. **Earth-like rivers**: width and depth from discharge; meanders of the
   right wavelength on lowland, straight and steep in mountains; deltas only
   at large mouths on gentle coasts; exotic rivers crossing deserts and
   shrinking.
6. **Valleys**: rivers sit in valleys that deepen downstream; mountains are
   cut by them; the elevation field stays smooth and walkable (ADR-0010's
   slopes), measured at ground speed.
7. **Every world, every size**: rivers, lakes and the new biomes in every
   legal world size, as M13 did for climate.
8. **Near and far agree**: the LOD shows water surfaces at their own levels;
   no gap or step where full resolution meets it.
9. **Sound and fast**: per-chunk independence and determinism hold;
   `GENERATOR_VERSION` bumped with the fingerprint re-pinned; world creation
   within its budget; `--bench` within ~10% of M13's close.
10. **Rivers differ as Earth's do**: torrents, meandering, braided and
    slow reaches each appear where their slope, discharge and sediment put
    them, measured over many worlds; river water visibly drifts downstream
    at its speed, lakes and the sea do not.
11. **The owner's walk**: on foot, following a river from a mountain spring
    to the sea; retro with maps before and after.

## Tasks (proposed order)

0. Catalogue additions (approved 2026-11-01) into `docs/biomes.md`; the
   water-physics design document for the owner's decision; instruments
   (maps of drainage and water).
   - [x] Catalogue additions in `docs/biomes.md` (2026-11-01).
   - [x] `docs/design/water.md` (2026-11-01): why Minecraft-style games keep
     water simple, the three models, and the recommendation — the hybrid
     (settled worldgen water; local finite water in eighths where disturbed;
     seas, rivers and large lakes as reservoirs) with the gameplay it buys
     (flooding mines, draining and damming, irrigation, water power, wells).
     **Decided (owner, 2026-11-01): the hybrid.** The ADR is written with
     task 3, where the storage it constrains is built.
   - [x] Drainage and water layers on the world map — with task 1's network
     (`rivers-<seed>-<size>.png`: rivers by discharge, lakes).
1. Stage-1 drainage: depression filling, flow routing, discharge, lakes and
   closed basins (ADR). Network tests (criteria 3, 4).
   - [x] The network (`vox_worldgen::drainage`): 128-block cells over the
     torus (1 600² on the default world, ~2 s); sea is ocean-connected
     ground below sea level, inland pits are land; runoff from M13's
     climate less Köppen's evaporation; a merge tree of the hollows decides
     lakes (deep basins anywhere, small hollows on ice-scoured ground) and
     breaches the rest; routing by priority flood over the water surface,
     channels cut so water never runs uphill; discharge accumulated.
     Tests: downhill to the sea, conservation, determinism, lakes commonest
     where ice-scoured.
   - [x] Closed basins by water budget (`drainage::hollows`): the steady
     state of Fill-Spill-Merge (Barnes et al., 2020) over the depression
     tree — runoff runs to the hollow it ends in, a hollow full to its spill
     overflows to its sibling or their shared pool, one that cannot fill
     settles where lake evaporation (2× Köppen's threshold) meets inflow:
     a salt lake, or a salt flat. Closed basins are never breached and are
     the network's other outlets. Tests: downhill to the sea or a closed
     basin; every drop reaches one, less what lakes evaporate.
   - Measured before erosion: **78% of land drains to closed basins**
     (Earth ~18%). The cause is the terrain, not the budget: the flat
     interior plateau is pitted everywhere, every pit a tiny closed basin
     on subhumid ground (35% of land is arid, 25% subhumid). So valley
     erosion (task 2) comes next, before flow speed, the cache and the
     network's Earth comparison, all of which are measured on the eroded
     terrain.
   - [x] Cached in the world folder (`drainage.cache`, ADR-0019 §6):
     ~28 MB at the default size, loaded in ~50 ms against a ~6.7 s build
     (dev); rebuilt when stale, damaged or another world's. The network is
     packed to ~11 bytes a cell in memory too.
   - [x] ADR-0019 (drainage and the erosion stage) proposed; accepted after
     the owner's walk.
   - [x] Network tests over many seeds against Earth (criterion 4):
     `drainage_survey` (ignored; six seeds) and `the_network_is_earth_like`.
     Hack's exponent 0.54–0.55; lakes 14–20% of ice-scoured land against
     2–6% elsewhere; stream density rising from arid to humid. **Closed
     basins 16–36% of land, mean ~29%** (Earth ~18%): arid land is as
     common as Earth's but closed more often (62–72% of it, Earth about
     half) — the base terrain's enclosed interiors; carried to M15
     (landforms by climate). **Longest rivers 95–204 km**, not 40–70: the
     main continents are ~100 km across (source to mouth 31–93 km straight).
     Routing crosses lakes and filled hollows first in, first out (it had
     snaked: paths were 2–3× their straight length); the closure threshold
     is an aridity index of 0.3.
   - Measured before erosion: lakes cover ~27% of ice-scoured land, ~6%
     elsewhere (Earth: ~3.7% of unglaciated land). The excess is the flat,
     noise-pitted continental interior (`LOWLAND_RISE`), deranged until task
     2's erosion cuts it; re-measure then.
2. Valley erosion on the stage-1 grid, fed back as the elevation field's
   erosion stage (criterion 6).
   - [x] Erosion (`drainage::erosion`): implicit stream power (Braun &
     Willett, 2013; `m = ½`) on a grid of 2×2 drainage cells (256 blocks),
     8 passes of route-then-incise, driven by discharge with a paleo floor
     of 100 mm/yr (valleys are older than today's climate); valley sides
     erosion leaves steeper than 0.6 retreat to it, slopes it did not make
     are untouched. `Generator::surface` adds the cut, bilinear, to the
     base: chunks, the LOD and readouts read eroded ground; climate reads
     the base. `GENERATOR_VERSION` 4.
   - [x] The water balance it needed: potential evaporation from
     temperature (0 below −6 °C, ~720 mm at 10 °C, ~1 400 at 25 °C), runoff
     by the Budyko curve (Pike, 1964), lake evaporation 1.2× potential.
   - [x] The budget fixed: overflow onto draining ground runs on downhill
     from its saddle into lower hollows (it had been assumed to reach the
     sea); hollows on ground with an aridity index ≥ 0.5 never close; the
     routing takes the steepest way down among cells reached before.
   - Measured, default world: deepest cut 496 blocks; 20% of land cut 10+
     blocks, 4% 50+; valley depth (ground three cells around less the
     river) 5 / 17 / 29 / 50 blocks for 0.001 / 0.01 / 0.1 / 1+ m³/s; the
     steepest slope erosion adds 0.82 over 16 blocks. 32% of land drains to
     closed basins (78% before erosion and the budget's fixes); lakes 11.4%
     of land, salt lakes and flats 0.8%; the largest river 16.7 m³/s.
     Built in ~5–7 s at world load the first time; cached after.
   - [ ] Criterion 6 at ground speed: the owner's walk.
3. Stage-2 channels: polylines, meanders, banks, depth, water levels; the
   fill rule above sea level (ADR); materials.
   - [x] Channels (`drainage::rivers`, ADR-0020 proposed): reaches from
     jittered cell centres, meander warp at ~12× width, width `8·√Q`, depth
     `2.8·Q^⅜`; settled water floored to whole blocks down the long
     profile; beds of river gravel, sand or silt by slope; banks and levees
     (water never held by air, tested); lakes filling to their level along
     the ground's contours, freezing below −2 °C; salt flats; new blocks
     silt, salt, river gravel.
   - [x] The network's levels fixed for it: the sea's surface is level 0
     and never cut (rivers had been graded to inland pits' floors, 50–100
     blocks under the land); rim cuts at most 8 blocks; hollows below a
     river's grade filled with sediment (flat floors) unless backed up from
     a lake. Lakes on unglaciated land back to 1–4% (they rose to ~12% in
     between). Honest valley depth: ~3 blocks for rills, 11–13 for larger
     rivers — criterion 6's "deepen downstream" is only partly met.
   - [x] Streaming spans standing water (`surface_span_chunks`): a lake's
     surface had stood in layers never loaded. Found in a render; a test
     that failed first.
   - [x] Flow speed and direction per reach (`DrainageMap::flow`,
     Manning's relation) and **water that visibly flows** (the flow map,
     `flow.wgsl`, ADR-0020 §6): river water drifts downstream at its speed
     near and far, foams where fast; lakes and the sea are still. Checked
     headlessly: between two water-clock times only water pixels change.
   - [x] River kinds (criterion 10, ADR-0020 §7): wadi, braided, torrent,
     meandering from slope, discharge and arid/ice-scoured ground; slow
     reaches are the lakes rivers cross. Every kind in every surveyed world
     (`drainage_survey`: torrent 13–18 k cells, braided ~1 k, meandering
     31–41 k, wadi 1–2 k); torrents run higher than meandering rivers (133
     against 42 blocks, `rivers_differ_by_slope_discharge_and_sediment`).
   - [ ] Reservoir or finite per body — for the simulation (layer 2) when
     it is built; the network has what it needs (discharge, cover).
   - [ ] Cost: `ground` ~2× the eroded surface per column; measure with
     `--bench` (owner).
4. Lakes, salt flats, floodplains, wetlands, deltas, waterfalls; the new
   biomes.
   - [x] The nine water biomes (`docs/biomes.md`, "Water biomes as
     built"): river, lake, salt lake/flat from the column's water;
     floodplains, wetlands (marsh, swamp, bog by temperature) and deltas
     (mangrove in the tropics) from the nearest river — flattened at its
     level, wetlands waterlogged with shallow pools that only form where
     every side holds them. Peat. All 32 biomes in every legal world size;
     the delta threshold scales with world area.
   - [x] Backed-up water is a lake even where a stream crosses it (stream
     cells had been left as straight strips of land across a lake).
   - [x] Waterfalls (ADR-0020 §8, owner's decision 2026-11-01: static
     sheets until water can fall): at steps in the long profile (12+
     blocks, twice the river below), a sheet over a cliff; water's side
     faces run down in the shader, white. ~1 900 on the default world.
     Rapids are the whole-block steps elsewhere. The fingerprint now
     samples water.
   - [ ] Deltas' distributary channels; oxbow lakes.
5. Distant water in the LOD (criterion 8).
   - [x] The LOD's water at each cell's own level (`mesh_lod_cells`,
     `LodCells`; `lod_surface` returns the water of the column it sampled):
     rivers and lakes continue past full resolution; frozen lakes rise to
     their ice. Tested (`lod_water_is_the_columns_water`, vox-mesh
     `lod_water_stands_at_each_cells_level`); seen in renders.
   - [ ] Large rivers as ribbons for kilometres: today a river is drawn
     where a sample lands in it — at far levels, by chance. Judge on the
     owner's walk before building more.
6. The owner's walk, `--bench`, retro.

## Non-goals

- **Flowing water** — water you can pour, dams that break, floods. That is a
  simulation (`vox-sim`, much later, `docs/vision.md` §1.3). Worldgen water
  is settled until disturbed.
- **Currents that push the player** — later, with boats (owner,
  2026-11-01); the network carries a flow direction and speed per channel,
  so they can be added without regenerating.
- **Climate-shaped landforms** — dune seas, mesas and buttes, canyonlands,
  glacial U-valleys, fjords, cirques: **M15** (owner, 2026-11-01).
- **Seasons**: rivers do not rise and fall; wadis are dry.
- **Vegetation** (gallery forests, reeds, mangroves themselves), **caves and
  groundwater** (geology), **placer gold** (geology, which will read this
  network).
- **Water that absorbs with depth** — still the fix for grey shallows; could
  ride along if the owner wants (water is everywhere in this milestone).

## Notes

- **Water levels above the sea touch lighting and the LOD** (CLAUDE.md's
  invariants): water is rendered but not solid or opaque (ADR-0011), and the
  skylight heightmap asks `opaque`; a lake must be lit through, its bed
  shaded with depth like the sea's. Write the tests that model a lake at a
  chunk seam before the code.
- **Climate reads the uneroded base; rivers read the eroded field.** No cycle.
- **Small worlds**: a 32 km world's continents are ~10 km; Hack's law
  still holds, rivers are just shorter. Calibrate only where needed (M13).
- **Judge at walking speed** (5.612 m/s): following the longest river to the
  sea should take an hour or more on foot.

## Decisions (owner, 2026-11-01)

1. **Two milestones**: M14 is water and the valleys it carves; **M15** is
   landforms by climate, built on M14's network.
2. **All nine water biomes** join the catalogue (river, freshwater lake,
   salt lake / salt flat, floodplain, marsh, swamp, bog / fen, delta,
   mangrove coast), with silt, peat, salt and river gravel.
3. **The drainage network is cached in the world folder** — derived from the
   seed and size, versioned, rebuilt automatically if missing or stale.
4. **Currents later, with boats**; flow is recorded now.
5. **Water physics: the hybrid** (`docs/design/water.md`) — settled
   worldgen water, local finite water where disturbed (a later milestone),
   seas, rivers and large lakes as reservoirs.
6. **Rivers differ by terrain, and look like they flow** (owner's question,
   2026-11-01; see "Kinds of river" and criterion 10).
