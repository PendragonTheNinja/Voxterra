# Landform catalogue

*Draft 2026-11-03; the owner answered its five questions on 2026-11-04
(Decisions, at the end — the text below already follows them).
**Approved by the owner, 2026-11-04** ("Yes that is approved!"). Written for the owner's approval
before M15 builds anything
(M15 criterion 1: "a landform catalogue … approved before code"). Once
approved, this is what the landform layers must produce, what the
where-Earth-puts-it tests check (criterion 3), and what the every-world test
looks for (criterion 7). It does for the shape of the ground what
`docs/biomes.md` did for its colour.*

## Rules

- **Every landform is a real place.** Each has Earth analogues and is made by
  a named process (wind, flash flood, ice, deep weathering, rain). No
  invented landforms; no landform placed without its process.
- **Landforms are made by processes, never scattered.** A column's landform
  follows from the stage-1 process weights (climate, ice-age climate, height,
  relief, the drainage network). A dune sea is where wind and sand are, not
  where a noise field says "dunes here".
- **Height layers, blended as heights** (`docs/vision.md` §10.1). Each
  landform is a pure function of `(x, z)` and the stage-1 maps, so any chunk
  still generates alone, and the LOD samples the same ground near and far.
- **Dimensions by walking time** (ADR-0010). Earth's proportions within a
  landform, our sizes chosen for a player sprinting at 5.6 m/s. Every walking
  time below is ground sprint.
- **Cliffs may be cliffs** (owner, 2026-11-03). A landform's walls are as
  steep as its rock makes them; some have a way up and some do not. The
  player's jump clears 1.25 blocks, so any slope stepping at most one block
  per block (45°) is walkable today; steeper is a climb for a later system.
- **Water fits the ground** (M14). Landforms before the drainage network,
  except ice, which reshapes the network's own valleys inside the erosion
  stage (M15 spec, *Water must still fit the ground*).

## The process weights (stage 1)

Each place gets a weight in `[0, 1]` for each process, smooth and noisy at
the edges — never read straight off a coarse grid (M14's lesson: a grid
shows through as squares). The weights are the instrument the world map
shows (task 0/1) and what every test below conditions on.

"Aridity" below is precipitation over Köppen's threshold `20·T + 140` mm,
the same index M13's biomes use: under 0.5 is desert, 0.5–1 steppe, above 1
humid.

| Process | Weight rises with | Zero where |
|---|---|---|
| **Wind** | aridity under ~0.4; low relief; a basin (lower than the ground upwind); sand supply — downwind of dry lowland and of desert rivers' beds | aridity above ~0.6; mountains; a plateau above ~150 blocks |
| **Flash flood** | aridity under ~0.8; an upland (ground above ~120 blocks) of moderate relief; strongest where badlands are | humid; mountains (orogeny a real share of height); lowland basins |
| **Ice** | ice-age ground temperature below 0 °C (see below) | ice-age ground temperature above ~+2 °C |
| **Weathering** | tropical (≥ 20 °C), aridity 0.6–2 (savanna, dry forest); low relief, old plains | mountains; deserts; rainforest's wettest; anywhere glaciated |
| **Rain** | aridity above 1, rising to full at ~3; any relief but mountains | aridity below ~0.8 |

Wind and flash flood share the dry lands and divide them by height: basins
take dunes, uplands take terraces. Weathering and rain share the tropics and
divide them by rainfall. Ice is independent of the other four — Patagonia
is a cold desert *and* glaciated — because it works on the network's valleys
after the others have shaped the ground.

### The ice-age climate

The owner chose glacial landforms wherever the last ice age's ice lay
(decision 2). Earth ~20 000 years ago was ~4 °C colder in the tropics and
10–15 °C colder at high latitudes; ice sheets reached ~40–50° of latitude
and mountain glaciers came ~1 000 m lower than today's.

Proposed: **ice-age temperature = today's − ΔT**, with ΔT = 4 °C up to 20°
of latitude, rising smoothly to 11 °C at 50° and beyond. Ground counts as
glaciated where its ice-age mean annual temperature is below **0 °C** —
glaciers flow below their snowline, so ice lay a little lower than the
ice-age snowline. With M13's latitude curve and lapse rate that gives:

