# ADR-0025: The tectonic framework — terranes at walking scale, plates at world scale

- **Status:** Proposed (2026-11-10). The *decision to have one* is the
  owner's (2026-11-10, `docs/geology.md` §4, option C: "yes we do this");
  this is how. **Built** (milestone 16 tasks 1 and 2, 2026-11-11):
  the framework, `vox_worldgen::tectonics`, and since generator version
  9 the terrain placed by it — corrected by the building in both tasks;
  see *As built* and *As built: task 2* at the end. ADR-0010's guard
  tests pass. **To be accepted after the owner's flight and `--bench` on
  version 9**, and his answer to the two questions task 2 leaves open
  (narrower shelves; smaller landmasses). On acceptance it
  supersedes the *placement* of ADR-0010's continents, mountain belts and
  trenches; ADR-0010's scale — its wavelengths, amplitudes and the
  walking-time table — stands unchanged and is this ADR's hardest
  constraint.
- **Context:** The terrain's large features are placed by unrelated noise
  fields (`elevation.rs`): a continent field; a ridged "orogeny" field
  whose ranges are let through by a second, 13 000-block **gate** noise
  ("crust is being shortened along a few plate boundaries and nowhere
  else", its comment says — the gate *stands in* for plate boundaries);
  and trenches from a third ridge and gate. Geology needs those features
  to have causes (`docs/geology.md` §4). The obvious design — a dozen
  plates, mountains on their convergent edges — fails on arithmetic that
  is worth writing down, because it is the trap:

  > ADR-0010 puts the next mountain belt 13 000 blocks away — 39 minutes'
  > walk — and that number is why the world is good to walk. A dozen
  > plates in a 204 800-block world are ~50 000 across. Ranges only on
  > their edges would be three to four hours apart. The world would be
  > tectonically correct and as empty as the one ADR-0010 was written to
  > fix.

## Decision

### 1. Two scales, as Earth has

Earth's continents are not single slabs. Each is a mosaic of **terranes**
— old cores, strips of island arc, slivers of other continents — welded
together over billions of years, with a worn-down mountain belt along
every weld. North America has dozens. Most of Earth's ranges are on these
old sutures, not on today's plate boundaries: the Appalachians, Urals,
Caledonides, and the Tien Shan — an old suture squeezed high again by a
collision a thousand kilometres away.

So the framework has two levels, at the two scales `docs/vision.md` §1.1
already separates:

| | Size | Scales with | Gives |
|---|---|---|---|
| **Terranes** | ~13 000 blocks across — ADR-0010's belt spacing | Nothing: *landform scale*, tied to the player's legs, the same in every world | Sutures: where the mountain belts are |
| **Plates** | Tens of thousands of blocks; ~a dozen in the default world | World size: *climate scale*; never fewer than three | Today's boundaries: the highest ranges, trenches, ocean ridges, rifts, arcs |

A plate is a set of terranes. Every plate boundary is also a terrane
boundary; most terrane boundaries are inside a plate, and are old.

### 2. Terranes are a jittered lattice

One seed point per cell of a lattice around the torus (a whole number of
cells per lap, at least three — ADR-0012's rule for every periodic
field), each displaced within its cell by a hash of the world seed. A
column belongs to its nearest seed, measured after a smooth warp of the
coordinates so that boundaries curve. The seeds that matter are found
among the twenty-five cells round the column (nine are not enough: with
seeds free to sit anywhere in their cells, a seed two cells away can be
the nearest): a pure function of `(x, z)`, no grid to interpolate and so
nothing to show through as squares. The distance from a column to the
line between two seeds is exact — the difference of the squared
distances over twice the seeds' separation — which is all a belt's
profile needs.

### 3. World creation writes each terrane's story

Once per world, over the terrane list (about 250 at the default size; it
is the stage-1 "simulation", and it is a table, not a grid):

1. **Plates.** A dozen terranes are chosen as plate centres, well spread;
   every other terrane joins a plate by growing outward from the centres
   with uneven speed, so plates are irregular and of different sizes.
   Each plate gets a drift vector.
2. **Continents.** Some plates carry a continent: a cluster of terranes
   grown from a core that sits *off-centre* in its plate, so one side of
   the continent reaches the plate's edge and the other does not — an
   active margin and a passive one. Clusters are grown until the world's
   land target is met; the exact coastline is then calibrated to 42% as
   it is today.
3. **Each continent's past.** Its first one to three terranes are
   **cratons** (ages Eoan–Ferrian). The rest were *accreted* onto them in
   order, outward, and each records the **step** at which it was welded
   on: 1 for the first belt of terranes round the cratons, 2 for the
   next. The first belt round an Eoan core is Ferrian; everything since
   is Thalassian; and ground on a plate boundary that is closing today
   is Orean whatever its history (cratons excepted). The weld between
   terranes of *different* steps is a **suture** — an old mountain belt,
   dated by the younger of the two, running round the continent's core as
   the Appalachians wrap North America's. The joint between two terranes
   of the *same* step is a **seam**: it runs across the belts, was never
   a range, and raises low hills at most. (Without the distinction every
   terrane is ringed by mountains — a honeycomb of closed basins.)
4. **Each plate boundary's kind**, from the two plates' relative drift
   across it and the crust on each side (`docs/geology.md` §4's table):
   convergent, divergent or transform; ocean–ocean, ocean–continent or
   continent–continent; and which side goes under (ocean under continent;
   between two oceans, the older plate). The drift is fixed for a pair of
   plates; *across what* it is measured is each stretch of boundary's own
   direction bent two parts in three toward the trend of the whole
   boundary. So a boundary keeps its character along its length and
   changes it only where it truly turns — a subduction zone that becomes
   a transform round a bend, as the Pacific's edge does between Alaska
   and California — and the crust changes along it too, so one
   subduction zone runs from island arc to continental arc, as the
   Aleutians run into Alaska.
5. **Hotspots**: a few fixed points (about two for every five plates),
   placed without regard to the plates, each with a track along the
   drift of the plate above it.

### 4. The framework replaces the gates, not the mountains

`Elevation` keeps every shape it has. Three inputs change source:

| Today | Becomes |
|---|---|
| The **continent field** (noise) | Signed distance to the nearest boundary between continental and oceanic terranes, plus the same fine coastal octaves, minus the same calibrated offset. The shelf, slope and abyss profile is untouched |
| The **belt gate** (13 000-block noise) that decides where the ridged orogeny field may raise mountains | A **belt envelope** across each suture and convergent boundary: full at the line, falling to nothing over ADR-0010's range half-width. Its strength is the boundary's: continent–continent collision 1.0; continental arc ~0.75; a suture by its age, from ~0.6 (Orean) down to ~0.15 (Ferrian: hills, not mountains); raised again where a suture lies within reach of an active collision (the Tien Shan). The ridged field, its floor, the squared-smoothstep shaping, `MAX_UPLIFT` and "height earned by extent" are exactly as they are |
| The **trench ridge and its gate** | A trench envelope along the subducting side of each convergent boundary in deep water, with the same depth |

New, and small: a **mid-ocean ridge** (a broad rise of ~100 blocks with a
rift along its crest, the floor deepening away from it as the square root
of distance — the cooling curve of real sea floor); a **rift valley**
where a divergent boundary crosses a continent (a floor dropped 30–120
between raised shoulders); a **foreland sag** on the continental side of
each active belt. The arc line and the hotspot tracks are provided for
milestone 17's volcanoes and change no height now.

### 5. ADR-0010's statistics are held by calibration, as the coastline is

Land fraction used to be an accident of the seed until each world
calibrated its own coastline to hit 42% (ADR-0012). The same is done for
mountains: at world creation the belt strengths are scaled so that the
share of land under ranges, and the distribution of the uplift field
over land, match generator version 8's (measured over many seeds, pinned
as numbers). `MIN_RANGE_SHARE` and `calibrate_ranges` already do the
small-world half of this.

Then the guard: `terrain_changes_as_you_travel` and the ADR-0010
walking-time measurements must pass on the new terrain within stated
tolerances *before* M13–M15's tunings are touched. If they cannot be
made to pass, the framework is wrong, not the tests.

### 6. Provinces are read off the framework

A column's province (`docs/geology.md` §6) follows from its terrane's
kind and age and its distances to the nearest suture, plate boundary and
coast — craton interior, platform, basin (a slow sag inside a craton),
fold belt (the outer part of a belt envelope), orogen core (the inner),
suture, arc, foreland, passive margin, rift, ocean floor by its distance
from the ridge. No second map.

### 7. Deterministic and cheap

Hashes, integer lattice arithmetic, `+ − × ÷` and square roots only (all
bit-exact everywhere; no trigonometry — drift vectors are normalised
hash pairs). The per-world table is built sequentially in a fixed order.
Per column, the framework costs one search of the cells round it plus a
few table reads, in place of three noise fields it removes — and that
trade is **not** even: see *As built*.

## Consequences

- **Every world's terrain changes**: a new `GENERATOR_VERSION`, and every
  existing world refused (audit finding D2 is still open and is now
  closer to mattering).
- **M13, M14 and M15 are re-verified on the new terrain**: every biome in
  every world; the water tests; closed basins re-measured (expected to
  fall: interiors now have forelands and margins to drain to); the
  process weights' coverage (dune seas' share of desert, and the rest).
  Each has its survey; none should need a new mechanism.
