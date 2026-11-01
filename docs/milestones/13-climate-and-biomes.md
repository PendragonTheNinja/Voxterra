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

**Status: in progress.** Spec and catalogue approved by the owner
(2026-11-01; decisions recorded at the end). Tasks 0 (map writer) and 1
(climate maps) done headlessly; next, task 2 (classification).

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
   - [x] Catalogue approved as drafted (owner, 2026-11-01): "a really
     awesome list … we can decide as we go if we want to add more biomes
     at any point or drop any."
   - [x] World map writer (2026-11-01): `vox-worldgen/src/map.rs`, test-only;
     `cargo test -p vox-worldgen --release world_map -- --ignored
     --nocapture` writes elevation, temperature and precipitation PNGs to
     `target/world-map/` (env: `WORLD_MAP_SEED`, `WORLD_MAP_SIZE`,
     `WORLD_MAP_PX`, `WORLD_MAP_DIR`), with the equators, tropics and polar
     circles marked, and prints land shares by rainfall and temperature and
     mean land rainfall per 10° of latitude. A dependency-free PNG writer
     (stored deflate). The elevation map is the "before".
   - [x] In-game readouts (2026-11-01): the telemetry line ends with the
     column under the camera — `temperate forest 11C 950mm lat 41.2` — and
     so does the **P** key's `VIEW` line. The climate map is built at world
     load, its time logged (`climate map: … built in … ms`).
1. **Climate maps (stage 1).** ADR-0017. The grid, temperature, winds,
   moisture advection, rain shadow, ocean moderation. Tests for criterion 4's
   physical causes. Tune against the world map, many seeds, every size.
   - [x] **Done headlessly (2026-11-01)** — `vox-worldgen/src/climate.rs`,
     ADR-0017 (proposed). 800×800 cells of 256 blocks at the default size,
     built in ~0.5 s (4 cores). Deterministic to the bit on every machine:
     basic arithmetic only, with its own `exp`. Tuned on the world map over
     four seeds — first runs were far too dry (40% of land under 250 mm) and
     streaked along the winds; fixed by continent-scaled drying with
     recycling, sideways mixing between rows, and orographic rain as a
     capped multiplier. Mean land rainfall per 10° of latitude now within
     ~10–20% of Earth's, same shape. Tests: determinism, wrapping across
     the seam, **rain shadows** (windward 3.3× the lee over 817 ridges;
     1.02× and failing with orographic lift disabled), latitude belts,
     continental cooling, snowlines. Not yet read by chunks:
     `GENERATOR_VERSION` unchanged until task 3.
2. **Classification.** Climate + terrain → biome, with boundary noise. Tests
   for criteria 3, 5 and 6.
   - [x] **Done headlessly (2026-11-01)** — `vox_worldgen::biome`:
     `Generator::column(x, z)` gives a column's surface, temperature,
     precipitation and biome, from `Elevation::surface` (height with the
     relief and uplift it was built from, at no extra cost) and the climate
     map — pure functions of `(x, z)`, so chunks, LOD and readouts agree.
     Dryness is Köppen's own threshold (`20·T + 140` mm); mountains get a
     treeline and snowline; mountains inside an ice-sheet climate are ice
     sheet, as Antarctica's are; shingle on cold or rough shores.
     `Generator` became `Clone` (the climate map is shared behind an `Arc`,
     built on first use). Unit tests: Earth's climates classify as Earth
     does (Manaus, Serengeti, Sahara, Sahel, Paris, Seattle, Gobi, Great
     Plains, Seville, Arkhangelsk, Yakutsk, Antarctica), tree- and
     snowlines, shores and seas, badlands.
   - **Tuning, from the survey** (`biome_survey`, ignored, release build;
     24 seeds per size). First pass: hot desert 0.5% of land, badlands
     5.1%, temperate forest 23%. Changes, each physical: drier subtropical
     sinking air; air over land dries 4× faster in the 15–30° belt;
     badlands need desert-level dryness and real roughness; a narrower
     Mediterranean band; the snowline at −5 °C; ice-sheet mountains;
     temperate rainforest wet *for its temperature* (≥ 4× the aridity
     threshold, ≥ 1 300 mm); drying and continental cooling scaled by the
     world's continent size (see "Open question" below).
   - **Criterion 5 — met.** Default size, 24 seeds, % of land (Earth in
     brackets, rough): tropical rainforest 7.4 (7–9), dry forest 6.2 (4–6),
     savanna ~9 (10–14), hot desert ~6 (12–14), hot steppe ~7 (5–8),
     Mediterranean 0.5 (2–3), temperate grassland 5.1 (6–8), cold desert
     ~7 (3–5), temperate forest 14.9 (8–12), temperate rainforest 1.9
     (1–2), boreal 8.5 (11), tundra ~7 (6–8), ice sheet ~12 (9–10), alpine
     ~5 (3–5), badlands ~2 (1–2); seas: polar sea ~6% of the world, deep
     ocean ~50%. Groups: deserts and steppe ~27% (25–35 asked), forests
     ~39% (30–40), tundra and ice ~19% (10–20). **Off Earth, recorded:** hot
     desert about half Earth's share and temperate forest ~1.5×; the
     Mediterranean band is thin, because what makes Rome Mediterranean is
     a dry summer, and climate here has no seasons.
   - **Criterion 3 — met from 64 km up.** `every_biome_in_every_world`
     (ignored; ~100 s in release) asserts every biome in 24 worlds at 64,
     205 and 524 km on seeds the tuning never saw. The survey agrees over
     24 more seeds per size. **The smallest world (32 km) is open:**
     temperate rainforest is missing in 5 of 24 seeds and alpine rock and
     snow in 1 — see "Open question".
   - **Criterion 6 — met.** The biome stretch a walker stands in, at
     ground sprint, default size: ~12 minutes walking north–south (across
     the climate belts), ~23 east–west (along them); at 64 km, 5 and 15.
     0.3% of land lies in flickers under 100 blocks.
   - The world map now draws the biomes (`biomes-<seed>-<size>.png`) and
     prints each biome's share.
   - Chunks do not read biomes yet: `GENERATOR_VERSION` unchanged until
     task 3.
