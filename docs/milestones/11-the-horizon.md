# Milestone 11 — The Horizon

**Goal:** see as far as you would in real life. A default view of **32 km**,
engineered to run at **64 km**, with Earth's real curvature so plains fall away
past the horizon while mountains rise above it from tens of kilometres out —
and a frame rate that doesn't notice.

Depends on M10 (the torus, `f64` world positions, and the sparse LOD sampler).
Climate & Biomes (M12) comes after, and benefits: a biome you can see from
30 km away is a destination.

## Changes at M10 close (2026-10-01)

This spec was written before M10 finished, and M10 took on part of it:

- **Criterion 8 (reverse-Z) is done** — ADR-0013, in M10. What remains is to
  confirm it holds at 32 and 64 km.
- **Criterion 7 (distant water) changed shape.** ADR-0011 decision 1 moved to
  (c): LOD draws its real seabed and a translucent sea surface, not a flat
  opaque top. Flat-cell merging (criterion 6) still applies to the sea surface;
  the seabed under it merges by its own heights.
- **New task 0 — why relighting doubled.** M10 closed with criterion 8 partly
  met: sprint-fly median ~130 fps against M09's ~180, with relight at
  192–253 ms/s against M09's 95–130 and meshing unchanged (M10 retrospective).
  Before the horizon spends the same budget, measure whether each relight got
  slower or more relights happen per chunk (per-chunk timing at the
  `v0.9.0-m09` tag against now; relights per loaded chunk in telemetry), and fix
  what the measurement names. Optimize from measurements, not vibes.
- **Criterion 9 (atmosphere) is wavelength-dependent.** Distant terrain should
  turn blue-violet with distance, as real mountains do through air, not fade to
  white: per-channel extinction (blue scattered most) with in-scattering toward
  the sky colour. The owner raised this independently on 2026-10-01; see
  `docs/vision.md`.

## Status and remaining work — START HERE

*Kept current at every task boundary. A new session reads this list, verifies
the remote matches it, and starts at the first unchecked item.*

### Done