| Latitude | Today's ice (rock and snow, < −5 °C) above | Ice-age ice above |
|---|---|---|
| 0° | 1 000 | ~720 |
| 20° | 880 | ~600 |
| 30° | 760 | ~410 |
| 40° | 620 | ~165 |
| 45° | 540 | ~50 |
| 50° and poleward | 440 → sea level | **sea level** |

So every range whose peaks pass ~700 blocks has glacial valleys even at the
equator (as the Andes, Kilimanjaro and New Guinea do), mid-latitude ranges
are glaciated well down their flanks, and from ~47° poleward the lowlands
were under an ice sheet: lake districts, moraines and fjords on coasts that
are temperate today, as Norway's and Scotland's are. Where ice still lies
(M13's ice sheet), the glacial ground is beneath it, as Greenland's is.

These are tuning targets; task 1 fits the curve and the threshold to them.

**Task 1's fit (2026-11-04, ADR-0021 §3; approved by the owner the same day).** The curve and threshold above
reproduce the table exactly (ΔT a smoothstep in `sin(latitude)` from 20° to
50°; a test pins it). Two things were added, because temperature alone put
54% of land under ice, with every cold interior glaciated: **snow** — the
threshold shifts with today's precipitation (−8 °C at 100 mm, 0 at the
table's reference 1 000 mm, +2.5 at 4 000: wet ranges glaciate lower, as
on Earth), and below 200–450 mm no ice sheet grows (Siberia stayed
ice-free); and **today's ice sheet** counts as glaciated however dry
(Antarctica). Result: 23–41% of land glaciated over four seeds (Earth's
last glacial maximum ~25–32%).

"Mountains" in the table's *Zero where* column means real ranges — uplift
200–400 blocks and more — not any uplift: almost all high ground here is
raised by mountain building, so the stricter reading left flash floods on
1.4% of land.

## Wind — dune seas

| Landform | Earth analogues | Earth size | Ours | Walking | Where |
|---|---|---|---|---|---|
| **Transverse dunes** (ridges across the wind) | Namib, the Sahara's ergs, Gran Desierto | 10–50 m high, 0.5–2 km apart | ridges 6–30 high, 100–250 apart, crests sinuous over ~500 | crest to crest 20–45 s | the core of a dune sea, where the wind is steady |
| **Barchans** (crescents, horns downwind) | Peru's Pampa de Jagüay, the Western Desert | 3–30 m high, 50–300 m across | 4–12 high, 40–120 across, scattered | — | a dune sea's edges and sand-starved ground (gravel plains between) |
| **Star dunes** (arms in several directions) | Grand Erg Oriental, Badain Jaran | 50–300 m, the tallest dunes | 30–60 high, 300–500 apart, few | a climb of ~1 min | where the wind is not steady: the boundaries between wind belts (~25–35° and ~55–65° latitude), and against mountains |
| **Dune sea (erg)** as a whole | Rub' al Khali, Taklamakan | ~20% of the world's hot deserts | 2–15 km across; ~20–30% of hot-desert land, ~10% of cold-desert land | 6–45 min to cross | desert basins downwind of their sand; rarely on uplands, never on mountains |

- **Shape.** A dune is asymmetric: a gentle windward slope (~10–15°, a long
  walk up) and a steep lee face at sand's angle of repose (~32–34°, still
  walkable at one block per 1.5). Crests are sharp; the lee faces downwind.
- **Orientation from the wind.** M13's winds blow along latitude rows
  (`climate.rs`: trades westward, westerlies eastward, blended at ~25–35° and
  ~55–65°). Transverse crests therefore run roughly north–south, lee faces
  west in the trades and east in the westerlies. Earth's trades blow from
  the north-east and south-east, not due east, so **the dune wind is
  tilted per belt** (trades from ~ENE/ESE, westerlies from ~WSW/WNW) and
  crests run diagonally as on Earth (decision 3). The tilt is read only by
  the dunes: M13's rain, temperatures and biomes are unchanged.
- **Interdunes** between ridges: flat sand or gravel corridors, the walking
  routes through a sea, as on Earth.
- **Water.** Rivers die in dune seas (M14's dry rivers end in closed
  basins); they run in the interdunes, never over a crest (criterion 5).
