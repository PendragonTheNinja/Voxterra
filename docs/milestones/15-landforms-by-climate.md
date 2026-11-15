# Milestone 15 — Landforms by Climate

**Goal:** biomes that differ in the *shape* of the land, not only its colour
(the owner, 2026-11-01). Each climate shapes the ground with its own
processes, as Earth's do: wind piles sand into dune seas in the deserts;
flash floods cut mesas, buttes and canyons into dry uplands; ice carved
U-shaped valleys, cirques, horns and fjords where it lay; deep weathering
leaves domes standing out of savanna plains; rain rounds humid hills and
dissects them with valleys. At our scale (ADR-0010): processes real,
dimensions chosen for walking.

Depends on M13 (climate, winds, biomes) and M14 (the drainage network, the
erosion stage, and its lesson that water must fit the ground). Written
2026-11-03 from `docs/vision.md` §9 and §10.1 and M14's decisions, at the
owner's request.

**Status: spec approved (2026-11-03)** — the owner's decisions at the end.
Task 0 starts with the owner's re-bench of M14 at `GENERATOR_VERSION` 8.
**Task 0 progress (2026-11-04):** the landform catalogue is drafted in
`docs/landforms.md`; the owner answered its five questions (its Decisions
section: linear dunes from a seasonal dune-wind swing, slot canyons left
for geology, the dune wind tilted, tors on temperate uplands too, about
half the mesas with a way up). **Baseline benched (owner, 2026-11-04,
version 8):** `docs/bench/2026-11-04-m14-v8.md`; see *Baseline* below.
**Re-benched on the audited build (2026-11-11):**
`docs/bench/bench-1794373242.md`, version 8 (sprint 786–866 fps, relight
76–95 ms/s, backlog draining to 0). Criterion 9 compares against this
one: the 2026-11 audit made generation, relight and threading cheaper,
and a baseline from before it would hide a real M15 cost behind the
audit's gains. **Catalogue
approved by the owner (2026-11-04)** — task 0 is done.
**Task 1 progress (2026-11-04):** process weights and the ice-age climate
built (`vox_worldgen::landform`, `Generator::processes`), the world map's
`processes-*` and `ice-age-*` layers (`process_map`; gallery
`docs/gallery/m15/`), ADR-0021 proposed (renumbered ADR-0026 at the merge: main had taken 0021). No terrain changed
(`GENERATOR_VERSION` stays 8). Shares of land at weight ≥ 0.5, default
seed: wind 8%, flash flood 2.5%, ice 41%, weathering 9%, rain 36%; over
four seeds ice 23–41%. Two changes from the approved catalogue, for the
owner to confirm: ice needs snow as well as cold (temperature alone
glaciated 54% of land), and "mountains" means real ranges, not any uplift
(see the catalogue's *Task 1's fit*). The owner approved
the maps and both refinements (2026-11-04: "Seems good, move ahead!") —
task 1 is done.
**Task 2 progress (2026-11-05):** **dune seas built (2a,
`GENERATOR_VERSION` 9)** — `landform::dunes`: transverse, linear, star and
barchan dunes chosen by the dune wind (tilted 22.5°, swinging seasonally
toward the deserts' poleward edges), as waves with whole cycles around the
torus so they tile with no seam; laid on dry ground after the water (they
only raise ground, so water still fits it) and faded out within 4–40 blocks
of a river's bank; sand blocks; the readout names them ("hot desert, star
dunes"). Measured (`dune_survey`, four seeds): dunes 3+ blocks high on
20–25% of hot desert and 12–16% of cold; ridge columns' sand p50 6, p90
16–17, max 30–35; under 0.5% of steps between dune columns over one block.
Cost: `ground()` +2% across the world, +0.65 µs a column inside a dune sea.
Tests: `dunes_lie_where_the_wind_works_and_never_on_water`,
`transverse_crests_run_across_the_wind`, waves tile. **Benches, mesas
and buttes built (2b, `GENERATOR_VERSION` 10)** — `landform::mesas`,
applied before the network (`Generator::landformed`, `DrainageMap::build_on`):
caprock raised 30–120 blocks in islands over the stripped ground, sheer
walls with talus aprons (~34°), about half with a walkable ramp; benches
18–60 high (whole 3-block beds) with risers 5 blocks wide on any slope.
Raising the caprock, not digging the floors: the first draft dug floors
and added 2–6 points of land draining to closed basins; raised, the share
is unchanged (31.9 / 39.7 / 27.4% against 32.1 / 39.6 / 27.4% over three
seeds). The layer touches ~9% of land (tops ~1%). The reshaped networks
found two M14 lake-shore seams, fixed with failing tests first: a channel
on ground level with a lake runs on into it; a column hard against a
lake's region always gets its bank. Cost: `ground()` ~+10% across the
world (noisy), ~2.7× inside mesa country. Test:
`mesas_stand_only_on_flash_flood_uplands`; `mesa_survey`. **Canyons
built (2c, `GENERATOR_VERSION` 11)** — in the erosion stage: on
flash-flood ground (the weight passed in with the ground,
`DrainageMap::build_on`) incision is up to 4× and valley sides may stand
at 3:1 instead of 0.6. Measured (`mesa_survey`, rivers of 0.1+ m³/s):
40–95 blocks deep on flash-flood ground against 8–9 (p50) elsewhere — but
on only 1–4 sampled cells a world, because few rivers cross flash-flood
uplands (they are dry and mostly drain inward; no world has an "exotic"
river like the Colorado fed from wet mountains). Closed basins unchanged.
Tests: `caprock_cuts_canyons`, `caprock_holds_walls_steep`. **Task 2 is
built**; it waits on the owner's flight over the dry lands (the owner
chose to test everything together after, 2026-11-05). Carried: walls
banded only in their top 12 blocks (deep strata are geology's — the
12-block profile keeps the stone-chunk fast path); canyons rare, for
lack of rivers crossing the plateaus; `ground()` ~+10% (re-check with the
owner's `--bench`). **Next: task 3** — ice.
**Task 3 progress (2026-11-05):** **glacial troughs built (3a,
`GENERATOR_VERSION` 12, drainage cache format 5)** — in the erosion
stage, on the drainage grid: ice flux is the glaciated catchment routed
down the valleys the rivers cut; a trough's depth grows with √flux (to
400, at most 0.6 of the valley's relief), its half-width with flux^¼ (to
200); stored as lines along the network's own jittered nodes (so rivers
run on the floors) and evaluated at full resolution as a U — parabolic
floor, 3:1 walls — the ground the lower of its own and the U's. Only
where a valley has 100–250+ blocks of relief to channel the ice; troughs
taper in from nothing at a glacier's head. Three rejected first tries,
kept in the gallery's notes: a per-cell carved grid (128-block squares
showed through), a cut-off at the rim (sheer 300-block steps), and
troughs on low ground (a lattice of grooves and bowls on every gentle
glaciated slope). Measured: troughs up to ~560 blocks below the original
valley in the highest glaciated ranges, floors ~200 wide; lakes and
closed basins unchanged; world creation +1–2 s (network build 5.1 →
6.8 s, noisy); `ground()` unchanged across the world, ~+35% inside trough
country. Test: `ice_carves_a_valley_into_a_u` (with the existing water
tests all passing). **Fjords rare:** few glaciated coasts in these worlds
have mountains to channel ice, so troughs seldom reach the sea (the
deepest glaciated cut at a coast, default seed: 26 blocks). **Found in
M14's code and fixed at the owner's request (version 13):** in steep
mountains the river erosion stage's 256-block grid showed through as
squares (bilinear creases; now Catmull–Rom, flanks ~10% steeper at most),
and mountain rivers had sawtooth wedges along their banks (every steep
stretch held its water level to its end; now only a reach's highest step
is a waterfall and the others cascade). Gallery `ice-trough-…-v11-before`
and `-v13`. **Fjords:** kept rare (recommended, not overruled by the
owner, 2026-11-05). **Cirques (3b,
version 14):** a glacier's head is a bowl ~240 across, flat-floored and
overdeepened below its lip, its headwall 3:1 (1 773 on the default seed);
tarns where the network's 128-block cells see the bowl. **End moraines
and drumlins (3c, version 15):** `landform::deposits`, before the
network, raise-only, on low ground: moraine ridges 4–12 high along the
contour where the smooth (unjittered) ice-age weight is ½, broken by
meltwater gaps about half their length (0.8% of land); drumlin swarms
behind it (weights 0.55–0.95), ~260 long and ~110 apart along Z (2.1%).
Tests: cirques in `ice_carves_a_valley_into_a_u`,
`drumlins_lie_along_the_ice_flow`. **Arêtes and horns** are left to
emerge where cirques and troughs cut both sides of a divide — not built
or measured separately. **Closed basins are chaotic:** small changes
reroute whole basins, so seeds move both ways — over three seeds 33.0%
at version 11, 33.5% at version 15 (31.9 → 34.1, 39.7 → 38.4,
27.4 → 27.9); task 5 must aim at them directly. **Task 3 is built**,
waiting on the owner's flight with task 2. **Next: task 4** — inselbergs
and rain-dissected hills.
**Task 4 progress (2026-11-05):** **inselbergs and tors (4a, version
16)** — `landform::inselbergs`: sites on a jittered lattice tiled with
the world (domes every ~2.5 km, tors every ~500 blocks), kept by the
weathering weight at the site's centre so a dome is always whole; domes
40–150 high, 200–600 across, elongated with a wobbled outline, steep at
the foot (half rounder, climbable); tors stepped in 3-block courses, also
on rainy uplands at low weight; bare stone; before the network. 0.09% and
0.02% of land on the default seed (domes every few km on savanna
plains). **Rain-dissected hills (4b, version 17)** —
`landform::dissection`: ridges 10–40 high between small valleys left at
the ground's level, after the water and faded toward rivers (cutting
valleys before the network would hand it pits; after, lakes over air);
ridge crests on warped two-octave noise; closer with rain (test). 16.5%
of land. Half-orange hills are not a separate shape: the rounded ridges
are them at full rain. Cost: the ridge layer ~+10% on `ground()`
(noisy); the inselberg layer, after its cheap tests, none measurable.
Tests: `domes_are_whole_steep_and_tiled`,
`inselbergs_stand_on_weathered_plains`, `ridges_rise_from_untouched_valleys`,
`valleys_closer_with_more_rain`. **Task 4 is built**, waiting on the
owner's flight. **Next: task 5** — M14's carried items: closed basins
(~33%, target ≤ ~22%) and valleys of 1+ m³/s rivers ≥ ~20 deep.
**Merged onto the tectonic terrain (2026-11-14, `GENERATOR_VERSION` 18).**
Tasks 1–4 were built in a session whose branch was never merged; in the
meantime, in the owner's order, milestone 16's tasks 1–2 replaced the
terrain under them (versions 9–10 on that line) and a second task 1
(`vox_worldgen::process`, ADR-0023) was built in parallel. The two lines
were merged as version 18: this line's landform module replaces the
`process` module (the same weights, built further), and every landform
now stands on the plate framework's continents and ranges. What the
merge changed, each with a failing test first or a measured reason:
ponds past a river reach's end are filled, not held, where a lower
reach's banks — or its water below a waterfall — come within three
blocks (`lake_shores_hold_their_water`: the cascading reaches of
version 13 met the tectonic ranges' steeper valleys); the dune survey's
floor is 500 columns, the drumlin test samples a wider square (the
tectonic continents put fewer of either in the old windows); and the
erosion stage's steepest added slope is 1.39 (bound 1.45: the ranges'
spines and the Catmull–Rom cut together). The images in the gallery
up to `hills-…-v17` show this line's own terrain, not the merged one.
**Re-checked on the tectonic terrain (2026-11-14, `GENERATOR_VERSION`
19).** Every survey rerun on version 18 (gallery rows `…-v18`). Better
than on the old terrain: closed basins **19.0 / 23.7 / 18.1%** over three
seeds (33% before — task 5's target of ≤ ~22% is all but met already;
re-measure there), fjords cut 184–258 deep (26 before). Fixed, each with
a failing test first: end moraines ran dead straight over flat lowland
(the ice weight follows latitude there) — now bent into lobes
(`moraines_bend_in_lobes`); drumlins stood on a grid of identical hills —
now staggered, sized and thinned (`drumlins_are_not_a_grid`); 13% of
cirques were craters walled all round on gentler slopes — now cut no
deeper than the slope below falls across the bowl, plus a 6-block tarn
lip (`cirques_open_downhill`; 1 402 cirques of 8+ blocks, 1 952 before).
Worse, carried: **no canyons** — no river of 0.1+ m³/s crosses
flash-flood ground in three seeds (the dry uplands drain inward, as
before, and the tectonic continents put them farther from wet
mountains); dune ground slightly rougher (0.4–1.6% of steps over a
block, under 0.5% before). Moraines 0.21% of land, drumlins 1.4%,
dissected hills 22.8%, inselbergs 0.08%. Every landform waits on the
owner's flight of version 19.
**Task 5 progress (2026-11-14, `GENERATOR_VERSION` 20).** M14's carried
items, measured in the chunks over the survey's seeds:
- **Closed basins: met.** 19.3% of land over six seeds (13.1–24.4%;
  `drainage_survey`), against ≤ ~22% (33% on the old terrain, 29% at
  M14's close). The tectonic terrain did most of it; the inner valleys
  below took it from 19.9%.
- **Valleys for 1+ m³/s rivers: met.** Rivers' **inner valleys**
  (ADR-0026, *Rivers' inner valleys*): every river of 0.1+ m³/s lowers the
  ground along its own line, 8 blocks per decade of discharge (at most
  30), fading out over its sides, its floor falling all the way to its
  mouth. Valley depth (the ground 384 blocks off less the river's water,
  M14's measure; `valley_survey`): rivers of 1–10 m³/s **30–32** at the
  median (p25 23–25), 10+ m³/s ~32; 18–19 at version 19, 11–13 at M14's
  close. Streams of 0.01–0.1 m³/s unchanged (~18–20). Test:
  `large_rivers_run_in_valleys` (held at 25; version 19 measured 21).
  A first draft clipped the ground to a U about straight lines and sliced
  ridged hillsides into flat facets (gallery, `…-clipped-draft`).
  Found on the way and fixed with the failing test first: a pond a block
  short of its reach's end, beside a lower river whose bank had cut solid
  ground to the pond's level (`lake_shores_hold_their_water`) — every
  pond now holds only where no bank within three blocks is cut below it.
  Lakes on ice-scoured ground fell from 13–19% to 9–13% of cells (the
  valleys cut some rims); still far above other ground's ~1.2%. World
  creation +~1 s (network 6.9–7.0 s → 7.7–8.7 s, two seeds); chunk
  generation unchanged within noise. Drainage cache format 6.
- **Small streams that follow the ground: built (version 21).** Each
  stream's path bends across its straight line to the low ground (stage
  1: four points chosen together, up to 32 blocks across, a smooth curve
  in stage 2); streams on slopes cut their own gullies (3–9 blocks,
  fading over four depths); the grid-scale wander shrinks on steep
  reaches. Channels on hillsides run on ground a median **3 blocks below
  their straight lines** (`stream_survey`, three seeds, p25 1–2; version
  20: 0) — test `small_streams_take_the_low_ground`. A first draft as a
  polyline through seven points zigzagged (gallery, `…-zigzag-draft`).
  What did not move: the share of hillside stream points with a slot's
  wall (4+ blocks above the water two blocks off) stays ~4–7%, as at
  version 20 — the banks' 45° cut already decides the ground nearest a
  channel — and many small streams still run *across* hillsides, held
  by their downhill bank, because the network's eight directions between
  cells 128 blocks apart rarely match the fall line; bending within
  ±32 blocks cannot turn a reach. That is the owner's to judge on
  foot. Found on the way, each with the failing test first: a fill
  raising ground below sea level out of the water was still treated as
  under it, leaving a river beside it unbanked; rivers stood a block or
  two above the sea or a lake at the edge of its region, where the shore
  is left unbuilt (`water_is_never_held_by_air`,
  `lake_shores_hold_their_water`). Gullies on flat ground had lowered
  banks into wetland; they act on slopes only. Cost: chunk generation
  +~5% (1.10 → 1.15 ms a chunk column), LOD nodes +5–7%, world creation
  +~1 s. Drainage cache format 7.
- **Task 5 is built.**

**Task 6 progress (2026-11-15, `GENERATOR_VERSION` 22).**
- **Canyons.** Version 21 had almost none: 0.3% of flash-flood drainage
  nodes stood in a canyon 30+ deep (the ground 48 blocks round 30+ above
  them). Washes, not rivers, now cut them: on caprock every wash of
  0.001+ m³/s gets an inner valley 90% of the way down to the floor it
  drains to, near-sheer walls (ADR-0026, *Canyons are washes' inner
  valleys*): 2 192 canyon lines on the default world, median 15 deep, p90
  53; **1.1% of flash-flood nodes in canyons** (test
  `canyons_cut_the_dry_plateaus`, held at 0.8%). Inner valleys wander
  ±20 blocks by a tiled value noise (straight, they were ruled cuts; the
  first canyons, gallery `…-straight-draft`). **Carried:** these are
  arroyos and canyons into the floors between mesas; a plateau's deep
  canyons want rivers from wetter ground crossing it, which these worlds'
  dry uplands — mesa islands on stripped floors, draining inward —
  seldom have.
- **Bare rock on cliffs.** A dry column 3+ blocks over a neighbour in its
  chunk wears stone (sandstone in hot desert; badlands keep their bands,
  dune sand stays sand); the LOD applies the same rule on its own grid at
  3×stride, and agrees with full resolution at stride 1. Around a trough,
  a cirque and a canyon, 29% of cliff columns wore soil at version 21;
  under 10% now (test `cliffs_are_bare_rock`). Domes and tors were rock
  already, dunes sand. The LOD/full-resolution test now samples on the
  chunk grid, as every LOD node is.
- Cost: chunk generation and world creation unchanged within noise
  (measured interleaved against version 21 on the same machine).
- **The owner's first look at version 22 (2026-11-15)**, eight VIEW
  lines re-rendered headlessly (gallery `owner-v22-…`). Fixed as version
  23, each with a failing test first: cliff rock judged against one
  neighbour put rows of stone posts on terrace edges and shore steps —
  now a steady slope of 1.5 a block across ±2 columns
  (`a_terrace_step_is_not_a_cliff`); mesa and butte walls were banded
  for 12 blocks and plain stone below, a grey fluted wall — dry-land
  cliffs are now banded down their whole face, dry by Köppen's aridity,
  since a mesa's cooler top is often grassland
  (`mesa_walls_are_banded_to_the_floor`: 107 of 107 plain at version 22).
  Seen and not yet fixed: buttes are extruded cylinders with no benches
  or scree ("straight up, no real shape"); inselbergs are smooth grey
  blobs on the plain; thin gaps in the LOD's mesa walls; the LOD under
  sea ice; ice blocks all light. The world emptying while flying was
  speed: 400–700 blocks a second, 3–6× sprint-flight, outruns generation.
- Still to do in task 6: renders of every landform (the gallery has one
  per landform at versions 18–22), the owner's `--bench` and walk, the
  retrospective.

## Why this milestone exists

After M13 and M14 the world has Earth's climates, biomes and rivers, but one
kind of ground: the same elevation field everywhere, coloured by biome and
cut by rivers. A desert is sand-coloured hills, a glacier's valley is a
river's V, a savanna is a green-yellow plain. On Earth you can tell where you
are from the shape of the horizon alone — dune crests, a mesa's flat top, a
U-valley's walls, Uluru. That is what this milestone adds.

It also takes over two things M14 carried: closed basins cover ~29% of land
(Earth ~18%), because the base terrain's continental interiors are enclosed,
and large rivers sit in valleys only 11–13 blocks deep.

## Earth, process by process — what "right" looks like

Earth's dimensions, then ours, chosen by walking time (ADR-0010: detail
underfoot ~800 blocks, terrain character ~2 600, a range ~7 000; peaks to
~1 150, lowlands ~70; sprint 5.6 m/s).

### Wind: dune seas (hot and cold deserts)

- **Earth:** sand seas (ergs) cover about a fifth of the world's deserts —
  the Sahara's, Arabia's Empty Quarter, the Namib, the Taklamakan. Dunes are
  shaped by the wind: transverse ridges and crescent barchans across a
  steady wind, long linear ridges along a two-way wind, star dunes where it
  turns. Typically 10–50 m high and 0.5–2 km apart; star dunes to 300 m.
  Ergs lie in basins downwind of their sand, rarely on mountains.
- **Ours:** dune seas in desert basins, ridges 6–30 blocks high and 100–250
  apart (half a minute from crest to crest), oriented by M13's wind field —
  transverse where it is steady, star dunes in the few places it is not;
  barchans scattered at the edges. A dune sea a few kilometres across: ten
  minutes of climbing.

### Flash floods on dry uplands: mesas, buttes, canyons (arid and badlands)

- **Earth:** the Colorado Plateau — flat-topped mesas and narrow buttes left
  where a hard caprock resists, stepped slopes of alternating hard and soft
  beds, canyons where rivers cut down through the plateau (the Grand Canyon
  1.8 km deep), talus aprons below every cliff.
- **Ours:** dry uplands terraced into benches 20–60 blocks high with steep
  risers and talus at their feet; mesas and buttes isolated where the
  network's valleys have cut back; canyons where M14's rivers cross a
  plateau, 50–200 blocks deep. Badlands strata (M13) already mark the beds.
  Cliffs are as steep as the rock makes them; some mesas have a talus ramp
  or a broken bench up and some have none, as on Earth.

### Ice: glacial valleys and coasts (where ice lay, now and in the ice age)

- **Earth:** valleys glaciers widened from a river's V to a U (Yosemite,
  Lauterbrunnen), cirques at their heads holding tarns, sharp arêtes and
  horns between cirques (the Matterhorn), side valleys left hanging above
  the main one with waterfalls; fjords where U-valleys reach the sea
  (Norway, Patagonia, New Zealand, British Columbia); moraines and drumlins
  and countless lakes on the lowlands ice covered. Most of it was carved in
  the last ice age, ~20 000 years ago, when the world was ~6 °C colder and
  ice sheets reached ~40–50° of latitude.
