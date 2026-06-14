# ADR-0003: Texture array (not atlas) for block textures

- **Status:** Accepted
- **Date:** 2026-06-14
- **Context:** Milestone 03 task 2 (textures), see `03-...md`.

## Context

Blocks need per-face textures. The vertices are produced by the **greedy
mesher**, which merges coplanar same-block same-facing faces into maximal
W×H rectangles (one quad can cover up to 32×32 block faces). The texturing
approach has to make a merged W×H quad show its tile **repeated W×H times**,
not one tile stretched across the whole rectangle.

Options:

1. **Single texture atlas** (all tiles packed into one image, faces address
   sub-rectangles via UV). The classic approach, but it fights greedy
   meshing: a `Repeat` sampler wraps the *entire atlas*, not a single tile,
   so you can't tile one tile across a merged quad. Workarounds — splitting
   merges at unit boundaries (loses greedy's benefit), or manual UV
   wrapping in the shader with per-tile bounds and half-texel insets to stop
   bleeding — are all fiddly and bleed-prone.
2. **Texture array** (`texture_2d_array`: each tile is its own layer). A
   `Repeat` sampler wraps within a layer, independent of other layers, so a
   merged W×H quad with UVs running `0..W`/`0..H` tiles its layer correctly
   with zero bleed between tiles. Each face carries a `layer` index plus UV.
3. Compute tiling in the shader from a tile index + local face coords
   (essentially reimplementing what a texture array gives for free).

## Decision

Use a **texture array**. Each distinct block tile is one array layer; the
block registry (ADR-less, M03 task 1) assigns a layer per block face. The
mesh vertex carries `uv` (running 0..W / 0..H across a greedy quad), a
`layer` index, and a directional `brightness` scalar. The sampler uses
`Repeat` addressing and **nearest** filtering (crisp voxel pixels). No atlas
packing, no inset math, no merge-splitting.

This preserves full greedy meshing (the merge criterion stays "same block +
same face direction", which already implies same layer) and eliminates
atlas-bleed as a class of bug.

## Consequences

- **Vertex format changes** from `{ position, color }` to
  `{ position, uv, layer, brightness }`. This touches `vox_mesh::Vertex`,
  `emit_rect`, both meshers, the differential test (now compares layer
  coverage instead of color), the wgpu pipeline vertex layout, and the WGSL
  shader.
- The mesher's appearance input changes from `color_of: Fn(BlockId)->[f32;3]`
  to `layer_of: Fn(BlockId, face_index)->u32`, resolved via the registry.
  vox-mesh stays headless and registry-agnostic (it just receives a closure).
- The renderer creates a `texture_2d_array` with one layer per tile, a
  Repeat+nearest sampler, and a bind group (group 2) for them. Pipeline gains
  the texture/sampler binding; shader samples `array[layer]` at `uv`.
- Layer count is bounded by the GPU's max array layers (≥256 everywhere,
  typically 2048+) — far more than the block set needs for the foreseeable
  future. If we ever exceed it, multiple arrays or array-of-atlases is the
  escape hatch; not a concern now.
- Initial tiles may be **procedurally generated** (solid/noise per block) so
  the engine ships no image assets yet; real PNG tiles drop in later by
  replacing tile generation with image loading, no format change.
- Per-vertex `layer` (u32) and `uv` (2×f32) add 12 bytes/vertex over the old
  format's color (was 12 bytes of color → now 8 UV + 4 layer + 4 brightness
  = 16, net +4 bytes/vertex). Negligible at current mesh sizes.