- **Ground.** Sand on dunes (red sand in the badlands' and hot deserts'
  redder regions is a later choice); interdunes keep the desert's ground.
- **Linear (seif) dunes** — Earth's most common dune, long ridges *along* a
  two-way wind (the Simpson, the Namib's south) — need two winds at an
  angle. M13 has no seasons, so the dune wind gets one (decision 1): a
  **seasonal swing** of its direction per latitude, read only by the dunes
  like the tilt. Small in the trades' core (transverse dunes), large toward
  the deserts' poleward edges, ~20–30° (linear dunes along the mean wind,
  as the Simpson's and the Kalahari's form from the seasonal shift of the
  subtropical high); where belts meet the wind turns right round (star
  dunes). Fryberger's rule: one wind → transverse and barchans, two at an
  obtuse angle → linear, many → star.

**Built (M15 task 2a, version 9).** As above, with: one dune type per
place (transverse and linear ridges added together made a lattice); the
erg needs warmth as well as wind (cold deserts keep ~45% of a hot one's),
so dunes stand on ~20–25% of hot desert and ~12–16% of cold; star dunes
on a wandering hexagonal lattice of crossing waves, many points empty,
heights 25–55; barchans from crests cut one spacing long by the wave
across the wind. Measured in `dune_survey`; gallery `docs/gallery/m15/`.

## Flash flood — benches, mesas, buttes, canyons

| Landform | Earth analogues | Earth size | Ours | Walking | Where |
|---|---|---|---|---|---|
| **Benches** (stepped plateau) | Colorado Plateau's Grand Staircase | cliffs 100s of m between broad benches | risers 20–60 high, near vertical; benches 100–600 wide | a bench in 20 s–2 min | dry uplands: every flash-flood-weighted plateau |
| **Mesa** | Monument Valley, the Hopi mesas | flat top km across, cliffs 100–300 m | flat top 200–1 000 across, walls 30–120 | top crossed in 0.5–3 min | where the network's valleys have cut back into a bench and left it isolated |
| **Butte** | Monument Valley's Mittens | narrower than tall | 40–200 across, as tall or taller than wide | — | the last stage of a mesa: small remnants between valleys |
| **Canyon** | Grand Canyon, Fish River, Canyonlands | to 1.8 km deep, rim to rim to 29 km | 50–200 deep, rim to rim 150–800, stepped walls, a gorge at the bottom | rim to river: a long climb or none | where an M14 river of any size crosses a flash-flood upland; depth grows with discharge and the plateau's height |
| **Talus apron** | every cliff in the desert West | slopes of fallen rock at ~34° | at the foot of every riser and mesa, up to half its height | walkable (1 block per 1.5) | below cliffs, from the caprock's rubble |

- **The caprock.** Benches form where a hard bed resists: a bench's top is
  its caprock, its riser cuts the soft beds below, and talus covers the
  riser's foot. M13's badlands strata already band the rock; the bench
  heights should land on band boundaries, so the colours read as beds.
- **A way up, or none.** Some mesas keep a talus ramp or a broken bench to
  their top and some have none (owner, spec decision 3). Some have a
  walkable route (decision 5): about half, chosen per mesa from the seed,
  the share a tuning knob.
- **Slot canyons** (Antelope Canyon) are left for the geology milestone
  (decision 2): they need soft and hard beds the rock does not have yet,
  and they are narrower than anything the network sees.
- **Rivers cut them.** Benches are applied before the network, so M14's
  rivers find the plateau and cut canyons through it; the canyon's walls
  step as the benches do. Dry washes (arroyos) are M14's dry rivers.

**Built (M15 task 2b, version 10).** Benches, mesas, buttes and talus as
above, before the network. Mesas are the caprock *raised* above the
stripped ground (which keeps the base height and its drainage), never the
floor dug: dug floors made bowls the network filled with closed basins.
Caprock covers at most about a third of an upland so it stands in islands.
A per-mesa "way up" is approximated by a 450-block noise that widens walls
into ramps — about half of all wall length, not a choice per mesa. Bench
levels are whole 3-block beds, but walls show the beds only in their top
12 blocks (the profile depth); below is stone until geology.

**Canyons (task 2c, version 11)** are cut in the erosion stage: harder
incision and walls up to 3:1 on flash-flood ground. 40–95 blocks deep
where a river crosses a plateau, but rare — dry uplands seldom carry a
through-flowing river.

## Ice — glacial valleys and coasts

All of these are made inside M14's erosion stage, on the network's own
valleys, wherever the ice-age weight is above zero. Today's climate then
decides what lies in them (ice sheet, tundra, forest, a lake).

| Landform | Earth analogues | Earth size | Ours | Walking | Where |
|---|---|---|---|---|---|
| **U-valley (glacial trough)** | Yosemite, Lauterbrunnen, Glen Coe | floors 0.5–2 km, walls 500–1 000 m | floors 100–400 wide, walls 100–400 high, steep to near vertical at their foot | floor crossed in 20 s–1.2 min | every glaciated valley in mountains and uplands; the width grows with the ice's catchment |
| **Cirque** with **tarn** | Snowdon's Glaslyn, the Rockies' cirques | bowls 0.5–2 km, headwalls 100s of m | bowls 200–500 across, headwalls 100–300, a tarn on the floor | — | the head of every glaciated valley, facing the pole (shaded slopes held ice longest) |
| **Arête** and **horn** | Matterhorn, Striding Edge | knife ridges, pyramid peaks | ridges a few blocks wide between cirques; horns where three or more cirques meet | arêtes walkable along the crest; horns a climb | the divides between cirques |
| **Hanging valley** with waterfall | Yosemite Falls, Bridalveil | tributaries 100s of m above the main floor | tributary floors 30–150 above the trough's floor | — | where a smaller glaciated valley meets a larger one; M14 draws the waterfall at the step |
| **Overdeepened basin** (ribbon lake) | Loch Ness, Lake Como | long lakes deeper than their outlet | 1–6 km long, 20–80 deep below the outlet's rock sill | — | the floors of large troughs; M14's lakes fill them |
| **Fjord** | Sognefjord, Milford Sound, Howe Sound | to 1 300 m deep, 200 km long | sea in a trough 30–150 below sea level, a shallow sill at the mouth, walls to 400 | — | where a glaciated trough reaches the sea |
| **Moraine** | the Lake District, Long Island, Wisconsin's kettle moraine | ridges 10–100 m, km long | ridges 5–20 high, 30–100 wide, in arcs | crossed in seconds | the ice-age ice's farthest edge (end moraines) and valley glaciers' snouts; sides of troughs (lateral) |
| **Drumlins** | Clew Bay, Upstate New York | 5–50 m high, 250–2 000 m long | 5–15 high, 100–300 long, 30–80 wide, in swarms aligned with the ice's flow | — | glaciated lowlands behind the end moraine |
| **Lake district** | Finland, Canadian Shield | thousands of lakes | M14's ice-scoured lakes, now on the ice age's ground | — | glaciated lowlands (M14 already makes them below 2 °C today; the ice age extends them) |

- **Ice flows down the network.** The ice's direction is the drainage
  network's (valley glaciers follow river valleys; on Earth they occupied
  them). Drumlins and moraine arcs align with it.