- **Order of work.** M15's remaining tasks tune landforms to terrain that
  this ADR is about to re-place. Tuning them twice is waste. **Proposed:
  milestone 16's tasks 1 and 2 (the framework, then terrain placed by it)
  are done before M15's tasks 2–6**, and M15 is finished on the terrain
  it will ship on; then milestone 16 continues with strata. This is the
  owner's call, since it reorders approved work.
- **Climate gets real geography**: rain shadows behind ranges that are
  where ranges belong, coasts of two kinds.
- **The smallest world** (32 768 blocks) has a 3 × 3 lattice of terranes
  and three plates: one continent of a few terranes, one active margin,
  one passive. It will not have every province; the spec's "every world,
  every size" criterion states which it must.

## Alternatives considered

- **Plates only** (no terranes). Fails ADR-0010 by a factor of four, as
  above — or needs sixty "plates", which is terranes by another name
  without the history that makes them geology.
- **A grid-based plate simulation.** Option B in `docs/geology.md` §4:
  the most defensible, months of work, and no guarantee the result walks
  well.
- **Keep the noise, label it** (option A). Rejected by the owner with
  option C's acceptance.

## As built (milestone 16 task 1, 2026-11-11)

`crates/vox-worldgen/src/tectonics.rs`: `Tectonics::new(seed, shape)`,
held by `Generator` (`Generator::tectonics()`), read by nothing in
generation; `GENERATOR_VERSION` is still 8. Fifteen tests and four tools
(`tectonics_map`, `tectonics_survey`, `belt_jump_scan`,
`tectonics_cost`). Map: `docs/gallery/m16/`.

