# ADR-0011: Transparency, and water

- **Status:** Accepted (M10 amendment A1). All three open decisions resolved
  below.
- **Context:** M10 generates oceans covering ~58% of the world and renders none
  of them, because the engine has no transparency at all. `BlockType` says so:
  *"All current blocks are opaque cubes; transparency is a later milestone."*
  Water is the first block that needs it, and M10's remaining work — tuning
  terrain by eye — cannot be judged against dry basins.

## Why this is bigger than "add a blue block"

Three assumptions are baked in at different layers, and each has to be
separated before water can exist.

**1. The mesher's occlusion test is `is_air`.** Not `is_solid`, not a registry
lookup — a raw comparison against block id 0. `mesh_chunk` receives only a
`layer_of` closure; it has no other knowledge of what blocks *are*. Any
non-air block therefore fully occludes its neighbours, so water would render as
a solid blue cube with an invisible interior — and, worse, would occlude the
seabed beneath it.

**2. `solid` means five different things.** The registry documents it as
"occludes neighbor faces / can be targeted by the raycast", but call sites use
it for:

| Site | Question actually being asked |
|---|---|
| `vox-app` collision AABB | does this stop the player? |
| `vox-app` raycast | can this be targeted and broken? |
| `vox-app` column height | is this the top of the terrain? |
| `vox-core::light` (many) | does this block light? |
| `vox-mesh` (via `is_air`) | does this hide the face behind it? |

Water answers these differently: it stops nobody, is not targetable, is not the
terrain top, should eventually dim light, and hides nothing. One flag cannot
carry that.

**3. Everything renders in one pass.** Correct transparency needs opaque
geometry drawn first, then transparent geometry with blending and depth-writes
disabled.

## Decision

### Split `solid` into three independent properties

```
solid    physical  — collision, raycast targeting
opaque   visual    — occludes light, culls the neighbour face behind it
renders  visual    — has geometry at all
```

|  | solid | opaque | renders |
|---|---|---|---|
| Air | no | no | **no** |
| Stone, dirt, grass | yes | yes | yes |
| **Water** | **no** | **no** | **yes** |

Every existing `is_solid` call site is then reclassified as physical or visual.
That audit is the risky part of this change, not the rendering: a site left on
the wrong predicate produces a bug that only appears in play — the player
falling through the seabed, or the ocean floor unlit — in crates the sandbox
cannot compile. **The migration must reclassify every site in the table above
explicitly, not by find-and-replace.**

`column height` deserves particular care: it must stay on `solid`, so the
terrain top under an ocean is the seabed and not the water surface. Streaming
and the LOD edit overlay both depend on that reading.

### Face-culling rule

Emit face F of block B toward neighbour N when:

- N does not render (air) → **emit**
- N is opaque → **cull**
- N renders and is not opaque → **cull if N is the same block as B, else emit**

The same-block clause is what makes an ocean cost a surface instead of a
volume: interior water-water faces vanish and only the top, the shoreline edges
and the seabed contact remain. Without it, a 260-block-deep ocean meshes every
layer.

`mesh_chunk` gains an `occludes(BlockId) -> bool` closure alongside `layer_of`,
matching the existing style — vox-mesh stays free of a registry dependency.

Ambient occlusion and the light-corner sampler must move to the same predicate.
Water casting AO onto the seabed would be visible and wrong.

### One vertex buffer, two index ranges

`MeshData` grows a boundary: opaque indices first, transparent indices after,
sharing one vertex buffer. The renderer draws `[0, opaque_end)` in the opaque
pass and `[opaque_end, len)` in the transparent pass.

The alternative — a second `MeshData` per chunk — doubles the buffers, the
bind groups and the upload paths for a feature that is one block type today.
This costs one `u32` per mesh.

### Transparent pass: depth test on, depth write off, no sorting

Drawn after all opaque geometry, alpha blended, without writing depth so
overlapping transparent surfaces do not occlude each other.

**Explicitly not sorted per triangle.** Correct alpha needs back-to-front
ordering; that is real work and it buys nothing for a single near-planar water
surface, where overlaps are rare and the blend is nearly idempotent. The
limitation is recorded here so the first person to add glass knows why their
windows look wrong through each other.

### Decision 1 (taken: **a**) — distant water

LOD is a heightfield of land, so oceans beyond the full-resolution radius will
still read as empty basins unless something covers them.

- **(a) LOD water is opaque.** Cells below sea level emit a flat top at sea
  level with a water texture, in the existing opaque LOD pass. Costs almost
  nothing; distant water stops being transparent, which fog largely hides at
  512+ blocks.
- **(b) A sea plane.** One large quad at Y=0 in the transparent pass. Elegant,
  and automatically correct wherever terrain pokes through — but it z-fights
  with full-resolution water at exactly Y=0, so it needs the same
  coverage-suppression machinery the LOD ring already has.
- **(c) LOD water is transparent.** A transparent LOD pass. Most correct, most
  work, least visible payoff through fog.

**Taken: (a)**, and the transition matters less than it appears.

Because interior faces are culled, an ocean is a SINGLE water surface — so the
blend is one constant alpha regardless of depth, and shallow water looks
identical to 260-block-deep water. (A known limitation of voxel water rather
than a bug here; depth-tinting needs per-fragment depth and belongs with the
lighting work in decision 2.) The visible difference between transparent and
opaque distant water is therefore only "you see a fraction of the seabed
colour" versus "you don't" — nearly nothing over deep water, and shallow water
is at coasts, where the player is normally inside the full-resolution radius
anyway.

Mitigation, cheap: give distant water a colour already blended toward a typical
seabed, so the two match rather than step. If a band still shows in play, (c)
becomes a contained upgrade, because the transparent pipeline will exist by
then.

### Decision 2 (taken: defer) — does water dim light?

Real water attenuates skylight with depth, which is what makes deep ocean read
as deep. Doing it means teaching the lighting flood-fill about a per-block
attenuation cost rather than a binary blocker — an M04 change.

**Taken: deferred past M10.** Water passes light undimmed; the seabed is lit as
if dry. It looks flat but not broken, and attenuation is a lighting feature that
should land with a lighting pass rather than be smuggled in here — the
flood-fill would have to learn per-block attenuation costs instead of a binary
blocker, which is an ADR-0004 change.

### Decision 3 (taken: **a**) — what happens when the player walks into the ocean?

Water is `solid: false`, so the player falls through it to the seabed.

- **(a) Leave it.** Swimming is gameplay and out of M10's scope.
- **(b) Treat water as ground for collision.** Player walks on the surface.
  Wrong, but keeps survival mode usable.
- **(c) Minimal buoyancy** — slowed fall, slowed movement, no drowning.

**Taken: (a).** Falling to the seabed is survivable in spectator and honest
about what exists; faking a walkable surface would hide the gap and then have to
be unpicked. Buoyancy (c) is the natural follow-up once swimming is wanted.

## Consequences

- Four crates change together and cannot be split: `vox-core` (registry,
  lighting predicates), `vox-mesh` (culling, AO, index split), `vox-render`
  (transparent pipeline and pass), `vox-worldgen` (fill below sea level).
- **Only `vox-core` and `vox-mesh` are testable in the sandbox.** The
  reclassification audit and the render pass are review-only, so the migration
  table above is the substitute for a compiler.
- Chunks holding an ocean surface gain geometry that used to be air. Water
  meshes as a surface, not a volume, so the cost is bounded — but coastal chunks
  will be denser than they are today, and the M10 performance numbers should be
  taken after this lands, not before.
- Glass, ice and leaves all become possible; none are in scope here.
