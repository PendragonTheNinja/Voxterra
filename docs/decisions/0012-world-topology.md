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
2. **Sizes are quantised** to a multiple of the largest LOD node span,
   **8 192 blocks** (256 chunks). A node can then never straddle the seam at a
   misaligned offset. The default world becomes **204 800 blocks** (25 quanta)
   rather than 200 000.
3. **The view distance is less than half the period.** Otherwise the same
   terrain is visible twice, around the world in both directions. Irrelevant at
   the default size; for small worlds the horizon clamps.
4. **Size and generator version are recorded in `world.meta`.** A world is
   meaningless without its period, and the audit already requires the generator
   version there.

### 4. Seams are made hard to get wrong

The classic failure of wrapped worlds is a system that compares or subtracts
horizontal positions without wrapping — chunks that fail to load across the
seam, LOD rings that tear, physics that teleports.

- Horizontal positions are stored **only in canonical form**, in
  `[0, period)` per axis. Nothing downstream ever sees an out-of-range X or Z.
- **Every horizontal difference goes through one function**, `planet::delta`,
  returning the shortest signed separation. There is no other sanctioned way to
  subtract two horizontal positions.
- **Every system that uses horizontal distance gets a test that straddles the
  seam**: streaming, LOD ring membership, suppression, render offsets,
  collision, raycast, spawn search, save round-trips.

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

## Consequences

- `planet.rs` changes from bounds to periods. `in_bounds_xz` and `clamp_xz`
  become `wrap_xz` and `delta`; `latitude_fraction` becomes the looped
  equal-area mapping.
- **Every horizontal-distance consumer changes** — the streamer, the LOD ring,
  coverage suppression, the renderer's floating origin, physics, the raycast,
  the spawn search, and saves. This is the substance of the work and the reason
  to do it now: after climate (M12) is built on the current model, every one
  of these is harder to change.
- The `f64` world-position rework from the audit touches most of the same code,
  so the two are done together.
- The Horizon milestone's per-level ring centring must be written seam-aware
  from the start.
- The elevation field's wavelengths are re-derived under rule 1. Measured
  landform statistics should be re-checked, but changes of a few percent in
  wavelength are within what ADR-0010's tuning tolerates.
- The spawn search no longer needs to skip out-of-bounds candidates; it wraps.
- The world has no pole *points* — polar regions are full-width bands. Climate
  (M12) makes them ice, and that is where the ends of the world used to be.