- **Trough shape.** A parabola, width-to-depth ~2–4 : 1, against a river's
  V. The trough widens and deepens the network's valley; the river (M14)
  then runs on the trough's floor.
- **Fjord walls are rocky coast** (biome 19, already); their sill makes
  their mouths shallow.

**Built so far (M15 task 3a, version 12).** U-troughs along glaciated
valleys with relief (100–250+ blocks) to channel the ice, as lines along
the network's nodes evaluated at full resolution; overdeepened floors
fill as lakes through M14's routing; tributaries cut less deep hang above
the main trough. Lowland ice sheets dig no troughs (M14's lake districts
stand for their scour). Fjords: rare, because few glaciated coasts have
mountains. Cirques at every glacier's head (version 14); end moraines
along the ice age's margin and drumlins behind it on low ground (version
15), aligned along Z. Arêtes and horns are left to emerge between cirques
and troughs.

## Weathering — inselbergs

| Landform | Earth analogues | Earth size | Ours | Walking | Where |
|---|---|---|---|---|---|
| **Bornhardt (dome)** | Uluru, Sugarloaf, Spitzkoppe, Zimbabwe's domes | 100–700 m high, 1–3 km across | 40–150 high, 200–600 across, convex, walls near vertical at the foot | round the base in 2–5 min; seen for kilometres | savanna and dry-forest plains, one every ~2–5 km, sometimes in clusters |
| **Tor / kopje** (boulder pile) | the Serengeti's kopjes, Dartmoor's tors | 5–30 m | 6–20 high, 20–60 across, stacked blocks | — | the same plains, more often than domes; also on temperate uplands (Dartmoor) at low weight (decision 4) |

