# ADR-0005: Cubic-chunk skylight via per-column heightmap

- **Status:** Accepted
- **Date:** 2026-06-15
- **Context:** Milestone 05 (skylight). Builds on ADR-0004 (light storage +
  block-light flood-fill).

## Context

Skylight = ambient illumination from the open sky. A voxel exposed to the
sky (nothing solid directly above it, all the way up) gets full skylight;
skylight floods downward and sideways into shadows, caves, and under
overhangs, dimming as block light does.

In classic Minecraft this is easy: the world has a fixed, *low* height
(≈320), so you start at the top of every column and flood straight down.
Voxterra cannot do this:

- **Cubic chunks (32³), unbounded vertically.** There is no tall column
  chunk; a point's vertical neighborhood is many separate 32-tall chunks.
- **Realistic terrain height is a hard requirement.** Mountains may reach
  real-world scale (Everest ≈ 8,800 m → Y≈8800+). A "fixed ceiling" would
  have to sit above the tallest possible terrain (≈Y9000), making each column
  ≈280 chunks tall.

A fixed-ceiling flood is *correct* but pathologically expensive here: to
propagate skylight from Y9000 down to a surface at Y30 you would need the
~280 vertical chunks of (almost entirely empty) air between them to
participate — either keeping them all loaded (loaded-chunk count explodes
from ~3k to hundreds of thousands) or special-casing "empty air chunk = full
skylight, skip," which is just the heightmap approach arrived at indirectly.

## Decision

**Track the highest solid block per world column (a heightmap) and treat
every cell at or above that height as full skylight; propagate skylight
*downward and sideways* only from there.**

- A **column** is an `(x, z)` world coordinate. Its **height** is the Y of
  the highest solid (sky-occluding) block, or "none/open" if the loaded
  world has no solid block in it.
- Any cell strictly above the column height is **full skylight (15)** by
  definition — nothing is above it to block the sky. These are never
  simulated; they cost nothing.
- Skylight **propagation** runs only from the boundary at/just below the
  height downward and into shadowed neighbors, using the **same BFS
  machinery as block light** (ADR-0004), with one rule difference:
  **skylight does not dim when travelling straight down** through open air
  (a vertical shaft stays at 15 to the floor), but dims by one per step
  horizontally and (optionally) upward. (This "vertical skylight doesn't
  attenuate downward" rule is what makes daylight reach the bottom of a deep
  pit at full strength, as in Minecraft.)

This is **height-agnostic**: it works identically whether the surface is at
Y30 or Y8800, and never commits to a ceiling. It is automatically compatible
with arbitrarily tall realistic terrain.

### Storage

Skylight uses the **reserved high nibble** of the existing per-voxel light
byte (ADR-0004). No storage reshape: block light stays in the low nibble,
skylight in the high nibble. The mesher combines them as
`brightness = directional_shade × light_curve(max(sky, block))`.

### Heightmap under streaming (the hard part)

The heightmap is **per-column derived state**, like light. The difficulty:
a column spans many chunks across an unbounded Y range, and the full column
is never all loaded at once. Rules:

- The heightmap is maintained over the **currently loaded** chunks: a
  column's height is the highest solid block among loaded chunks in that
  column. It is an approximation that converges as more of the column loads.
- **When a chunk streams in above** the current known height of a column it
  covers (i.e. a taller piece of terrain appears overhead), the affected
  columns' heights rise, and skylight **below** must re-propagate (shadows
  appear). Those chunks get marked dirty for relight — reusing the existing
  dirty-set path.
- **When a chunk with no loaded chunk above it** is lit, its top face is
  treated as **open to sky** (full skylight enters from the top boundary),
  *unless* the heightmap says a loaded higher chunk in that column occludes
  it. This is the skylight analogue of block light's neighbor-border input.
- Edits (break/place a sky-occluding block) update the column height for
  that `(x,z)` and re-propagate, via the same dirty path as block-light
  edits.

The heightmap is **not serialized** (derived from blocks, recomputed on
load), consistent with light (ADR-0004) — the chunk format stays unchanged.

## Consequences

- Skylight reuses the `LightVolume` BFS and the parallel
  compute/apply/dirty-set plumbing from M04, with: a second nibble, the
  "no downward attenuation" rule, and a per-column heightmap feeding the
  top-boundary condition.
- **No ceiling, no height cap, no floor** is committed by this decision;
  vertical extent stays free. (A bedrock floor, if added later for realistic
  worldgen, is independent and does not affect this design.)
- The top-of-column "open to sky" boundary replaces block light's "missing
  neighbor = dark": for skylight, a missing/absent chunk *above* the column
  height means open sky (15), not darkness. Getting this asymmetry right is
  the main correctness risk and the focus of testing.
- Approximation while streaming: a column may briefly be over-lit if a tall
  chunk overhead hasn't loaded yet (light leaks before the shadow caster
  arrives), correcting within a frame or two when it does — analogous to,
  and bounded like, block-light cross-chunk convergence. Acceptable; note in
  telemetry.
- Combining sky+block as `max` (not sum) keeps levels in 0..=15 and matches
  player expectation (a torch in daylight doesn't exceed daylight).
- Daytime brightness now exists, so the M04 interim ambient floor (0.06)
  should be re-tuned: unlit/night surfaces vs. full-sky surfaces need a
  pleasing contrast. (Day/night *cycle* is out of scope — skylight is a
  static "full day" for now.)
