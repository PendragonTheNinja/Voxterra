# ADR-0009: LOD geomorph — per-vertex height morphing in the vertex shader

- **Status:** Accepted (M09 amendment A4). Both decision points below are
  resolved; see "Decisions taken". Implemented in `vox_mesh::LodVertex` /
  `mesh_lod_heightfield`, `vox-render/src/lod.wgsl`, and the `lod_morph_band`
  setting.
- **Context:** M09 amendment A4 commits to "lerp heights across a band at ring
  boundaries so level swaps are invisible." A2 (fog) and A1 (heightfield LOD)
  landed first and made the falloff far less obvious, but two artifacts
  survive that fog cannot hide, because both are *motion*, and motion reads
  through fog:
  1. **The pop.** When the camera crosses a snapped-centre boundary, a whole
     region hands over from level *N* to level *N+1* in one frame. Its
     silhouette changes instantly.
  2. **The seam.** At a ring boundary, stride-2 terrain abuts stride-4
     terrain. The exact partition (ADR-0008, M09 task 2) guarantees no gap and
     no overlap, but it does not make the two silhouettes *agree*.

## The enabling property

The strides nest 2:1 and every level takes the **minimum** real surface over
its cell (`lod_heightfield_never_exceeds_real_terrain`). Minimum is
associative, so:

> A level-*N+1* cell's height is exactly the minimum of the four level-*N*
> cells inside it.

That is the whole trick. A level-*N* node can compute, locally and with no
extra sampling, no neighbour data and no second generation pass, precisely
what the next coarser level will show for the same ground: group the 32×32
cells into 16×16 blocks of 2×2 and take the min. Because 32 is even and node
origins are multiples of `32 × stride`, the 2×2 grouping is exactly aligned
with the coarser grid — no half-covered cells, same nesting guarantee the ring
partition already depends on.

## Decision

Standard CDLOD-style morphing, adapted to a heightfield.

**1. Each LOD vertex carries a second Y: the height it would have at the next
coarser level.** Computed in `mesh_lod_heightfield` as the 2×2 minimum above.

**2. The vertex shader lerps between them by a per-vertex morph factor** `t`
derived from distance, so the geometry is fully morphed to the coarser
silhouette *before* the handover happens. At `t = 1` a 2×2 group is coplanar,
the walls between its members collapse to zero height (degenerate, no
fragments), and the group is geometrically identical to one coarse cell. The
pop and the seam both disappear: the level swap replaces geometry with
geometry that already matches.

**3. Day/night discipline (M07) applies unchanged.** `t` moves continuously as
the player walks, so it is a **per-frame uniform input, never baked into the
mesh**. Moving must re-mesh nothing. This is the same rule that keeps the sun
out of the vertex data, for the same reason.

**4. The morph band is a live slider** (A3), because the right width is a
judgement call: too narrow and the morph itself becomes visible as a ripple,
too wide and near terrain is needlessly flattened.

### Decision 1 (taken: **c**) — where the second Y lives

- **(a) Reuse the `block` light slot.** LOD is skylight-only surface — the
  spec's non-goal is explicit ("No block light / caves at distance") — so
  `Vertex::block` is hard-coded `0.0` for every LOD vertex today. The LOD
  pipeline is already separate (depth-biased), so it can bind a shader that
  reads location(4) as `morph_y`. **Zero bytes added, no new vertex layout,
  no new pipeline.**
  - *Cost:* an aliased field. `Vertex::block` would mean two different things
    depending on which pipeline consumes it. If block light at distance is
    ever wanted, this must be undone first. The non-goal makes that unlikely
    within the LOD design, but it is a real constraint being written down.
- **(b) Add `Vertex::morph_y: f32`.** Honest and obvious.
  - *Cost:* +4 bytes on **every** vertex in the world, including the
    full-resolution chunks that never morph. Full-res vertices dominate by
    volume, so this is the option that shows up in the GPU-bytes telemetry.