- [x] **Task 0 — why relighting doubled (measured, three fixes).** A headless
  model of `stream_tick`'s relight loop (the real generator, streamer,
  heightmap and `compute_chunk_light_2ch`, with vox-app's triggers copied)
  reproduced the sprint log: ~880 ms/s of single-thread relight work, which
  over the worker pool is the ~220 ms/s `lt`. Where it went, at 120 m/s and
  radius 8 (~470 chunks loaded per second, ~1 800 relights per second):
  - **Uniform air under open sky missed the fast path** — 985 relights/s at
    ~425 us each, ~70% of the lighting function's time. The fast path refused
    any uniform chunk whose side neighbours carried sky light, which is every
    air chunk above the ground. Where 15 enters every column from above, side
    planes cannot change a cell; an all-solid chunk likewise. Both now take
    the fast path, checked against the full path in tests (418 → 31 ms/s).
  - **`top_sky` cost ~80 us per relight**: 1 024 hash-map lookups, one per
    block column. `ColumnHeights` now stores each chunk column's heights as
    one array, and `ColumnHeights::sky_top` reads a chunk's whole footprint
    with one lookup (141 → 2.5 ms/s). Semantics unchanged, unknown included.
  - **`Streamer::update` re-sampled the terrain every frame**, standing still
    included: ~245 columns × 5 elevation samples, ~0.3 ms a frame on the main
    thread, in neither `lt` nor `msh`. It now returns early when nothing has
    changed (its own field comment already promised this), and vox-app keeps
    spans in a `SpanCache` (0.32 → ~0.001 ms per still frame; 51 → 3 ms/s in
    flight). The likeliest cause of M10's ~15% lower standing fps.

  Overall: relight work 879 → 292 ms/s in the model, with every relight's
  result identical (same count, same causes). Not all of this was new in M10 —
  the fast-path miss and the per-column map predate it — but M10 streams more
  air (the surface band's upper layers and the camera window), so it paid
  them more often. No exact M09 comparison exists: the `v0.9.0-m09` tag is an
  in-progress commit and M09's completion landed together with M10's tasks
  1–4. Still open in the model, for later: 83% of relights change nothing
  (most queued as a new chunk's six neighbours), and non-uniform chunks are
  now most of the cost. To confirm in play: the M10 retro's sprint-fly and
  standing runs at radius 8, 3 levels — compare `lt`, fps and worst frame;
  and that lighting looks unchanged (caves dark, no bright or black chunks
  while flying).

### Remaining, in order

1. **Lighting asks `solid` where CLAUDE.md requires `opaque`.** Found during
   task 0: `light.rs` (relight, `chunk_column_heights`, the fast path) still
   calls `registry.is_solid`, though the M10 checklist recorded ADR-0011's
   step 1 as converting it. Invisible today — every block has
   `solid == opaque` — but wrong the first time a block is solid and
   transparent (glass). Fix with a test whose registry has such a block.
2. **Tasks 1–6 below**, in order.

## Why this milestone exists

Measured in the September 2026 audit: from random points on land, how often is a
mountain even in sight?

| View distance | Ground above 900 blocks visible from |
|---|---|
| 2 km (M10) | 10% of land |
| 8 km | 39% |
| 16 km | 67% |

The median walk to a 900-block peak is ~11 km. The terrain already has the
mountains; the player can't see them. Magnificence is watching a range grow on
the horizon for half an hour of walking, and that same view is what keeps travel
from being a slog — you are always walking *toward* something.

## What "real life" means here

On a flat world only the atmosphere limits sight: 30–50 km on a clear day. With
**Earth's radius** (6 371 km) and one block per metre, curvature gives exactly
real horizons:

| Eye height | Horizon |
|---|---|
| Standing (1.62 blocks) | ~4.5 km |
| On a 500-block hill | ~80 km |
| A 1 150-block summit, seen from the ground | above the horizon from ~120 km |

So from the ground, plains sink away beyond a few kilometres — as in life —
while high ground stays visible from far beyond the render distance.

Curvature is **visual only**. Physics, collision, the raycast and every
distance in the simulation stay flat. ADR-0012 records why the curvature radius
deliberately does not match the torus's circumference: a consistent radius for a
204.8 km world is ~32.6 km, which would put the horizon at ~330 blocks.

## Acceptance scene

Spawn on a lowland plain. The near ground is full resolution; beyond it the land
rolls away and drops below the horizon a few kilometres out. Above that line,
far off, a mountain range stands pale-blue with distance. Walk toward it for
twenty minutes: it grows steadily, foothills lift over the horizon, and at no
point does the landscape visibly swap detail or pop.

Climb a 500-block hill and look back. The plain you crossed is laid out below,
fading to haze tens of kilometres away, with the coastline and the sea beyond
it. Frame rate stays in the M10 envelope throughout, on the machine the M10
numbers were taken on.

## Acceptance criteria

1. **Per-level ring centring.** Each LOD level snaps its own centre to its own
   node grid, instead of all levels sharing one centre snapped to the coarsest
   grid. At 64 km the coarsest cell is 8 km, and a shared centre could sit
   kilometres from the player. The exact, gapless partition between levels
   (ADR-0008) is preserved. The rings stay in the player's **unwrapped** frame
   and must not canonicalise anything — the world's seam lives only in
   generation, saves and the edit overlay (ADR-0012 §4), so a correct ring
   needs no seam handling at all. Headless: every chunk column within the
   horizon is covered by exactly one level, at many camera positions including
   ones several laps from the origin.
2. **Levels to 64 km.** Eight levels (strides 2–256 chunks), ~1 600 nodes. Node
   alignment relative to the world's seam does not matter: nodes sample
   periodic terrain in the unwrapped frame (ADR-0012 §4).
3. **View distance clamps to less than half the world's period** (ADR-0012
   rule 3), so no terrain is ever visible twice around the world.
4. **Earth-radius curvature** in both terrain shaders — `shader.wgsl` and
   `lod.wgsl` — computed identically. The two must agree exactly or the
   full-resolution/LOD boundary tears. Headless where possible: the curvature
   function is shared and tested.
5. **Horizon culling.** A LOD node lying entirely below the curvature horizon
   for the current eye height is not drawn. From the ground this should remove
   most of the far ring.
6. **Flat-cell merging** in `mesh_lod_heightfield`. Adjacent cells with equal
   heights *and equal morph targets* merge into one quad. A node of open ocean
   becomes a handful of quads instead of 1 024. The morph-target condition is
   not optional — ADR-0009 records that merging on height alone reintroduces the
   see-through holes fixed in M09.
7. **Distant water.** LOD cells below sea level draw a flat sea-level top, per
   ADR-0011 decision 1, and merge per criterion 6.
8. **Reverse-Z depth.** Standard depth loses nearly all precision past a few
   kilometres; distant ranges would z-fight. Reverse the depth range (f32 depth,
   near maps to 1). The LOD depth bias (ADR-0008) inverts sign under reverse-Z
   and must be re-derived, not just flipped.
9. **Atmosphere.** Fog is retuned for tens of kilometres: distant terrain fades
   toward sky colour so range reads as depth. Aerial perspective, not a wall.
10. **Performance.** Against M10's final numbers on the same machine: steady fps
    within the envelope, LOD backlog returning to 0 after a ring update, GPU
    memory and triangle counts reported. Report numbers at 32 km and 64 km.
11. **Bookkeeping.** ADR for per-level centring (supersedes ADR-0008's shared
    snapped centre) and one for curvature and depth. Retro with numbers;
    CLAUDE.md status.

## Tasks

1. **Per-level centring (`vox-core`, headless).** The ring redesign, in the
   unwrapped frame, with partition tests at many positions. Everything else
   depends on this, so it lands first and alone.
2. **More levels + view-distance clamp.** Extend to eight levels behind the
   existing settings slider; clamp to half the period.
3. **Flat-cell merging (`vox-mesh`, headless).** With morph-target agreement
   and the existing inversion test extended to merged quads. Includes distant
   water.
4. **Reverse-Z (`vox-render`).** Re-derive the LOD depth bias.
5. **Curvature + horizon culling.** Shared curvature function, both shaders,
   CPU-side culling.
6. **Atmosphere retune + performance pass + retro.**

## Non-goals

- **No climate, biomes or vegetation** — M12. The long view will be green; it
  is judged on distance, stability and frame rate, not colour.
- **No true sphere.** The world stays a torus with visual curvature (ADR-0012).
- **No world-size selection UI.** Sizes stay a constant; the engine just has to
  be correct for any legal size.
- **No streaming of full-resolution terrain beyond the current load radius.**
  The horizon is LOD all the way out.

## Notes for whoever builds this

- **Judge it on foot.** Spectator sprint is 21× ground sprint. The acceptance
  scene is a walk.
- **The two shaders must curve identically.** Put the curvature formula in one
  place and derive both from it. Any drift shows up as a crack exactly at the
  full-resolution boundary, which is the one place the player is always
  looking.
- **Geomorph gets smoother for free.** With per-level centres snapped to each
  level's own (fine) grid, the handover boundaries move in small steps instead
  of 256-block jumps. Re-evaluate the residual pop ADR-0009 describes once
  criterion 1 lands; the time-driven morph it proposes as a fallback may become
  unnecessary.
- **Low-end hardware is pillar 1.** 64 km must be an option that runs well, but
  the *default* should be chosen by what an ordinary machine sustains, and
  lower-end defaults need deciding before release.

## Open questions for the owner

- **Default view distance by hardware.** 32 km is the target on the development
  machine. Whether weaker hardware defaults lower, and how that is detected or
  chosen, is a release-time decision — noted here so it isn't forgotten.