**What a default world holds** (24 seeds, 204 800 blocks): 256 terranes
on 13 plates; 7–11 continents; 46% continental crust. Boundary length by
kind: sutures 24%, seams 17%, ridges 16%, passive margins 14%,
continental arcs 9%, transforms 8%, island arcs 7%, collisions 3%,
rifts 2%. Every world has both kinds of coast, an arc, a ridge and
sutures; collisions and rifts are in most worlds, not all. The smallest
world (32 768) is nine terranes on three plates with four continental.

**What the building corrected in this ADR:**

- *Nine cells are not enough* (§2), and neither is "the four nearest
  seeds". The belt envelope must be continuous, and two shorter versions
  of it were not: the continuity test found a 0.16 step where a fifth
  terrane came into range, and then another. The rule that works is
  geometric, not a count: consider every seed within two belt half-widths
  of being nearest. A seed farther than that can neither be half of a
  boundary that reaches the column nor come between two that are.
- *A boundary's line runs on for ever; the boundary does not.* Each
  pair's envelope is also faded by how far past the boundary's end the
  column is — the most that any third seed is nearer than either of the
  pair — so belts end in a rounded cap at each junction instead of
  crossing the next terrane.
- *Kind per plate pair was too coarse and kind per segment too fine*
  (§3.4): per segment, every kink flipped a subduction zone to a rift
  and back within a terrane's width.
- *Sutures and seams* (§3.3), above.
- *Continentalness is clamped* at 3 500 blocks either side of the coast
  (`COAST_REACH`) — more than the coastal profile's width — which is
  also what lets most columns stop the search at nine cells.
- *Tunables are a `Params` struct* (terrane size, plate size, continental
  share), so that the owner's "every tunable a per-world option" needs a
  field in `world.meta` and nothing here.

**Cost, measured — and task 2's first problem.** In the cloud container,
best of seven over 400 000 scattered columns: one search with both
fields (`Tectonics::column`) is **~165 ns**; today's whole terrain
height is **~112 ns**. The framework replaces about four of the
height's noise octaves, perhaps 45 ns. So a column's height would cost
roughly **twice** what it does now if `Elevation` simply called the
framework per column. That would undo part of the 2026-11 audit's
generation work and must not ship. The way out is that nobody asks for
one column: chunk generation asks for 32 × 32 (the `GroundCache`), the
LOD sampler for a grid, the stage-1 maps for a grid. Across a patch a
few dozen blocks wide the seeds that matter are the same handful, so the
search is done once per patch and each column costs the warp and a few
distances. Task 2 builds that and measures it with `tectonics_cost`,
the generation benches and `--bench`, before and after; the per-column
entry point stays for tools and tests.

