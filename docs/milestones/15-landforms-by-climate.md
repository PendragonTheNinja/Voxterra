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
half the mesas with a way up). **The catalogue was approved by the owner
on 2026-11-10** (criterion 1's first half is met; the ADR is task 1's).
**The baseline is taken (2026-11-11):** `docs/bench/bench-1794373242.md`,
generator version 8 on the audited build (sprint 786–866 fps, relight
76–95 ms/s, backlog draining to 0) — criterion 9 compares against it, and
task 0 is complete. What was asked, for the record: the owner's re-bench of M14 at `GENERATOR_VERSION` 8 (`cargo run -p vox-app --
--bench`; it runs in its own world now, so no need to move `world/`
aside; copy the report from `bench/` to `docs/bench/` and commit it there —
`bench/` is git-ignored). **Take it on a build that includes the 2026-11
audit's engine changes** (generation, relight and threading all got
cheaper there, `docs/notes/code-audit-2026-11.md`): criterion 9 compares
M15's terrain against M14's on the same engine, and a baseline from before
the audit would hide a real M15 cost behind the audit's gains. The
process-weight map layer
needs task 1's weights and lands with them. **Task 1 is done (2026-11-10):** the
process weights and the ice-age climate are a stage-1 map
(`vox_worldgen::process`, `Generator::process`), with its tests, the
survey (`process_survey`) and the map layer (`process_map`,
`docs/gallery/m15/`); ADR-0023 is proposed and fixes the pipeline order.
Nothing in generation reads the map yet, so the generator is still
version 8. **Next: task 2**, dry lands — and its first decision is what
the drainage network samples once the ground has dunes and benches on it
(ADR-0023, *Consequences*).

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