- **(c) A separate `LodVertex` type + vertex layout.** Correct in principle.
  - *Cost:* a parallel vertex struct, a second buffer layout, and
    `emit_rect`/`MeshData` either duplicated or made generic — for one extra
    float. Against "avoid premature abstraction; three concrete uses before
    generalizing."

**Taken: (c)**, a dedicated `LodVertex`, on the owner's standing instruction
to build for longevity rather than expedience. The first draft of this ADR
recommended (a); that recommendation was wrong on its own terms, because it
weighed only implementation cost.

The measurement that settles it: LOD needs no block-light channel and full-res
needs no morph target, so **trading one field for the other keeps `LodVertex`
at the same 32 bytes as `Vertex`**. Option (a) therefore buys nothing in
memory — its only advantage was avoiding a struct — while permanently
committing `Vertex::block` to meaning two different things depending on which
pipeline reads it. Option (b) is the one that actually costs bytes, and it
costs them on the full-resolution vertices that dominate by volume.

Two further properties of (c) that matter over the project's lifetime:

- **It fails loudly.** The mesher and the shader must land together either
  way. Under (a) a partial application would silently feed morph heights into
  the block-light channel and render the horizon fully lit; under (c) it does
  not compile.
- **It has room to grow.** Distant terrain will eventually want things near
  terrain does not — biome tint indices when real worldgen lands, normals if
  distant lighting stops being flat-shaded. Those go in `LodVertex` without
  touching the full-resolution format.

### Decision 2 (taken: **a**, then REVERSED to **b**) — what distance drives `t`

The ring's annuli are measured as **squares around a snapped centre** (the
camera's chunk floored to the coarsest stride, up to 8 chunks away from the
camera itself). So:

- **(a) Chebyshev distance from the snapped centre** — `max(|dx|, |dz|)`.
  Matches `in_annulus` exactly, so `t` reaches 1.0 at precisely the radius
  where the handover happens. Costs two floats in the sky uniform (the snapped
  centre, updated per frame) and makes the morph band visibly *square*.
- **(b) Euclidean distance from the camera** — the usual CDLOD choice, and
  free (`sky.cam_scale.xyz` is already in the shader for fog). The morph band
  is a circle, which looks more natural, but it will not line up exactly with
  a square boundary measured from a different origin: at the corners the morph
  can still be short of 1.0 when the swap fires, leaving a residual pop.

**Taken (a) first, and it was wrong. Now (b), Chebyshev from the camera.**

The argument for (a) was that measuring from the same origin as the annuli
makes `t` reach 1.0 exactly where the handover fires. That is true and
irrelevant, because it ignores what the snapped centre does over TIME.

The snapped centre is frozen between ring updates. It holds perfectly still
while the player walks, then teleports a whole coarsest-stride — 256 blocks —
at the very instant the handover happens. So `t` never animated: it was a step
function firing simultaneously with the swap it existed to hide. In play this
looked exactly like no geomorph at all, at every band width including maximum,
which is how it was caught.

The lesson generalises past this ADR: **a reference frame that is exact but
quantised is useless for smoothing, because smoothing is a property of time,
not of geometry.** Precision at the instant of the swap is worth nothing if
nothing moves in between.

(b) gives up that precision to get continuity. The camera moves every frame, so
`t` climbs smoothly. The swap can now arrive with `t` short of 1.0 — the
snapped centre sits up to 256 blocks from the camera — so the morph band has to
be wide enough to absorb that offset, which is why `lod_morph_band` defaults to
192 rather than 64.

If a residual pop survives a well-tuned band, the next option is not a third
distance metric but a different driver entirely: a per-node factor animated
over TIME when the ring retires a node, riding on the retirement machinery
below. That decouples the morph from the ring's quantisation completely, at
the cost of a per-node uniform write per frame while animating.

## Related: retirement, not deletion, on ring re-centre

Geomorph hides the *shape* change at a handover. It does nothing about the
ground being ABSENT, which is a separate defect the milestone also carried.