**For task 2's calibration** (§5): the envelope as built covers 22% of
continental ground above 0.25, 10% above 0.5 and 1.4% above 0.75.
Version 8's belt gate lets mountains through over most of the land and
leaves rarity to the ridged field, so the envelope is the narrower of
the two by a wide margin; holding ADR-0010's range share will mean
scaling strengths up or widening belts by kind (a collision's orogen is
wider than an old suture's), not only matching a number.

## As built: task 2 — the terrain placed by it (2026-11-11)

`GENERATOR_VERSION` **9**. `Elevation` holds the framework and reads it
for every column; every shape §4 keeps is kept.

**What reads what** (`elevation.rs`):

| Input | Version 8 | Version 9 |
|---|---|---|
| Continent field | Three noise octaves (56 / 21 / 7.5 km) | `continentalness` ÷ 23 000 blocks per unit on land, ÷ 46 000 at sea (a longer shelf and slope than the land side's rise: a distance field cannot run nearly level along a coast as the noise did), plus coastal noise (6.1 / 2.3 / 0.85 km, ±0.1, a third of that far from any coast so it cannot open lakes of sea inland); then the same 42% calibration |
| Where ranges may rise | A 13 km noise gate | The belt envelope (`Boundary::belt`): full across the inner half of a belt's 3.5 km half-width, falling to nothing at its edge; an arc's belt set back 0.35 of that onto the overriding plate |
| How high | Ridged field above its floor, × gate | The same ridged field, given a **spine** (`BELT_SPINE`: raised toward full by half the belt's strength squared), × belt × `belt_gain`. Strong belts become one continuous cordillera with passes; weak ones stay as separate worn massifs |
| Trenches | A ridge noise × a gate | On the plate going under, 2 200 blocks off the boundary, ±800 wide, 340 deep at most, varying along strike (12 km wavelength, never under 40%) |
| New: ocean ridges | — | +100 at the line falling as √distance (the cooling curve) over 3.5 km, with a 60-deep, ±300 axial valley |
| New: rifts | — | A floor ±380 dropping 30–110 (by the along-strike noise), walls out to ±800, shoulders +60 fading over the rest of the reach; the valley wanders ±900 from its line |

**Strengths, recalibrated** (task 1's numbers gave summits that were too
low once squared through `uplift`): collision 1.0, continental arc 0.9,
island arc 0.6; sutures Orean 0.8, Rhexian 0.6, Thalassian 0.55, Ferrian
0.35, Eoan 0.3; seams 0.3 / 0.15 / 0.1. **Calibration** (§5) became a
floor rather than a match: `MIN_RANGE_SHARE` 5% of land under ranges of
400+ uplift (version 8 measured 5.0–7.6% at the default size), reached by
a per-world `belt_gain` up to 3; most worlds need none. Continental share
is now 0.41 (near the 42% land target, so the coast calibration moves
shores hundreds of blocks, not a continent).

**Not built:** foreland sags (they are a province, task 3); hotspot
volcanoes and crustal thickness (milestone 17).

**Cost.** One framework lookup per column would have doubled terrain
height (task 1's finding). Built instead:

- **Seeds kept between columns** (`tectonics::Kept`): a search keeps the
  seeds that can matter within 512 blocks of it, with the boundaries
  between them; a column nearby is answered from those alone. Proven
  bit-exact against a fresh search over every world shape
  (`remembering_changes_nothing`, and the pinned fingerprint).
- **The warp baked into one table** at 8 nodes per terrane cell (four
  reads per column, not three octaves of interpolation).

Measured in the cloud container, both versions back to back:

| | v8 | v9 |
|---|---|---|
| Generating a column's surface chunks (cold) | 1.01 ms | 0.89–1.06 ms |
| LOD node, stride 2 / 16 / 128 | 2.1–2.3 / 6.9–8.2 / 8.5–9.2 ms | 2.1 / 8.0 / 9.5–11.1 ms |
| Height, chunk columns / LOD grid | 127–145 / 130–145 ns | 169–199 / 189–208 ns |
| Height, columns scattered across the world (tools only) | 113–157 ns | 410–500 ns |
| World creation (climate + drainage) | 5.9–6.6 s | 6.6–7.4 s |

Chunk generation is unchanged within noise; the far LOD level is up to
~20%; world creation ~10–15%. Scattered single columns cost ~3× — only
tools and tests ask for those.

**Statistics against version 8** (`terrain_stats`, 6 seeds, default
size):

| | v8 | v9 | |
|---|---|---|---|
| Land | 41.5–41.7% | 41.5–41.7% | held |
| Land under ranges (400+) | 5.0–7.6% | 5.1–8.5% | held |
| Heights: median / 99th / highest | 73–75 / 994–1 096 / 1 335–1 362 | 76–81 / 875–1 022 / 1 240–1 391 | held |
| Near-flat land | 83–86% | 81–85% | held |
| Elevation spread of a 3 km / 10 km walk | 77–85 / 143–166 | 89–104 / 223–338 | more varied (the guard is one-sided) |
| Width of a range's core (200+) on a straight line | 3.6–4.6 km | 2.9–3.7 km | ~20% narrower |
| Land between ranges on a straight line | 39–46 km | 23–38 km | ranges met more often |
| Landmass crossed by a straight line, mean | 15–21.5 km | 10.9–13.5 km | **~30% smaller** |
| Shore to water 130 deep | 3.2–8.0 km | 0.9–1.4 km | **4–6× narrower shelves** |
| World in trenches | 0.08–0.24% | 0.45–0.82% | a trench off every subduction zone |

And M13–M15, re-run: every biome in every legal world size (passes);
closed basins **29% → 22%** mean (Earth ~18%; §4 predicted the fall);
lakes 13–20% (14–20%); hot desert 3.1–7.7% of land (4.7–9.5%), dry land
26–32% (28–39%) — a wetter world, from smaller landmasses; dune seas
13–24% of hot desert (20–24%); no process misplaced. Larger continents
per plate (`CONTINENT_OF_PLATE` up to 0.75–1.0) were tried and barely
moved landmass size while costing passive margins: crooked coasts, not
continent area, shorten the chords.

**Found by the moved terrain in older code, and fixed** (each by a test
that failed on version 9):

- A braided river's dry bar stood at its own river's level beside a
  higher reach's water, which hung a block over it
  (`water_is_never_held_by_air`): a bar now stands as high as any
  reach's water beside it.
- A floodplain flat was cut a block below a lake beside it, letting the
  lake out (`lake_shores_hold_their_water`): flats are never cut below
  a lake within a block.
- The LOD put sea ice on a polar-sea column whose ground stood at sea
  level, which the chunks leave bare
  (`lod_surface_matches_full_resolution_in_every_biome`).
- **Levees beside steep torrents.** A bank built to hold a waterfall's
  lip fell away at 45°; on the spines' torrents, steeper than 45°, it
  fell no faster than the river below and ran beside it as a wall 15
  blocks high, raised over ground that had stood lower
  (`rivers_run_in_valleys_not_slots`: 552 slots → 339 of 90 394 banks).
  A levee now falls at least twice its river's slope.

**Tests that changed, and why:** two M14 tests looked at the owner's own
VIEWs, places version 9 moved; they now look at the default world's
largest rivers and at every column 3 km inside a continent. The second
also dropped "ground below sea level is never raised" — true of that
one basin, false in general (M14 fills hollows to grade, and a rift is a
hollow); the causeway it guarded is `rivers_end_at_still_water...` and
`rivers_build_no_walls`. The slot test no longer counts a bank holding
water above a waterfall within 3 blocks (it should). The erosion
gradient bound rose 1.0 → 1.25: it measures the cut, not the ground, and
the wall rule that keeps valleys walkable did not change.

**For the owner's flight:**

- **Shelves are narrow** — deep water within ~1 km of most coasts. That
  is Earth's scale (an 80 km shelf ÷ 100), but version 8's broad
  shallows are gone. The geologist's answer is wide shelves on passive
  margins and narrow ones on active margins (`docs/geology.md` §4); it
  needs `COAST_REACH` raised and is proposed for task 3.
- **Landmasses are smaller** and the world wetter. Plate size
  (`Params::plate_blocks`) is the knob; a per-world option by design.
- **Enclosed basins are rhombic.** A terrane ringed by belts is a closed
  basin, and filled to its spill level it is a flat plain against four
  nearly straight range fronts — the Tarim and Qaidam basins are exactly
  this. Say if it reads as artificial at ground level.

