# Milestone 13 — Climate & Biomes

**Goal:** every place on the planet gets a climate from real causes —
latitude, altitude, oceans, prevailing winds, mountains — and a biome painted
from that climate and its terrain, visible from underfoot to the 64 km
horizon. Walking should cross recognisable, real-world biomes that blend into
one another; flying high should show the planet's belts: wet green equator,
dry subtropics, temperate forests, taiga, tundra, ice.

Depends on M10 (the elevation field, mountains to cast rain shadows), M11 (the
horizon, so climate belts are visible tens of kilometres off) and M12
(headroom: generation and meshing are about to do more per chunk). Written
2026-11-01 from `docs/vision.md` §1.1 and §10.1, with the biome catalogue in
`docs/biomes.md`.

**Status: draft.** The owner answered four of the five open questions
(2026-11-01; recorded at the end). Nothing is built until the catalogue
(`docs/biomes.md`) is approved.

## Why this milestone exists

Pillar 2 is realistic world generation, and today the generator knows one
vertical profile: grass, four blocks of dirt, stone, and sea water below sea
level. Every land column on the planet looks the same; every sea is the same
blue over the same stone. The elevation field (ADR-0010) has continents,
plains, mountain belts and trenches, but nothing on them says where on the
planet you are. Climate is the next layer the proposed order needs (§9):
hydrology needs precipitation, geology's placer gold needs rivers,
vegetation needs biomes and surfaces, creatures need biomes.

## How climate works here

Mirror Earth's **mechanisms**, choose the **dimensions** for play (ADR-0010's
rule). Five mechanisms, each simple, each testable:

1. **Temperature from latitude.** Sea-level temperature follows insolation,
   `latitude_sine` (ADR-0012) — warmest at the equators, coldest at the poles,
   every band in every world.
2. **Temperature from altitude.** Colder going up. Earth's lapse rate
   (6.5 °C/km) is useless here: our peaks reach ~1 360 blocks against Earth's
   8 850 m, so they would be only ~9 °C colder than the coast and no mountain
   would carry snow. The game's lapse rate is exaggerated, as ADR-0010
   exaggerated relief, and **tuned to targets** (criterion 4), not to a ratio.
3. **Moderation by the ocean.** Coasts are milder and wetter than
   continental interiors; interiors are drier and more extreme.
4. **Prevailing winds carry moisture.** The three-cell model by latitude:
   easterly trade winds within ~30°, westerlies ~30–60°, polar easterlies
   beyond. Moisture is picked up over the sea, carried downwind, and rained
   out over land. The rising air of the equatorial belt is wet; the sinking
   air of the subtropical highs (~20–35°) is dry — which is why Earth's great
   deserts sit there.
5. **Mountains cast rain shadows.** Air forced up a range rains out on the
   windward side; the lee is dry. A desert should be found behind a range
   *because the range is there*.

Then **biomes are classified from climate and terrain** (`docs/biomes.md`):
temperature and precipitation give the climate class (Whittaker / Köppen),
and altitude, relief and the coast pick the terrain-shaped biomes (alpine,
badlands, beaches) that share a climate with their neighbours but not their
ground (§10.1). A biome sets the column's **surface materials** — top block,
the layer beneath and its depth, the seabed, sea ice — and its **colour**.

## Biomes and the shape of the land

The owner's point (2026-11-01): two different biomes should not differ only
in soil and colour — the land itself has a different shape in a desert, a
rainforest or a glaciated range. That is true of Earth, and the causation is
worth stating, because it decides what M13 builds and what comes after:

- **The land comes first, and makes the climate.** Mountains, coasts and
  continental interiors (the M10 elevation field) decide where it rains and
  where it is cold — the rain shadow behind a range, the dry interior of a
  continent. M13 builds this direction.
- **Then the climate reshapes the land, through erosion.** Under a dry
  climate rock weathers into mesas, buttes, canyons and dune seas; under a
  wet one hills wear round and valleys branch densely; under ice, glaciers
  carve U-shaped valleys, fjords, cirques and sharp horns. Earth's biomes
  look different largely *because* of this. It is the **hydrology & erosion**
  milestone that follows M13, and it reads M13's climate maps — temperature
  and precipitation are exactly its inputs.
- So **M13 paints biomes on today's terrain**; the next milestone lets each
  climate carve its own landforms. Biome-specific landforms are therefore a
  non-goal here by the owner's choice, not an omission — and the climate
  maps are built so erosion can read them.

