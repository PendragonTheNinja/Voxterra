# Skylight sub-surface leak — CONFIRMED root cause + fix plan

Status: **root cause confirmed via live SRCPROBE instrumentation** (not guessed).
Ready for clean implementation next session. The engine's isolated behavior is
correct; the bug is a model flaw in how the *surface chunk* seeds daylight.

## Symptom
Sub-surface caves/shafts/rooms are lit (sky ~3-14) when they should be dark.
Visually: a horizontal bright/dark seam at a chunk boundary — bright above,
dark below. Breaking blocks and forcing full relight (R key) does NOT fix it.

## How it was found
A SRCPROBE in `break_block` logged, for a broken cell: the sky value arriving
from each of the 6 neighbor chunk faces, the heightmap `top_sky[col]`, and the
engine's freshly computed sky. Key live data (chunk cy=0 spans world Y 0-31,
cy=-1 spans -32..-1, terrain surface h≈11-19):

    cell          chunk  h    top_sky  computed  neighbor planes
    (-28,4,12)    cy=0   11   0        3         +Y=15 (rest 0)
    (-3,3,-12)    cy=0   18   0        7         +Y=15, -Y=9
    (-3,-1,-14)   cy=-1  19   0        9         ALL ZERO
    (-27,-9,13)   cy=-1  12   0        0         ALL ZERO (deeper = dark)

## Confirmed root cause
The **all-air chunk above the surface** (e.g. cy=1) is fully lit to 15 and its
bottom (-Y) face plane is 15 everywhere. The surface chunk (cy=0) receives that
as its `+Y` overhead plane (`sky[2]`) and `compute_padded_sky` seeds it across
the **entire flat top face** at 15. That daylight then floods DOWN inside the
surface chunk and, because terrain surfaces are uneven (neighbor columns at
different heights) and the player has carved connected air, it reaches
sub-surface cells and spreads (attenuating 1/block), then bleeds across the
seam into cy=-1. Deeper cells eventually reach 0, producing the bright-above/
dark-below seam.

The earlier `top_sky` fix (inject 15 at the chunk top face only when the column
is open through the whole chunk, `h < origin.y`) was correct but INSUFFICIENT:
it stopped the heightmap from injecting at the ceiling, but the **redundant
overhead neighbor plane (`sky[2]`) still blanket-seeds 15 at the flat top
face**, which is the same ceiling injection by another route.

Why isolated tests passed: single/two-chunk tests used a flat full-chunk
surface or hand-fed a *correct* (already-capped) overhead plane. They never
reproduced "all-air lit chunk above + uneven real terrain surface inside the
chunk below + connected carved air." The bug lives in that multi-chunk + real-
terrain interaction.

## The correct fix (next session)
Seed daylight at the **true per-column surface height**, not at the flat chunk
top face. Concretely:

1. `compute_padded_sky` (and `compute_chunk_light_2ch`) must know each column's
   highest solid LOCAL y *within this chunk* (use `chunk_column_heights`,
   already in light.rs). For the +Y/top daylight source:
   - If the column is open through the whole chunk (no solid in it AND open
     above, i.e. heightmap says open): seed 15 at the top face as today.
   - If the column's surface is INSIDE this chunk: seed 15 only at the air
     cells AT/ABOVE that column's surface; do NOT let the flat top-face 15
     reach below the surface. Equivalent: only inject the overhead-plane /
     top_sky value into a column down to the first solid, never past it.
2. The overhead neighbor plane `sky[2]` must be applied PER COLUMN and clipped
   the same way — it represents sky coming straight down, so it must stop at
   the first solid in the column just like top_sky. The current code seeds it
   at the padded top face and relies on BFS to stop at solids, which is exactly
   what leaks when the column has air both above and below the surface within
   the chunk (carved shaft) connected via neighbors.
3. After the change, re-run the live SRCPROBE: a sub-surface cell under solid
   must show computed_sky=0 even when the chunk above is fully lit.

## Cheapest correct implementation idea
Instead of seeding the top FACE and flooding down, seed sky **directly into
each column's open-to-sky cells**: for each column, walk from the top of the
chunk down; while cells are air AND the column is open above (heightmap/overhead
says 15), set them to 15; stop at the first solid. Then run the BFS only for
horizontal/diffuse spread and the overhead-shaft (genuinely open columns). This
makes "open straight up" explicit and prevents daylight from ever appearing
below a solid cap. This matches Minecraft's skylight: full sky only in columns
with nothing solid above; everything else is propagation.

## Build a reproduction test FIRST (so the fix is verified, not guessed)
Write a vox-core test that assembles a small multi-chunk world:
- cy=1: all air (lit 15).
- cy=0: per-column terrain with VARYING surface heights (e.g. half the columns
  surface at y=20, half at y=10), plus a carved air shaft from low up to y=18
  in a column whose surface is y=20, capped by solid.
Relight cy=1 then cy=0 with real planes (the way the app does). Assert the
capped sub-surface shaft cells are sky 0 while open-above columns are 15.
This reproduces the live bug headlessly; current code should FAIL it, the fix
should PASS it.

## Pointers
- `compute_padded_sky`, `compute_chunk_light_2ch`, `chunk_column_heights`,
  `chunk_sky_plane` — all in crates/vox-core/src/light.rs.
- `top_sky_from_heightmap`, `relight_chunks_parallel` — crates/vox-app/src/main.rs.
- Remove the SRCPROBE block in `break_block` (crates/vox-app/src/main.rs) before
  shipping; it's diagnostic only.
- Tests already added in light.rs (keep): seam_no_daylight_leak...,
  capped_shaft_below..., upward_tunnel..., overhead_plane..., covered_air...,
  no_light_from_nothing_subsurface. Add the multi-chunk varying-surface repro.
