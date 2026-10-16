# ADR-0015: Earth curvature in the terrain shaders, and horizon culling

- **Status:** Proposed (M11 task 5, 2026-10-16). Accepted once the owner has
  seen it in play.
- **Context:** M11 (The Horizon), criteria 4 and 5. ADR-0012 §5 fixed the
  radius — Earth's, not the torus's — and left the mechanism to this
  milestone. Depth for the long view is ADR-0013 (reversed-Z), already in.

## Decision

**Curvature is a vertex-shader drop, `d² / 2R`, in both terrain shaders, from
one function.** `d` is a vertex's horizontal distance from the camera, `R` is
`vox_core::CURVATURE_RADIUS_BLOCKS` (6 371 km). A point 4.5 km away sinks
1.6 blocks, 64 km away 321.

- **The parabola, not the sphere.** They differ by `d⁴/8R³`, under 0.01 block
  at 64 km; the parabola is one multiply in `f32`, the sphere a square root
  of a difference of huge numbers.
- **One function.** `curvature.wgsl` holds `curve()`, and vox-render appends
  it to `shader.wgsl` (full resolution, both passes) and `lod.wgsl` (both LOD
  passes) at shader creation. Full resolution and LOD must curve identically
  or the boundary between them cracks; with one function they cannot differ.
  A test asserts both composed shaders end with it, neither defines its own,
  and both draw through it.
- **One coefficient.** `1/2R` reaches the shaders in the shared terrain
  uniform's spare slot, `fog_range.z`, from `vox_core::curvature_coefficient`
  — the same value the CPU culling uses. Zero draws the world flat: the
  settings menu has an "Earth curvature" toggle, for comparison.
- **Visual only.** Clip position is curved; the flat position still feeds
  fog distance and the full-resolution column mask, and physics, collision,
  the raycast and every simulation distance stay flat (ADR-0012 §5). The
  block-highlight outline is not curved: within reach the drop is under
  0.00001 block.

**LOD culling judges nodes where the shader draws them.**

- **Frustum, curved.** Curvature only lowers terrain, so a node's box is
  stretched down by the drop at its farthest corner before the frustum test.
  The flat box would cull terrain that curves down into view — looking down
  from altitude, for instance.
- **Horizon.** A node is skipped when its highest point lies below the sight
  line grazing the sea-level surface (`vox_core::below_horizon`), tested at
  its nearest point. The sea-level surface is the lowest anything is drawn
  (ground below sea level is under water whose surface is at sea level), so
  real terrain only hides more: the test never culls what could show. Only
  with curvature on.

## Consequences

- From the ground, plains sink below the horizon a few kilometres out while
  high ground stays visible far beyond — M11's acceptance scene. From a
  1.62-block eye a 1 150-block summit stays above the horizon to ~120 km.
- **Horizon culling removes less than the spec hoped.** Measured at spawn
  with eight levels: 105 of 1 600 nodes from standing height (7%, all in the
  three coarsest levels), 36 from 100 blocks up, none from 1 000. Coarse
  nodes are 8 km wide, and almost every one holds ground tall enough to show
  over the horizon — which is the point of the milestone. Culling hidden
  PARTS of nodes would need per-cell work; not attempted.
- Over open ocean the sea surface writes no depth, so land beyond the horizon
  could show through it; horizon culling removes it.
- Fog is still measured on the flat distance, which differs from the curved
  one by under 0.01%.

## Tests

`vox_core::planet`: horizons from 1.62 and 500 blocks are 4.5 and 80 km; the
summit and plain cases above; `below_horizon` is conservative (eye at or below
the surface, anything nearer than the horizon) and monotonic beyond it; the
grazing line touches the curve; parabola within 0.01 block of the sphere at
64 km. `vox-render`: every shader parses and validates with naga (composed
terrain shaders included); both terrain shaders curve through the one
function; terrain curving into view is kept; low ground beyond the horizon is
culled with curvature on and kept with it off.