- **Bare rock**, no soil on the dome; the plain around it is flat — that
  abruptness is the landform. Some domes are climbable on one flank, some
  not (decision 3).
- **Rivers flow round them**: applied before the network, which sees them.

## Rain — rounded, dissected hills

| Landform | Earth analogues | Earth size | Ours | Walking | Where |
|---|---|---|---|---|---|
| **Dissected hills** | Appalachian Plateau, Borneo's uplands, Brazil's *mar de morros* ("sea of hills") | valleys 100s of m apart | valleys 40–120 apart, 10–40 deep, finer than the network's 128-block cells; spacing shrinks as rain rises | a valley every 10–20 s | humid hills and uplands; density rises with aridity index 1 → 3 |
| **Half-orange hills** (*meia-laranja*) | Brazil's Atlantic coast, south China | convex domes of deep soil | rounded tops, convex above and concave below, 20–60 high | — | the wettest warm hills (tropical rainforest, humid subtropics) |

- **Soil creep rounds the tops**: convex hilltops, concave footslopes —
  the opposite of a dry land's sharp cliffs.
- **Below the network's resolution.** The network's cells are 128 blocks
  (up to ~1 000 in the largest worlds), so these small valleys are a stage-2
  layer, applied before the network samples the ground. Their streams are
  too small for M14's rivers; they are dry valleys or, later, brooks.

## M14's carried items

Not landforms, but this milestone owns them (task 5): closed basins cover
~29% of land against Earth's ~18% (target ≤ ~22%, criterion 6), and rivers
of 1+ m³/s sit in valleys only 11–13 blocks deep (target ≥ ~20). Dissection
and canyons both help the second.

## What the tests check (criterion 3)

Over many seeds, conditioned on the process weights:

- **Dunes**: only where the wind weight is positive; dune crests'
  orientation across the local wind (height autocorrelation longer along
  the crest than across it); wavelength 100–250; heights 6–30.
- **Benches, mesas, canyons**: only on flash-flood ground; height histogram
  of a bench field has steps (risers steep, benches flat); every canyon has
  a river in it.
- **Glacial**: U-valleys, cirques, fjords only where ice lay in the ice age;
  a trough's cross-section is U (width-to-depth and wall steepness against
  a non-glaciated valley of the same discharge); fjords end in a sill.
- **Inselbergs**: only on weathering ground; height above the surrounding
  plain 40–150.
- **Dissection**: valley density (cross-overs of a smoothed surface per km)
  rises with aridity index on rain ground.
- **Every landform in every legal world size** whose climates allow it
  (criterion 7), as `every_biome_in_every_world` does for biomes.

## Decisions (owner, 2026-11-04)

The five questions the draft asked, and the owner's answers.

1. **Linear (seif) dunes** — the owner asked for Claude's recommendation;
   recommended and recorded: **include them**, from a seasonal swing of the
   dune wind per latitude (dune-only, like the tilt), so each dune type
   follows Fryberger's wind regimes rather than being placed.
2. **Slot canyons** — **left for geology** ("if it's best to leave for
   geology, we can do that"): they need rock of differing hardness.
3. **Tilted wind for dunes** — the owner asked whether it would hurt
   anything; it does not: only the dunes read it, so M13's rain,
   temperatures and biomes are unchanged, and it costs one direction per
   latitude. The terrain changes anyway in M15 (`GENERATOR_VERSION` bump).
   **Included.**
4. **Tors on temperate uplands** — **yes** ("any kind of unique Earth
   topology would be so cool to see"). The owner's wider wish — every
   distinctive kind of Earth topography in time — is for later milestones
   too (karst and volcanoes already wait for geology).
5. **Mesas with a way up** — **some do** ("I'm fine if some of them do");
   about half, a tuning knob.