3. **Surface materials.** The new blocks and their tiles; per-biome
   profiles (top, beneath, depth); beaches and rocky coasts; seabeds; sea
   ice; snow and ice sheets. `GENERATOR_VERSION` bump and re-pin.
   - [x] **Done headlessly (2026-11-01).** Thirteen blocks (ids 9–21, all
     opaque solid cubes): snow, ice, gravel, red sand, sandstone, red
     sandstone, three banded badlands rocks, coarse dirt, podzol, clay, mud
     — each with a procedural tile and a grain that reads as the material
     (pebbly gravel, banded sandstone, smooth snow and clay). Profiles in
     `vox_worldgen::materials`: every biome's top block and layers
     (rainforest over clay, sand over sandstone in deserts and on beaches,
     hamada in rough desert, podzol under taiga, gravel through tundra,
     snow over ice on ice sheets, snow caps on alpine rock, red sand over
     strata in badlands, mud and clay on the deep seabed, gravel on polar
     seabeds) with small-scale patches (`Elevation::patch_noise`, 40 and
     11 blocks) for sand through steppe, gravel through tundra, outcrops
     on meadows. **Badlands strata** are banded by world height in a
     per-world order, so cliffs read as layered rock. **Polar seas freeze**:
     ice on the sea surface. Profiles end within 12 blocks
     (`MAX_PROFILE_DEPTH`), stone beneath, so deep chunks stay uniform
     without classifying anything; chunk generation classifies a column
     only where a chunk meets its profile or its sea surface.
     `GENERATOR_VERSION` 3, fingerprint re-pinned (no version-3 world
     exists yet, so later M13 re-pins before the owner plays are allowed).
   - Tests: every profile ends in stone within the maximum depth; the sea
     fills and freezes; surfaces read as their biomes; strata are
     horizontal; **every block of the chunks through the surface, in every
     biome, equals the per-block profile** (fails if the lazy path's stone
     or the sea-ice path is broken).
   - **Cost**: chunk generation along the benchmark's sprint path, dev
     build, 1 800 chunks: 375 µs per chunk before, **364 after** —
     classification paid for by computing each chunk's surfaces once. A
     24-block profile first cost a tenth of all chunks their uniform fast
     path; 12 keeps all but 4%.
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

## The smallest world (decided 2026-11-01)

The smallest legal world (32 km) lacked a biome in some seeds: temperate
rainforest in 5 of 24, alpine rock and snow in 1. The owner chose (b) —
shrink landforms in small worlds, **but only when needed**. Done as two
calibrations, each a no-op wherever it is not needed:

- **Mountain ranges**: two of those worlds had no point above ~360 blocks —
  their mountain-belt field, a few lattice cells around a tiny world, had
  missed the land. Each world now checks, at creation, that at least 2.5%
  of its land lies under ranges (uplift ≥ 400); one that falls short lowers
  its belt gate until it does (`Elevation::calibrate_ranges`, bisection, like
  the coastline). Worlds of the default size and larger always have ≥ 4.6%
  over 48 seeds, so **their terrain is unchanged** (the fingerprint test
  passed untouched).
- **Ocean uptake** now scales with the continents like the drying length
  (ADR-0017 §5): a small world's narrow seas left its westerly coasts just
  short of rainforest wetness (1 220–1 320 mm).

Result: every biome in every one of 24 seeds at 32 and 64 km;
`every_biome_in_every_world` now asserts it from 32 km up (32 worlds, all
passing). Tested: `every_world_has_mountain_ranges_and_only_small_ones_are_changed`.

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
