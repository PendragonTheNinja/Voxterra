# ADR-0017: Stage-1 climate — a planetary grid built at world creation

- **Status:** Proposed (2026-11-01, M13 task 1). To be accepted at the M13
  close, after the owner's walk.
- **Context:** M13 needs a climate for every column: temperature and
  precipitation, from which biomes are classified (`docs/biomes.md`).
  Temperature is nearly local (latitude, altitude). **Precipitation is not**:
  a column is dry because of a mountain range hundreds of kilometres upwind,
  or because it lies deep in a continent's interior. Stage 2 — chunk
  generation — must stay per-chunk independent (CLAUDE.md) and cannot look
  that far. CLAUDE.md has always planned for this: "Stage 1 (world creation):
  low-res planetary simulation — tectonics, climate, biome maps. Stage 2:
  local detail derived deterministically from stage-1 maps." This is the
  first stage-1 system, so its shape sets the pattern for later ones
  (hydrology, geology).

## Decision

### 1. A coarse grid over the whole torus, computed from the seed and size

`vox_worldgen::climate::ClimateMap::build(elevation, shape)` samples the
elevation field on a grid (3×3 samples per cell for mean height and land
share), then simulates climate on it. Cells target 256 blocks, with 128 to
1 024 cells per axis: 800×800 at the default size, 128×128 at the smallest,
1 024×1 024 (2 048-block cells) at the largest. The cap is on the count, not
the size, because climate belts scale with the world (ADR-0012) while ranges
do not (ADR-0010) — a rain shadow needs cells that can see a 7 km range at
the default size, and the largest worlds trade that resolution for a bounded
build.

### 2. Derived, not saved

The map is a pure function of `(seed, size)`, like the coastline
calibration (ADR-0012), so it is rebuilt when a world opens rather than
stored. Built in ~0.5 s at the default size in the cloud container (4
cores); `rayon`-parallel over cells and the two wind sweeps. Caching it in
the world folder later is allowed as an optimisation, never as the source
of truth — and any change to its output is terrain output, covered by
`GENERATOR_VERSION` (CLAUDE.md) once chunks read it.

### 3. Deterministic on every machine: basic arithmetic only

Only `+ − × ÷`, comparisons and `floor`, which IEEE 754 makes bit-exact
everywhere — the rule the elevation field already follows implicitly.
`f32::exp` and the trigonometric functions call the platform's maths
library and may differ in the last bit between Windows and Linux; through
an integrated simulation and a classification threshold, a last-bit
difference can change a biome. So `climate::exp` is built from basic
operations (Cody–Waite reduction and a degree-6 polynomial; relative error
under 2·10⁻⁶), and everything latitude-dependent is written in
`sin(latitude)`, which `WorldShape::latitude_sine` gives exactly, never in
degrees. Parallelism never reduces across threads.

### 4. The mechanisms, mirrored from Earth with game dimensions

| Mechanism | Model | Earth | Here |
|---|---|---|---|
| Latitude | sea-level temperature as a polynomial in `sin(lat)` | annual zonal means | 27 °C equator, 19 at 30°, 0 at 60°, −25 at the poles |
| Altitude | lapse rate | 6.5 °C/km | **32 °C per 1 000 blocks** |
| Winds | moisture swept along each row: easterlies within ~30°, westerlies ~30–60°, polar easterlies, blended ±5° at the edges | three-cell circulation | as Earth, twice round the torus |
| Ocean | moisture taken up over the sea toward a capacity falling 4.5% per °C | Clausius–Clapeyron (~7%), damped for annual totals | equatorial sea ~3 000 mm/yr |
| Rising / sinking air | a rain multiplier by latitude | ITCZ wet, subtropical highs dry | 1.6 at the equator, 0.25–0.28 at 22–30°, 1.25 at 45–55°, 0.4 at the poles |
| Interiors | moisture falls by e over ~30 km of land at the default size (60% recycled), **4× faster in the 15–30° belt**; interiors colder by up to 9 °C at high latitudes | rain reaches thousands of km inland; hot subtropical interiors dry incoming air fast (the Sahara is dry well inland of both coasts) | continents are ~60× smaller than Earth's |
| Rain shadows | air lifted over high ground cools; above what it can hold it rains harder (up to 5×) and is stripped out, reaching the lee dry | windward slopes 2–5× | windward 3.3× the lee, measured over 817 ridges |
| Sideways mixing | moisture diffuses between neighbouring rows as it travels; a final blur | winds are not lines | streaks from single peaks removed |

**The lapse rate is exaggerated on purpose** (owner, 2026-11-01): our peaks
reach ~1 360 blocks against Earth's 8 850 m (ADR-0010), so at Earth's rate
the highest would be ~9 °C colder than its coast and no mountain would carry
snow. 32 °C per 1 000 blocks puts permanent snow (mean ≤ −5 °C) above
~1 000 blocks at the equator, ~530 at 45°, ~170 at 60° and at sea level
near the poles — the M13 snowline targets. The cost, accepted: high plateaus run
cool, so taiga can sit on a 300-block plateau in the temperate belt.

### 5. "Into a continent" scales with the continents

Distances that really mean "how far into a continent" — the drying length
and continental cooling — are multiplied by `Elevation::continent_scale`:
the world's continent wavelength over the default world's (1 at the default
size, ~0.2 at the smallest). Without it a 32 km world's ~10 km continents
never dried out and had no deserts at all (`docs/vision.md` §1.1: climate
scale grows with the world, landform scale does not; small worlds must
still hold every biome).

### 6. Stage 2 reads the grid, then applies local detail

A column reads sea-level temperature and precipitation by bilinear
interpolation between cell centres, wrapping with the torus (content
addressing — part of terrain generation, one of CLAUDE.md's three
canonicalising sites), then cools for its own exact height. Boundary noise
(two octaves, 1 400 and 350 blocks, ±1.5 °C and ±22% rain) lets biome
edges wander; classification is `vox_worldgen::biome` (M13 task 2).

## Measured (seed 0x7e22ac0de and three others, default size)

Mean land precipitation by 10° of latitude from the equator, mm/yr:
~1 500–2 400, 1 000–1 330, 430–600, 550–710, 725–815, 525–575, 230–280,
100–130, 70–90 — each within ~10–20% of rough figures for Earth's land
(~2 000, 1 300, 600, 600, 700, 600, 400, 200, 100), and the same shape:
wet equator, dry subtropics, wetter mid-latitudes, dry poles.

## Consequences

- **`vox-worldgen` depends on `rayon`** — data parallelism, allowed by
  CLAUDE.md, headless.
- **World creation and load cost ~0.5 s more** at the default size; tests
  use a 65 536-block world (256 cells) to stay fast.
- **Climate reads elevation; elevation never reads climate.** Landforms
  shaped by climate (erosion, the next milestone) are applied after, and
  stage 1 keeps reading the uneroded base.
- **What this does not model**: seasons (annual means only), ocean currents
  (cold coastal currents that dry the Atacama and Namib), monsoons. Each
  could refine the grid later without changing its shape.
- Tests: determinism to the bit, wrapping across the seam, rain shadows
  (fail with orographic lift disabled: windward/lee 1.02), the latitude
  belts, continental cooling, snowlines.