- **Ours:** in mountains and high latitudes that were glaciated, valley
  floors 100–400 blocks wide between walls 100–400 high, cirques with tarns
  at their heads, hanging tributaries with waterfalls (M14 already draws
  them at steps), fjords where these valleys meet the coast, and moraine
  ridges and lake districts on glaciated lowlands (M14's ice-scoured lakes
  are the start).

### Deep weathering: inselbergs (savanna and dry tropics)

- **Earth:** domes and tors of bare rock rising abruptly from flat plains
  where rock weathered deep for millions of years and the plain was stripped
  away — Uluru (348 m), the Sugarloaf, the bornhardts of Africa and Brazil.
- **Ours:** scattered domes 40–150 blocks high and 200–600 across, bare rock,
  standing out of savanna and dry-forest plains — landmarks seen for
  kilometres.

### Rain: rounded, dissected hills (humid lands)

- **Earth:** where rain is plentiful, soil creep rounds hilltops and many
  small streams dissect the land into close-set valleys — Appalachian ridges,
  Brazil's "half-orange" hills, Borneo's dissected uplands. Valleys are
  dense, hillsides convex above and concave below.
- **Ours:** humid hills rounded on top and cut by small valleys finer than
  the network's 128-block cells, their density rising with rainfall.

## How it works

