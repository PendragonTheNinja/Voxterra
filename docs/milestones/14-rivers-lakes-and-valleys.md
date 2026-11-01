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

**Status: draft, for the owner's review** — the open questions are at the
end. Nothing is built until they are answered and the biome additions are
approved.

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
10. **The owner's walk**: on foot, following a river from a mountain spring
    to the sea; retro with maps before and after.

## Tasks (proposed order)

0. Catalogue additions approved; instruments (maps of drainage and water).
1. Stage-1 drainage: depression filling, flow routing, discharge, lakes and
   closed basins (ADR). Network tests (criteria 3, 4).
2. Valley erosion on the stage-1 grid, fed back as the elevation field's
   erosion stage (criterion 6).
3. Stage-2 channels: polylines, meanders, banks, depth, water levels; the
   fill rule above sea level (ADR); materials.
4. Lakes, salt flats, floodplains, wetlands, deltas, waterfalls; the new
   biomes.
5. Distant water in the LOD (criterion 8).
6. The owner's walk, `--bench`, retro.

## Non-goals

- **Flowing water** — water you can pour, dams that break, floods. That is a
  simulation (`vox-sim`, much later, `docs/vision.md` §1.3). Worldgen water
  is settled until disturbed.
- **Currents that push the player** — unless the owner pulls them in (open
  question 4); the network will carry a flow direction and speed per
  channel either way, so they can be added without regenerating.
- **Climate-shaped landforms** — dune seas, mesas and buttes, canyonlands,
  glacial U-valleys, fjords, cirques: proposed as **M15** (open question 1).
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

## Open questions for the owner

1. **Split the milestone?** Recommended: M14 is water and the valleys it
   carves (this spec); **M15 is landforms by climate** — dune seas, mesas and
   canyons, glacial valleys and fjords — built on M14's network. One huge
   milestone instead is possible but long.
2. **The biome additions** above (river, lakes, salt flats, floodplain,
   marsh, swamp, bog, delta, mangrove coast) and the blocks they need (silt,
   peat, salt, river gravel). Add, drop, rename?
3. **World-creation time.** A drainage network over 204 km at 64–128-block
   cells may take a few seconds to build. Rebuild on every load, or cache it
   in the world folder (derived and versioned, rebuilt if missing)?
4. **River currents** that push swimmers and (later) boats: in this
   milestone, or later with boats?
