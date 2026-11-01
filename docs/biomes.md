# Biome catalogue

*Draft 2026-11-01, for the owner's approval before M13 builds anything
(`docs/vision.md` §1.1 and §9: "decide the catalogue first"). Once approved,
this list is what climate must produce, what the size-scaling test checks,
and what vegetation (later) and creatures (§5) are written against.*

## Rules

- **Every biome is a real place.** Each has Earth analogues, so its look,
  its later plants and animals, and its folklore come from somewhere
  (`docs/vision.md` §1.1, §5). No invented biomes.
- **Biomes are classified, never placed.** A column's biome follows from its
  climate (temperature, precipitation) and its terrain (altitude, relief,
  distance to the coast, depth of water). Nothing scatters biomes at random;
  deserts sit in the subtropics and behind mountains because that is where
  the climate is dry.
- **Every biome exists in every legal world** (16 km to 2 097 km across), for
  every seed. A test enforces it.
- **Until vegetation exists, a biome is its ground, its grass colour and its
  climate.** Forest biomes differ from grasslands by soil and tint alone
  until the vegetation milestone — the catalogue lists the plants so they
  are decided now, not so M13 builds them.

## Land — climate biomes

Temperatures are the column's mean annual temperature *after* altitude
cooling; precipitation is annual. Both are in the game's own units, tuned to
walking time (ADR-0010) — Earth's values are given as the analogue, and the
tuning targets in the M13 spec say where the boundaries actually fall.

| # | Biome | Köppen | Earth analogues | Climate | Ground | Grass | Later: plants |
|---|---|---|---|---|---|---|---|
| 1 | Tropical rainforest | Af | Amazon, Congo, Borneo | hot, very wet | dark soil, clay beneath | deep green | tall broadleaf canopy, lianas, ferns |
| 2 | Tropical dry forest | Aw (wet end), Am | Gran Chaco, central India | hot, seasonally wet | red-brown soil | green-olive | deciduous broadleaf, bamboo |
| 3 | Savanna | Aw | Serengeti, cerrado, northern Australia | hot, moderate rain | coarse red-brown soil | yellow-green | acacia, baobab, tall grass |
| 4 | Hot desert | BWh | Sahara, Arabia, Sonora | hot, very dry | sand over sandstone; rock where rough | — | cactus, dead shrubs, rare palms by water |
| 5 | Hot steppe | BSh | Sahel, Karoo, outback fringes | hot, dry | coarse dirt, sand patches | dry yellow | thorn scrub, sparse grass |
| 6 | Mediterranean scrub | Cs | Provence, California chaparral, Cape | warm, moderately dry, near coasts | pale stony soil | grey-green | olive, cork oak, maquis |
| 7 | Temperate grassland | BSk, Dfa (dry) | Great Plains, Pampas, Eurasian steppe | mild-cold, moderately dry | dark rich soil | green-gold | tall grass, wildflowers, few trees |
| 8 | Cold desert | BWk | Gobi, Great Basin, Patagonia | cool-cold, very dry | gravel, pale sand, rock | — | sagebrush, saxaul |
| 9 | Temperate forest | Cfb, Dfb | eastern US, western Europe, Honshu | mild, wet | brown soil | fresh green | oak, beech, maple, birch |
| 10 | Temperate rainforest | Cfb (very wet, coastal) | Pacific Northwest, Valdivia, New Zealand | mild, very wet, windward coasts | dark soil, moss | deep blue-green | giant conifers, ferns, moss |
| 11 | Boreal forest (taiga) | Dfc | Siberia, Canada, Scandinavia | cold, moderate | podzol | dark cool green | spruce, fir, pine, larch |
| 12 | Tundra | ET | Arctic Canada, Siberia's north coast | very cold | thin soil, gravel, moss | brown-olive | moss, lichen, dwarf willow |
| 13 | Ice sheet | EF | Antarctica, Greenland | frozen | snow over ice | — | none |

## Land — terrain-shaped biomes

Chosen from climate **and** terrain, so they can share a climate with a
biome above and still differ (`docs/vision.md` §10.1: cold peaks against
cold plains).

| # | Biome | When | Earth analogues | Ground | Later |
|---|---|---|---|---|---|
| 14 | Alpine meadow | above the treeline, below the snowline | Alps, Rockies, Tibetan grassland | thin soil, stone outcrops | grasses, flowers, no trees |
| 15 | Alpine rock and snow | above the snowline, or too steep to hold soil high up | Alps' summits, Himalaya | bare stone, snow on the level, scree | none |
| 16 | Badlands | hot or warm and dry, with high relief | Monument Valley, Painted Desert, Bryce | red sand, banded red and orange rock | rare scrub |

## Coasts

| # | Biome | When | Earth analogues | Ground |
|---|---|---|---|---|
| 17 | Sand beach | gentle shore, not cold | most warm and temperate coasts | sand |
| 18 | Shingle beach | gentle shore, cold | Scotland, Baltic, Patagonia | gravel |
| 19 | Rocky coast | steep shore | Cornwall, Big Sur, fjord walls | stone |

## Seas

The sea surface stays water everywhere; the seabed and the polar surface
change.

| # | Biome | When | Seabed | Later |
|---|---|---|---|---|
| 20 | Warm shallow sea | shelf, tropical | pale sand | coral reefs |
| 21 | Temperate sea | shelf, mid-latitude | sand, gravel near cold coasts | kelp |
| 22 | Polar sea | high latitude | gravel, mud | **sea ice on the surface** (frozen where cold enough) |
| 23 | Deep ocean | off the shelf | clay and mud | — |

