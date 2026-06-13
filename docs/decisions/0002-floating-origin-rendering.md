# ADR-0002: Floating-origin (camera-relative) rendering for unbounded worlds

- **Status:** Accepted
- **Date:** 2026-06-13
- **Supersedes:** the deferred f32-precision note in ADR-0001 / Milestone 01
  (criterion 8) — that note is now resolved by this decision.

## Context

Milestone 02 makes the world effectively unbounded (streaming chunks in/out
around the camera with no fixed extent). Through Milestone 01, chunk vertex
positions baked **absolute world coordinates** into `f32` at mesh time
(vox-app's `offset_mesh` added the chunk origin to each local vertex).

`f32` has ~7 significant decimal digits, and its precision is *relative*: the
absolute gap between representable values grows with distance from zero.
Approximate behavior with 1 m voxels:

| Distance from origin | Smallest representable step |
|---------------------:|----------------------------:|
| ~16,000 blocks       | clean (sub-mm)              |
| ~50,000–100,000      | ~0.008 blocks — jitter begins |
| ~1,000,000           | ~0.06 blocks — visible      |
| a few million        | >0.1 blocks — ugly          |

Symptoms as the camera travels out: vertex *shimmer/jitter* first, then
*z-fighting*, then *cracks* between chunks whose shared edges round
differently. Not a crash — a progressive visual degradation.

The project's stated top-level value is "build it right from the start, no
refactor in two years." The user has explicitly chosen **true
unboundedness** over a bounded world. Coordinate representation is a
foundational assumption that calcifies once save/load, entities, and physics
depend on it; retrofitting a floating origin *after* those exist is
materially harder than building it in now, alongside the streaming system
that is its natural partner.

## Decision

Render **camera-relative**: GPU-side vertex positions are expressed relative
to a *render origin* that tracks the camera, never relative to absolute world
zero. The numbers fed to `f32` therefore stay small regardless of how far the
player has traveled, so precision never degrades.

Concrete rules:

1. **World/simulation coordinates stay `i64` (absolute).** `WorldPos`,
   `ChunkPos`, block storage, worldgen, save/load — all unchanged, all
   exact. Floating origin is a *rendering* concern only. Game logic must
   never see camera-relative coordinates.
2. **A `render_origin: ChunkPos` (or block-aligned `i64` position) is chosen
   near the camera** and updated as the camera moves. To avoid re-meshing on
   every move, it is **chunk-granular and updated in steps** (e.g. snap to
   the camera's current chunk, or only when the camera crosses some chunk
   threshold), not continuously.
3. **Vertices are stored relative to their own chunk's origin** (i.e. local
   0..32 space, as the mesher already emits — vox-app's absolute
   `offset_mesh` is removed). The chunk's offset *from the render origin* is
   supplied to the GPU per draw as a **per-chunk translation** (push
   constant or per-draw uniform), computed in `i64` then narrowed to `f32`
   while still a small number.
4. **The view matrix is built with the camera at the render origin**, so the
   camera-relative translation and the view transform agree.
5. Because per-chunk offsets are now small `f32` values derived from exact
   `i64` differences, there is no accumulated error: a chunk 10 million
   blocks out, viewed from 20 blocks away, still renders with the precision
   of "20 blocks," not "10 million."

## Consequences

- Removes the `offset_mesh` step; meshes are uploaded in local space and
  positioned at draw time. Slightly *less* per-vertex work, and meshes become
  position-independent (a nice property for future instancing/relocation).
- Adds a small per-draw cost: one translation vector per chunk (push constant
  preferred where supported by wgpu/the backend; otherwise a dynamic-offset
  uniform). Negligible relative to the draw itself.
- The renderer must know each chunk's `ChunkPos` (already its map key) to
  compute the offset; no new bookkeeping.
- The shader gains a `chunk_offset` input added to position before the
  view-projection multiply.
- Depth precision far from origin is also improved as a side effect, since
  view-space Z is now camera-relative and small.
- Game logic, collision, save/load remain in exact `i64`/`WorldPos` space.
  This separation (exact simulation, relative rendering) is the standard
  approach for large-world engines and must be preserved: **never leak the
  render origin into simulation.**
- Open sub-decisions left to the Milestone 02 spec/implementation: exact
  render-origin update policy (per-chunk-crossing threshold) and the wgpu
  mechanism for the per-chunk offset (push constant vs. dynamic uniform).
  Both are implementation details under this ADR, not new decisions.