### Landforms are height layers, blended as heights

From `docs/vision.md` §10.1: each landform reshapes the ground as a height
layer, weighted by where its process acts; layers blend as heights, never as
densities. Every layer is a pure function of `(x, z)` and stage-1 maps, so any
chunk still generates alone and the LOD samples the same ground.

### Process weights in stage 1

A stage-1 map gives each place a weight for each process, smooth and noisy
(M14's lesson: anything read from a coarse grid shows through as squares):

- **wind** — desert climate, low relief, downwind of sand;
- **flash flood** — arid or badlands upland, a plateau's height;
- **ice** — glaciated now (ice sheet, alpine) or in an **ice-age climate**:
  M13's temperature less ~6 °C (more toward the poles), so ice reaches
  lower and further than today's;
- **weathering** — tropical savanna and dry forest on old, low plains;
- **rain** — humid climates.

### Water must still fit the ground (M14)

Every landform changes the ground rivers run on, and M14 found that water
must fit ground the network can see. So the order is:

1. base elevation → **landform layers** (dunes, terraces and mesas,
   inselbergs, dissection);
2. → the drainage network and erosion stage, built on that ground (rivers cut
   canyons into the mesas, flow round the inselbergs, die in the dune seas);
3. → **glacial reshaping inside the erosion stage** (ice widens and deepens
   the network's valleys into U-shapes, cuts cirques at their heads,
   overdeepens basins that hold tarns and fjords);
4. → M14's rivers, lakes and wetlands, unchanged.

The ADR records this order and its cost. All of M14's water tests must still
pass.

## Acceptance criteria

1. **Catalogue and ADR first**: a landform catalogue (each landform's Earth
   and in-game dimensions and where it forms) approved before code; an ADR
   for landforms as height layers and their order with the network.
2. **Instruments first**: a process-weight layer on the world map; the
   readout names the landform the player is on; headless renders of each.
3. **Each landform where Earth puts it**, measured over many seeds: dune
   seas only in deserts and oriented across the wind; mesas and canyons in
   dry uplands; U-valleys, cirques and fjords only where ice lay; inselbergs
   in savanna; dissection rising with rainfall. As tests.
4. **Recognisable at walking speed**: each landform reads as itself on foot
   and from a distance (owner's walk). Cliffs may be unclimbable, as Earth's
   are; traversal of rough terrain is a gameplay system for later (owner,
   2026-11-03).
5. **Water still fits the ground**: every M14 water test passes (never held
   by air, shores hold, rivers in valleys not slots, rivers end at still
   water); rivers cut canyons, avoid dune crests, fall from hanging valleys.
6. **M14's carried items**: closed basins nearer Earth's (≤ ~22% of land
   over the survey's seeds); rivers of 1+ m³/s in valleys at least ~20 blocks
   deep in the chunks.
7. **Every world, every size**: every landform in every legal world size
   whose climates allow it (as M13 did for biomes).
8. **Near and far agree**: the LOD shows each landform; no seam.
9. **Sound and fast**: determinism and per-chunk independence;
   `GENERATOR_VERSION` bumped and the fingerprint re-pinned; world creation
   within its budget; `--bench` within ~10% of M14's version-8 re-bench.
10. **The owner's walk** (or flights, as M14), and a retrospective with
    before-and-after maps and renders.

## Baseline (criterion 9)

The owner's `--bench` at `GENERATOR_VERSION` 8, 2026-11-04, RTX 4060,
800×600, dev build, flowing water on — `docs/bench/2026-11-04-m14-v8.md`.
Against M14's version-4 bench (after the span-cache fix) and M13's close:

| Phase | v8 (this) | M14 v4 | M13 close |
|---|---|---|---|
| 6 levels, land, standing | 834 | 850 | 894 |
| 6 levels, ocean, standing | 737 | 727 | 811 |
| 8 levels, land, sprint | 740 | 684 | 754 |
| 8 levels, ocean, sprint | 731 | 664 | 745 |
| 8 levels, rivers, standing / sprint | 695 / 741 | 716 / 709 | — |
| 8 levels, waterfall, standing | 617 | 598 | — |

Versions 5–8 (stage-1 long profiles, per-column region checks) cost
nothing measurable: every phase is within this machine's ~14% run-to-run
noise, and sprint is back at M13's level. Streaming unchanged: relight
177–212 ms/s (M13 181–190), mesh 195–205 µs per chunk, encode ~0.7 ms at
8 levels. Two things to watch, not act on yet: the sprint backlog at the
end is 177–354 (M13 190–232; the 354 is 8-level ocean, ~200 of it
waiting on relight), and the rivers standing phase had one 22 ms frame
with nothing streaming (1% low 389, so a single hitch). Criterion 9
compares against the v8 column. The bench header now names the
generator version and build profile (M14 lesson 4); this run predates
that, and its version is inferred from the build.

## Tasks (proposed order)

0. **Baseline and catalogue.** The owner re-benches M14 at version 8 (the
   baseline criterion 9 compares against). The landform catalogue for
   approval. The process-weight layer on the world map.
1. **Process weights in stage 1**, with the ice-age climate; ADR proposed.
2. **Dry lands:** dune seas oriented by the wind; mesas, buttes and canyons
   by terracing dry uplands and letting the network cut them; talus aprons.
3. **Ice:** glacial erosion in the erosion stage — U-valleys, cirques and
   tarns, overdeepened fjords, hanging tributaries; moraines and lake
   districts on glaciated lowlands.
4. **Tropics and humid lands:** inselbergs; dissected, rounded humid hills.
5. **M14's carried items:** the enclosed interiors (closed basins); deeper
   valleys for large rivers.
   **Small streams that follow the ground** (owner, 2026-11-11, after
   his first flight on generator version 9: "so many weird funky things
   with rivers ... worse in mountains"; agreed to fold in here). Streams
   are straight segments between drainage cells 128 blocks apart, so a
   small torrent on a steep hillside is a straight one-block slot down
   the slope with falls stacked on it
   (`docs/gallery/m16/flight1-3-river-mountain-a.jpg`, `-4-...-b.jpg`).
   Small streams should bend round spurs and cut V-shaped gullies at
   block scale. It is hillside work, as this milestone's landforms are,
   so it is tuned once, with them.
   **Proposed (2026-11-11), the owner to decide: coasts as a sixth
   landform family.** The owner's first flight on generator version 9
   found shores "obviously built from large sweeping shapes": one smooth
   slope from land to sea, its sand and rock in bands parallel to the
   water (`docs/gallery/m16/flight1-1-sand-curve.jpg`). Milestone 16's
   polish could only alternate the shore's material
   (`docs/milestones/16-tectonics-and-bedrock.md`, task 2b); the shape
   needs Earth's coastal landforms — sea cliffs and wave-cut platforms on
   rough coasts, coves and bayhead beaches between headlands, spits and
   barrier islands on low sandy ones, stacks off retreating cliffs —
   weighted, like the other five families, by what makes them (wave
   energy, rock, sediment supply). It would go into `docs/landforms.md`
   for approval first.
6. **Materials, the LOD, the bench, the owner's walk, the retro** — bare
   rock on cliffs and domes, sand on dunes; renders of every landform.

## Non-goals

- **Geology** — rock types, real strata, caves, ores, and the landforms that
  need them: karst (limestone towers, sinkholes), volcanoes. The next
  milestone in `docs/vision.md` §9.
- **Overhangs and arches** — they need a 3D density pass and an ADR for the
  heightfield LOD first (`docs/vision.md` §10.1).
- **Coasts by rock** (cliffs against beaches) — with geology.
- **Moving sand, seasons, vegetation** — later milestones.
- **Natural slopes and partial blocks** (`docs/vision.md` §1.4) — they would
  make dunes and talus walkable without steps, but they are their own
  milestone.

## Decisions (owner, 2026-11-03)

1. **Scope: all five families** — wind, flash flood, ice, weathering, rain.
2. **The ice age**: glacial landforms wherever ice lay in the last ice age
   (the most realistic), so fjords and U-valleys reach coasts that are
   temperate today, as Norway's and Scotland's do.
3. **Cliffs**: real cliffs, and some need not be walkable at all — "not all
   are walkable in real life"; rough-terrain traversal is a later gameplay
   system. ADR-0010's walkable slopes no longer bind landform cliffs.
4. **Dune size as proposed**: ridges 6–30 blocks high, 100–250 apart —
   crossing a dune sea is a climb.
5. **Order: dry lands first** (task 2), then ice (task 3).
6. **The catalogue's five questions** (2026-11-04) are answered in
   `docs/landforms.md`, *Decisions*.
