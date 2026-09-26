# ADR-0013: Reversed-Z depth

- **Status:** Accepted (M10 A3, 2026-09-26).
- **Context:** Diagnosing the faint grid lines across distant LOD terrain.
  Relates to ADR-0002 (floating origin: precision of *positions*) and ADR-0008
  (LOD skirts); this is precision of *depth*.

## Context

This came out of diagnosing the faint grid lines across distant LOD terrain,
and it was first adopted as their fix. **It was not their fix** — see "What the
lines actually were" below. It stands on its own evidence.

The depth buffer was standard-Z `Depth32Float` (near 0, far 1, near plane 0.1).
Standard-Z maps distant depths into a sliver just below 1.0, where a float has
its coarsest spacing. Measured with the engine's own projection:

| distance | standard-Z resolves | reversed-Z resolves |
|---|---|---|
| 1 km | 0.36 blocks | 0.0001 blocks |
| 2 km | 2.7 blocks | 0.0001 blocks |
| 4 km | ~20 blocks | 0.0002 blocks |

Twenty blocks of depth ambiguity at 4 km is unusable for M11's multi-kilometre
horizon: any two surfaces that close in depth at that range — a ridge and the
slope behind it, a node and its neighbour — resolve by draw order.

## Decision

**Reversed-Z**: the projection maps the near plane to depth 1 and the far plane
to 0, the buffer clears to 0, and nearer is `Greater`. A float's precision is
densest near 0, which is now where distant geometry lands, so resolution grows
roughly linearly with distance instead of quadratically.

The convention lives in one place in `vox-render`: `perspective()` (glam's
`perspective_rh` with the planes swapped), `DEPTH_CLEAR`, `DEPTH_NEARER`,
`DEPTH_NEARER_OR_EQUAL`. vox-app builds its projection through
`vox_render::perspective`, never glam directly.

Everything else that encoded the old convention changed with it:

- **LOD depth bias** flips sign (`-16`) and loses its slope term (see below).
  Under reversed-Z a float buffer's constant bias scales with the depth's own
  exponent, so 16 units is a push of ~2e-6 of the distance. That suffices: LOD never rises
  above real terrain (ADR-0008), so the only contest is an exact tie where the
  surfaces coincide. Under standard-Z the same 16 pushed ~0.6 blocks at the
  full-resolution edge.
- **The sky pass** reconstructs each pixel's ray by unprojecting the near and
  far planes; they are now depths 1 and 0. Reading them the old way mirrors
  every ray through the camera.
- **The frustum culler** extracted its depth planes for OpenGL's `-w..w` clip
  range; wgpu's is `0..w`. That only culled too little under standard-Z, but
  under reversed-Z it drops the far plane entirely. It now uses `z >= 0` and
  `z <= w`, correct for either direction.

## What the lines actually were

The owner confirmed with `L` that the lines belong to LOD, and with the `K`
debug toggle (LOD rebuilt without border skirts) that they are the skirts: with
skirts off they vanished, and sky showed through wherever a terrace step crossed
a node border — the gap skirts exist to cover. A skirt lies hidden behind its
neighbour's top face, sharing its top edge with the seam, so a depth tie there
seemed the obvious cause. Reversed-Z removed any possible tie; the lines stayed.

The cause was the LOD pipeline's **slope-scaled** depth bias. It pushes each
triangle back by its own screen-space depth slope. A top face seen at a grazing
angle has a steep slope and was pushed back about one pixel's worth of depth; a
skirt facing the camera has almost none and barely moved. Along every node
border that brought the top face level with the skirt behind it, and the
skirt's top pixel row showed through. Interior walls never showed it because
they are never hidden — they end exactly at their neighbour's height. The slope
term is now 0: the surfaces the bias separates from full resolution are coplanar
with it, where the constant term is exact. The depth format was never involved,
which is why changing it changed nothing.

## Consequences

- The skirts stay; they still cover the slits `K` exposed.
- M11's horizon (multi-kilometre view) inherits usable depth precision. Under
  standard-Z it would have been ~80 blocks at 8 km.
- The far plane stays finite (settings-driven), so the sky's unprojection of
  depth 0 is well defined. An infinite reversed projection would also work for
  geometry, but would make that unprojection divide by zero; if one is ever
  wanted, the sky must derive its ray another way first.
- Tests in `vox-render`: the projection is reversed and monotonic; 0.01 blocks
  is resolved at 256 m–8 km; standard-Z provably loses 1 block at 4 km; the
  frustum keeps what is ahead and culls behind, beside and beyond the far plane
  (the last fails on the old plane extraction).
- The `K` toggle stays as a debug tool: it is the quickest way to see what the
  skirts are covering.
