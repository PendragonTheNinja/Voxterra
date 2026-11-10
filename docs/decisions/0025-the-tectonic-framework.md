# ADR-0025: The tectonic framework — terranes at walking scale, plates at world scale

- **Status:** Proposed (2026-11-10). The *decision to have one* is the
  owner's (2026-11-10, `docs/geology.md` §4, option C: "yes we do this");
  this is how. Not built. Milestone 16, tasks 1 and 2. On acceptance it
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
coordinates so that boundaries curve. Nearest and second-nearest seed
are found among the nine cells round the column: a pure function of
`(x, z)`, a few dozen operations, no grid to interpolate and so nothing
to show through as squares. Half the difference of the two distances is
the distance to the terrane's boundary, which is all a belt's profile
needs.

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
   order, outward: each is given the age of its welding, older near the
   craton, younger toward the active margin. A suture's age is the
   younger of the two terranes it joins. This is how continents grow, and
   it gives every suture a date for free.
4. **Each plate boundary's kind**, from the two plates' relative drift
   across it and the crust on each side (`docs/geology.md` §4's table):
   convergent, divergent or transform; ocean–ocean, ocean–continent or
   continent–continent; and which side goes under (ocean under continent;
   between two oceans, the older plate). Kind is constant along the
   boundary between two plates but the crust is not, so one subduction
   zone runs from island arc to continental arc along its length, as the
   Aleutians run into Alaska.
5. **Hotspots**: a few fixed points, each with a track along its plate's
   drift.

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
Per column, the framework costs one nine-cell search plus a few table
reads, in place of three noise fields it removes; measured by
`bench_`-style tests before and after, and by `--bench`.

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