`LodRing::update` fires only when the snapped centre moves, and the centre
moves a whole coarsest-stride (256 blocks) at a time — so a single tick retires
~150 nodes and requests ~135 more. The unload loop dropped those meshes in the
frame it ran, while the replacements took many frames to generate and mesh. In
between there is nothing there, and the fog behind it reads as a white flash
sweeping outward from the player, worst at level 0 where the disc is largest.

**A retired node's mesh stays correct forever.** Node geometry is a function of
its own id and the terrain, never of the camera. So it is safe to keep drawing
one until its replacement lands, and safe to adopt it back unchanged if the
ring asks for it again — which also saves rebuilding identical bytes when the
camera oscillates across a boundary. Two levels overlapping briefly is
harmless: coarse never exceeds real terrain (the same contract geomorph relies
on), so the finer node wins the depth test wherever they differ, and where they
agree they are the same surface.

Retirees are flushed as soon as the pending queue drains, or after
`LOD_RETIRE_MAX_TICKS`, which bounds the stale geometry when flying fast enough
that the queue never empties. They stay in the coverage-suppression set while
they draw, or one lingering over dug ground would show a ghost through the hole
for those frames.

## Consequences

- `mesh_lod_heightfield` returns `LodMeshData` and emits morph targets, with
  five headless tests: at full morph a node whose coarse level is flat becomes
  flat (every wall collapsing to zero height), a top quad's target is exactly
  its 2×2 minimum, morphing never RAISES a vertex (the never-exceed contract
  has to survive the morph, not just the sampling), flat ground does not move,
  and targets are node-local like `position`. Verified to bite: breaking the
  2×2 grouping fails five of them.
- **`vox-mesh`, `vox-render` and `vox-app` land in the same commit.** The
  vertex format, the pipeline layout and the `set_lod_mesh` signature all
  change together.
- The LOD pipeline now binds `lod.wgsl` rather than sharing `shader.wgsl`.
  Everything downstream of lighting is duplicated verbatim between them —
  texture sampling, `light_curve`, fog — and **must stay in sync**, or the
  full-res↔LOD boundary will differ in colour. `light_curve`'s ambient floor
  and exponent now live in FOUR synced places (the two WGSL files,
  `vox_mesh::light_curve_f`, and the CLAUDE.md note).
- The full-resolution↔LOD boundary is untouched. That edge keeps the
  depth-bias overlap trick (ADR-0008); this ADR only concerns boundaries
  *between* LOD levels.
- The coarsest level has no coarser neighbour to morph toward. Its morph
  target is its own height (`t` has no effect), so the outermost ring behaves
  exactly as today.
- **The morph must never invert a quad**, and this is not free — it cost a
  round of see-through holes to learn. Each cell sinks to its OWN 2x2 group
  minimum, so the two ends of a wall sink at different rates: a neighbour that
  starts level or higher can finish well below (a face that was never emitted),
  and a wall that exists at t = 0 can have its top sink beneath its bottom
  (winding flips, back-face culling deletes it). Both read as white speckling
  across the LOD that intensifies with the band width, since a wider band means
  more ground is mid-morph at once.

  The mesher therefore emits a wall if the cells differ at EITHER end of the
  morph, and clamps each end's bottom to its own top so a wall that is
  degenerate at one end has zero height rather than negative. The invariant is
  tested directly — `the_morph_never_inverts_a_quad` asserts that for every
  quad, corner ordering in Y at t = 0 is preserved at t = 1.

  **Any future change to LOD geometry has to preserve this.** Greedy-merging
  equal-height neighbours in particular would need merged quads to agree on
  morph targets as well as heights, or it reintroduces exactly this class of
  hole.
- Greedy-merging equal-height neighbours — the "large unclaimed optimization"
  in the M09 spec — becomes harder afterwards, since merged quads would need
  matching morph targets as well as matching heights. Worth knowing before
  that optimization is attempted; it is not blocked, only constrained.