As close to real life as possible while still making sense as a game
(the owner's standing rule; ADR-0010): mechanisms real, dimensions chosen for
play.

## The architecture this needs

**Stage-1 climate maps, built at world creation** (CLAUDE.md: "Stage 1:
low-res planetary simulation — tectonics, climate, biome maps. Stage 2: local
detail derived deterministically from stage-1 maps"). This milestone builds
the first stage-1 system, so its shape is an ADR (ADR-0017):

- Winds and rain shadows are **not local**: a column's rainfall depends on
  the mountains hundreds of kilometres upwind. No single chunk can compute
  that, and stage 2 must stay per-chunk independent. So a coarse grid over
  the whole torus is computed once per world from the seed and size —
  elevation sampled per cell, moisture advected along the winds, rows
  wrapping around the torus — and every column reads it.
- **Deterministic and derived**, like the coastline calibration (ADR-0012):
  the same seed and size give the same maps on every machine, so they can be
  rebuilt on load rather than saved (caching them in the world folder is an
  optimisation the ADR may take, never a source of truth).
- **Budget:** built in ≤ ~1 s at the default size on the owner's machine, and
  bounded at the largest size (the grid's cell size grows with the world, as
  climate belts do — `docs/vision.md` §1.1: climate scale grows with the
  world, landform scale does not).
- **Per column, stage 2 is cheap and local:** bilinear map lookup → altitude
  cooling from the column's exact height → a little noise so boundaries are
  not grid-shaped → classify. Target ≤ ~100 ns a column, beside elevation's
  ~176.

**Biome colour near and far is one decision** (ADR-0018). Grass, and later
leaves, take a colour from climate, and it must be the same at the player's
feet and at 64 km, or the LOD boundary shows. Two candidates, the owner
chooses (open question 3):

- **(a) A climate tint in the shader**, from the stage-1 map uploaded as a
  small texture that wraps like the torus. Smooth gradients across belts, no
  change to vertex formats or greedy merging, the LOD tinted by the same
  lookup for free. Costs: a texture lookup per fragment, and a decision on
  how the shader addresses a periodic map without putting the seam back into
  render code (CLAUDE.md allows three canonicalising sites; this would be a
  fourth, or part of "generation").
- **(b) Distinct grass blocks per biome group** — four or five grasses.
  Simple, no shader work, but colour changes in steps at biome edges.

**The LOD must show biomes.** Today a LOD cell carries only a height, and the
mesher paints it grass, or sea above a seabed. `lod_heightfield` returns a
surface material per cell as well, chosen exactly as the full-resolution
column at that cell's sample is, so desert reads as sand and ice as ice to the
horizon.

## Acceptance criteria

1. **Catalogue first.** `docs/biomes.md` approved by the owner before any
   generation code; every biome in it produced, none outside it.
2. **Instruments before tuning.** A headless **world map**: an ignored test
   writes the whole world's biomes (and temperature and precipitation) as
   images, so the planet can be reviewed in one picture — the sandbox can
   read PNGs. In game, the telemetry line and the **P** key's `VIEW` line add
   the column's biome, temperature, precipitation and latitude.
3. **Every biome in every world.** For every legal world size (smallest,
   default, largest, and two between) and at least 50 seeds each, every land,
   coast and sea biome in the catalogue covers at least a small share of the
   world (thresholds set from measurement in task 1, recorded here). This is
   `docs/vision.md` §1.1's guarantee, as a test.
4. **Physical causes, as tests.**
   - Temperature falls with latitude and with altitude, monotonically.
   - **Rain shadow:** across mountain ranges perpendicular to the wind, the
     lee is markedly drier than the windward side at the same latitude.
   - **Deserts sit where Earth's do:** most hot-desert area within the
     subtropical belt or in a rain shadow; little at the equator.
   - **Snowline targets:** permanent snow only on the highest equatorial
     peaks (≳ 900 blocks), on mid-latitude peaks above ~450–600, near sea
     level in polar latitudes. Measured, then the lapse rate tuned to them.
   - Coasts milder than interiors at the same latitude.
5. **Earth-like proportions.** Share of land per biome group within broad
   bounds taken from Earth (deserts and steppe ~25–35% of land, forests
   ~30–40%, tundra and ice ~10–20%, …), measured over many seeds, recorded.
6. **Biomes at walking scale.** At ground sprint (5.612 m/s — never judged
   from spectator flight), a typical biome patch takes ~5–20 minutes to
   cross; a 30-minute walk from the median land start meets at least two
   biomes; edges blend over tens to a couple of hundred blocks
   (dithered materials, smooth colour), never a straight grid line.
7. **Near and far agree.** The LOD surface material at a cell equals the
   full-resolution surface there; colour is identical across the
   full-resolution edge (tested, and checked by eye: no visible seam).
8. **Generation stays sound.** Per-chunk independence and determinism hold
   (existing tests extended to materials); `GENERATOR_VERSION` bumped and
   `terrain_fingerprint_is_pinned` re-pinned in the same commit; worlds of
   older versions refused as now. World creation and load stay under ~1 s
   extra at the default size.
9. **No performance regression worth the name.** `--bench` against M12's
   close: chunk generation per chunk and LOD node cost measured; sprint fps
   and backlog within ~10% (backlog still ≤ ~350); standing fps within ~5%.
10. **Bookkeeping.** ADR-0017 (stage-1 climate maps, lapse rate), ADR-0018
    (biome colour near and far); `docs/biomes.md` amended where play changes
    it; retro with the world map before and after.

## Tasks

0. **Catalogue and instruments.** Owner approves `docs/biomes.md`. The world
   map writer (biomes, temperature, precipitation, plus today's elevation),
   and the in-game readouts. Run the map on today's generator as the "before".
1. **Climate maps (stage 1).** ADR-0017. The grid, temperature, winds,
   moisture advection, rain shadow, ocean moderation. Tests for criterion 4's
   physical causes. Tune against the world map, many seeds, every size.
2. **Classification.** Climate + terrain → biome, with boundary noise. Tests
   for criteria 3, 5 and 6.
3. **Surface materials.** The new blocks and their tiles; per-biome
   profiles (top, beneath, depth); beaches and rocky coasts; seabeds; sea
   ice; snow and ice sheets. `GENERATOR_VERSION` bump and re-pin.
4. **Biome colour and the LOD.** ADR-0018; the chosen colouring; surface
   material per LOD cell; criterion 7's tests.
5. **Owner's walk.** A play session on foot and in flight, judged at walking
   speed, with fixes; `--bench` (criterion 9).
6. **Retro.**

## Non-goals

- **Vegetation** — trees, plants, crops — its own milestone after block
  shapes, as proposed (owner, 2026-11-01). The catalogue records what will
  grow where.
- **Rivers, lakes above sea level, erosion, wetlands** — hydrology is the
  next step and needs this milestone's precipitation.
- **Strata, caves, ores** — geology.
- **Seasons and weather** — climate here is the annual mean; no snow that
  comes and goes, no rain falling.
- **Temperature as a survival mechanic** — deferred with survival (CLAUDE.md).
- **World-creation sliders** — any climate constant stays a constant; the
  settings screen comes once its systems exist (§1.2).
- **Block shapes and slopes** — their own milestone, after surfaces settle.
- **Biome landforms** — dunes, mesas, canyons, glacial valleys — come with
  hydrology & erosion, shaped by this milestone's climate (owner,
  2026-11-01; see "Biomes and the shape of the land").

## Notes for whoever builds this

- **Climate reads the elevation; elevation never reads climate** — unless a
  landform is chosen (question 5), in which case it is added *after* climate
  as a height offset and climate keeps reading the base elevation. A cycle
  between them would make a column's height depend on itself.
- **Walking time is the judge** (CLAUDE.md, ADR-0010). Climate belts scale
  with the world; biome patches and landforms must not become hours wide in
  a large world or a single colour in a small one.
- **Ask the property you mean** (ADR-0011): sea ice and ice sheets are solid
  and opaque, unlike water; snow is a full block until shapes exist.
- **The world map is the main review tool.** Most of this milestone is
  judged by looking at the planet, then by walking on it.
- Measure chunk generation under streaming (lesson 6): uniform chunks must
  stay O(1) — a buried-rock chunk should not classify 1 024 columns.

## Decisions (owner, 2026-11-01)

1. **The catalogue** (`docs/biomes.md`) — *open*: awaiting the owner's
   approval or changes.
2. **Mountains are cold enough for snow:** altitude cooling exaggerated to
   criterion 4's snowline targets, accepting cooler high plateaus.
3. **Biome colour is a smooth climate tint in the shader** — option (a) of
   "The architecture this needs"; ADR-0018 settles how the shader addresses
   the periodic map.
4. **Vegetation stays its own milestone**, after block shapes.
5. **No landforms in M13**: they come from erosion under each climate in the
   hydrology & erosion milestone.