## Left for the milestones that make them possible

- **Wetlands, marshes, mangroves, river floodplains, lakes above sea level** —
  need hydrology (rivers know their upstream).
- **Volcanic biomes, karst, salt flats** — need geology, or erosion and
  drainage basins.
- **Coral reefs, kelp forests, all trees and plants** — need vegetation; the
  catalogue already says where each will grow.
- **Caves and their biomes** — need the 3D density and cave pass (geology).

## New surface blocks this implies

Sand, stone, dirt, grass and water exist. New: **snow**, **ice** (sea ice and
ice sheets; opaque and solid, unlike water), **gravel**, **red sand**,
**sandstone**, **red sandstone** and two or three **banded badlands rock**
colours, **coarse dirt**, **podzol**, **clay**, **mud**. Around twelve — palette
storage makes them free in memory (CLAUDE.md).

## As built (M13, 2026-11-01)

How the catalogue is generated (`vox_worldgen::biome`, `vox_worldgen::materials`;
ADRs 0017 and 0018). Climate is the annual mean — there are no seasons.

**Classification, in order:** sea by depth and sea temperature; land within
3 blocks of sea level is shore; mountains (uplift a real share of the height)
are rock and snow below −5 °C and alpine meadow below +1 °C — unless colder
than −14 °C, where ice sheet covers them, as in Antarctica; then ice sheet
(< −14 °C), tundra (< −5 °C); dry land by Köppen's threshold (`20·T + 140`
mm: desert under half of it, steppe under all of it; badlands where also
warm, rough and desert-dry); boreal forest (< 3 °C); tropical (≥ 20 °C:
rainforest from 2 000 mm, dry forest from 1 200, else savanna); Mediterranean
(≥ 14 °C and humid by less than 1.3× the threshold); temperate rainforest
(< 16 °C and at least 4× the threshold, never under 1 300 mm); temperate
forest. Boundaries wander with noise at 1 400, 350 and 24 blocks.

**Ground, top down** (stone beneath; no profile deeper than 12 blocks):

| Biome | Ground |
|---|---|
| Tropical rainforest | grass, dirt 3, clay 3 |
| Tropical dry forest | grass, dirt 3 |
| Savanna | grass (coarse-dirt patches), coarse dirt 3 |
| Hot desert | sand 4, sandstone 6; where rough, bare sandstone with sand in hollows (hamada) |
| Hot steppe | grass with sand and coarse-dirt patches, dirt 3 |
| Mediterranean scrub | grass with coarse-dirt patches, coarse dirt 2 |
| Temperate grassland | grass, dirt 5 |
| Cold desert | gravel with sand patches, gravel 2 |
| Temperate forest | grass, dirt 3 |
| Temperate rainforest | grass, dirt 4 |
| Boreal forest | podzol (grass patches), dirt 3 |
| Tundra | grass with gravel patches, dirt 1 |
| Ice sheet | snow 3, ice to the profile's end |
| Alpine meadow | grass with stone outcrops, dirt 1 |
| Alpine rock and snow | snow (well below the snowline, or in patches), stone, scree |
| Badlands | red sand 2, then horizontal strata of four banded rocks |
| Sand beach | sand 4, sandstone 3 |
| Shingle beach (cold or rough shores) | gravel 3 |
| Rocky coast | stone (gravel patches) |
| Warm shallow sea | seabed sand 3, sandstone 3 |
| Temperate sea | seabed sand or gravel 3 |
| Polar sea | seabed gravel 2; **ice on the sea surface** |
| Deep ocean | seabed mud 1, clay 3 |

**Grass colour** follows climate rather than the biome's name — deep green
where hot and wet, yellowed where hot and dry, fresh green where mild and
wet, olive and brown toward the cold — so it shifts smoothly across a
boundary, near and far alike (ADR-0018).


## Water biomes (approved 2026-11-01, built in M14)

Hydrology (M14) makes the biomes deferred above possible. Each comes from the
drainage network, not from climate alone.

| # | Biome | Where | Earth analogues | Ground |
|---|---|---|---|---|
| 24 | River | the channel | Amazon, Rhine, Thames | river gravel and sand; silt in slow lowland rivers |
| 25 | Freshwater lake | basins that fill and overflow | Great Lakes, Baikal, alpine tarns | silt, gravel at the shore |
| 26 | Salt lake / salt flat | closed basins in dry climates | Great Salt Lake, Salar de Uyuni, Lop Nur | salt crust over silt |
| 27 | Floodplain | flat ground beside lowland rivers | the Nile valley, Mississippi bottomlands | silt over river gravel |
| 28 | Marsh | flat, waterlogged, open, warm or temperate | the Camargue, the Everglades' sawgrass | mud, shallow water |
| 29 | Swamp | flat, waterlogged, wooded, warm | the Atchafalaya, the Pantanal | mud, shallow water |
| 30 | Bog / fen | flat, waterlogged, cold | Hudson Bay lowlands, Irish bogs | peat |
| 31 | Delta | large river mouths on gentle coasts | Nile, Mississippi, Ganges; Okavango inland | mud and sand, distributary channels |
| 32 | Mangrove coast | tropical delta and estuary shores | the Sundarbans | mud (mangroves with vegetation) |

New blocks: **silt**, **peat**, **salt**, **river gravel**.
