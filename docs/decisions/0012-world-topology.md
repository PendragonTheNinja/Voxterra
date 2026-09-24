# ADR-0012: World topology — a torus, with looped latitude

- **Status:** Accepted (M10 amendment A2). All decisions resolved.
- **Supersedes:** M10 criterion 1 (bounded box) and criterion 2 (latitude as a
  line with poles at the Z edges). The ocean-border idea raised during the
  horizon discussion is dropped.

## Context

Two things forced the question.

**A long horizon exposes the edge of a bounded world.** With the 32–64 km view
distance the Horizon milestone targets, the boundary of a 200 km box is visible
from 54–87% of the map. The candidate fixes were an ocean border (hide the edge
behind empty sea) or wrapping (have no edge).

**The owner wants to walk around the world and arrive back where he started**,
in any direction, and eventually to let players choose the world's size at
creation — smaller or much larger than the default.

Boundless and Eco both ship edge-less voxel worlds, and both do it the same way:
the flat grid wraps on both axes, making a torus, and a curvature shader makes
it read as a sphere. Neither is a true sphere, and neither player base notices.

## Decision

### 1. The world is a torus

Both horizontal axes wrap. X and Z are each periodic; Y is not. There is no edge
anywhere, and walking in any cardinal direction eventually returns the player to
their starting point.

A cylinder (X wraps, Z bounded by the poles) was the first proposal and was
rejected: it leaves a hard wall at each pole that has to be disguised, and the
torus removes it for almost no additional cost — once horizontal distance goes
through one wrap-aware function, the second axis is the same code.

### 2. Latitude runs in a loop, and is equal-area

Latitude is periodic in Z. One lap north passes, in order:

> equator → north pole → equator → south pole → equator

That is exactly the sequence of climates a traveller crosses walking a great
circle through both poles of a real sphere. The only difference is that the
real traveller comes down the far side at the opposite longitude, and the torus
brings them down at the same one — which nobody can perceive.

The mapping is **equal-area**: `sin(latitude)` is a triangle wave in Z, rather
than latitude itself. A flat map with latitude laid out evenly over-represents
the poles badly:

| | Earth | Even layout | Equal-area |
|---|---|---|---|
| Polar (beyond ±66.6°) | 8.2% | 26% | **8.2%** |
| Tropical (within ±23.4°) | 39.8% | 26% | **39.8%** |

Equal-area reproduces Earth's proportions exactly. It is a property of the
latitude mapping alone, so it is correct at any world size.

**Remaining distortion, stated plainly:** walking east near a pole takes as long
as walking east at the equator, where on a sphere a player would circle the pole
in a few steps. Every flat-map topology has this, the rejected cylinder
included. Climate gives the polar bands ice and little else, so it reads as a
vast ice sheet rather than a wrong answer.

### 3. Rules that make variable world sizes work

World size will eventually be chosen at creation. These hold for every size and
live in `vox_core::planet`, so nothing else has to know them:

1. **Noise tiles at any period.** Each noise octave's lattice cell is adjusted to
   `period / round(period / target_wavelength)`, so a whole number of cells fits
   around the world. At 204.8 km the adjustment is a few percent and invisible;
   it works identically at 50 km or 2 000 km.
2. **Sizes are quantised** to **8 192 blocks** (256 chunks). The default world
   becomes **204 800 blocks** (25 quanta) rather than 200 000. *Revised:* this
   was first justified as keeping LOD nodes from straddling the seam at a
   misaligned offset, which section 4's unwrapped frame made irrelevant. The
   hard requirement is only whole chunks, for the save layer; the coarser step
   is kept to give size choices a sensible menu.
3. **The view distance is less than half the period.** Otherwise the same
   terrain is visible twice, around the world in both directions. Irrelevant at
   the default size; for small worlds the horizon clamps.
4. **Size and generator version are recorded in `world.meta`.** A world is
   meaningless without its period, and the audit already requires the generator
   version there.

### 4. The seam exists only where content is addressed

*Revised during implementation.* The first version of this section made every
system seam-aware: positions stored only in canonical form, every horizontal
difference routed through a wrap-aware `delta`, and a seam-straddling test for
streaming, LOD rings, suppression, the renderer, physics, the raycast, spawn and
saves. Nine systems, most of them in crates the sandbox cannot compile — and
seam bugs are the classic failure of wrapped worlds.

The seam does not need to be where positions are *compared*. It only needs to
be where the world's *content* is *looked up*. So:

- **The player's frame never wraps.** The camera, the player and every loaded
  chunk live in unwrapped coordinates: walk east past the far side of the
  world and x keeps counting, 204 800, 204 801, … Streaming, LOD rings,
  suppression, rendering, physics and the raycast subtract positions directly
  and never meet a discontinuity. **None of them changed.**
- **The world's content is periodic.** Exactly three things map an unwrapped
  position to a canonical one, and they are the only places the seam exists:
  1. **Terrain generation** — noise tiles with the world's period, so x and
     x + size generate identical ground.
  2. **The save layer** — chunk files are keyed canonically, so an edit made on
     one lap is found on every other.
  3. **The LOD edit overlay** — keyed canonically for the same reason.
- **Each has a seam test** that deliberately straddles it, and each lives in a
  crate the sandbox can build and test.
- **Rule 3 is what makes this sound.** Because the view never reaches halfway
  round the world, the same place can never be loaded under two unwrapped
  positions at once, so the unwrapped frame never contradicts itself.

`planet::delta_x` / `delta_z` still exist, for the case they genuinely serve:
comparing positions that may be on *different* laps — a saved home marker
against the player's current position. Positions in the loaded world share a
lap and are subtracted directly.

**Do not canonicalise positions anywhere else.** It is the one way to put the
seam back into code that is currently free of it. Spawn is the worked example:
the search returns the unwrapped position it found near its start, and only
display and saving canonicalise it.

### 5. Visual curvature uses Earth's radius, not the torus's

A radius geometrically consistent with a 204.8 km circumference is ~32.6 km,
which puts the ground-level horizon at ~330 blocks — an unmistakable
tiny-planet look. The curvature shader (Horizon milestone) uses Earth's radius
instead, giving real-life horizons: ~5 km from standing height, ~80 km from a
500-block hill, and mountains visible above the horizon from over 100 km.

The world is therefore secretly shorter than its curvature implies. Walking
around it takes about ten hours at sprint; no player will measure the
discrepancy, and the look matters more than the consistency.

### Decision (taken: **a**) — how wide are the climate bands?

On the bounded plan, Z spanned pole to pole once. On a torus, one lap of Z spans
pole to pole *and back*, so a square world has climate bands half as wide as
originally planned:

- **(a) Square world, narrower bands.** 204.8 km each way; equator to pole is
  51.2 km. Equator to the Tropic is roughly an hour at sprint. Matches the
  walking-time philosophy of ADR-0010 — more climates per journey.
- **(b) Original band width.** Z period doubles to ~409.6 km. Equator to pole
  stays ~100 km, but the world becomes a rectangle, which was rejected during M10
  scoping.

**Taken: (a).** Everything else in the world is now tuned to walking time
rather than Earth's dimensions, and climate belts are no exception. All walking
times in this ADR are ground sprint (5.612 m/s), never spectator flight.

## Found during implementation

**One noise cell around the world is no noise at all.** The continent octave's
target wavelength (56 km) exceeds the smallest legal world, so rule 1 rounded it
to a single cell — one value repeated everywhere — and that world came out as
one unbroken ocean. Every octave now has at least **three** cells around the
world, the fewest that still produce highs and lows on every axis.

**Land fraction was an accident of the seed.** Testing more than one world
exposed it: across 200 seeds at the default size, land ran from 15% to 91%, and
30% of seeds were over 60% land. The 42% chosen during M10 had been tuned on a
single seed. Each world now **calibrates its own coastline** at construction —
sampling its continent field and shifting it so the target share is land. All
200 seeds now land at 41–42%, and small worlds at 40–44%. It costs about a
millisecond, once per world, and is the knob a future "more land / more ocean"
creation option would turn.

**Tiled noise must not be tiled in floating point.** The pre-torus field divided
by wavelengths that were compile-time constants, which the compiler turns into a
cheap multiply-and-shift. Tiling divides by per-world values, and doing that in
floating point measured ~55% slower for the whole field. The lattice is computed
in 32.32 fixed point instead — one integer multiply per axis per octave — and
the field ends up **~38% faster** than before the torus (≈176 ns per column
against ≈285, whole-world sample, same machine).

**Terrain output is now pinned to a version.** `vox_worldgen::GENERATOR_VERSION`
is recorded in `world.meta`, and a test fingerprints terrain at fixed points, so
any change to terrain output fails until the version is bumped and re-pinned in
the same commit. It doubles as a determinism check across machines.

## Consequences

- `planet.rs` changes from bounds to a `WorldShape` value with periods; the
  bounds functions become vertical-only (`in_vertical_bounds`,
  `chunk_in_vertical_bounds`), and latitude becomes the looped equal-area
  mapping, queried through the shape.
- **Three systems change: generation, saves, the edit overlay.** Streaming, LOD,
  suppression, rendering, physics and the raycast do not (section 4).
- `world.meta` moves to version 2, recording the world's size and generator
  version. Version-1 worlds are refused with an explanation rather than opened
  with mismatched terrain.
- The `f64` world-position rework from the audit is still needed and becomes
  more so: the unwrapped frame lets coordinates grow with every lap.
- The Horizon milestone's per-level ring centring needs no seam handling of its
  own, provided it stays in the unwrapped frame — and it must, per section 4.
- The spawn search stops at half the world instead of the world's edge, and
  returns unwrapped positions.
- The world has no pole *points* — polar regions are full-width bands. Climate
  (M12) makes them ice, and that is where the ends of the world used to be.
