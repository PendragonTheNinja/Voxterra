# ADR-0011: Transparency, and water

- **Status:** Accepted (M10 amendment A1) and implemented (2026-09-26), in two
  steps: the property split and mesher rule, behaviour-neutral, then water.
  See "Implementation notes" at the end for the choices this ADR did not
  cover.
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

### Decision 1 (taken: **a**, then **c** after play) — distant water

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

**Revised in play (2026-09-30): (c).** A band did show, and it could not be
masked away. The reasoning above holds for distant water seen *from above*; it
fails for the line of sight that passes through translucent NEAR water and
continues, underwater, past the edge of the loaded area. Opaque LOD water is a
lid with nothing under it, so that line ended in empty space — a pale strip
along the whole edge, as wide as the sea is deep (260 blocks over the abyssal
plain, seen from above at 45 degrees). The owner put it exactly: the near field
renders the water and the land under it, the LOD only the water's top face.
Under (c) the LOD draws its real seabed in the opaque pass and the same
translucent water surface in the transparent pass, so near and far water are
one thing at two resolutions and every line of sight through either ends on a
seabed. It also retired the pre-blended distant-water layer and the LOD's
"effective height" (ground raised to sea level): the LOD ground is ground
again. Cost: the seabed's terraces and one water quad per row of sea cells per
node — rows merge, so an open-ocean node adds 32 quads, not 1 024.

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

## Implementation notes (2026-09-26)

Choices the decisions above left open, and why each was taken.

**Sea level is the highest water block.** Water fills every block above the
ground up to and including `SEA_LEVEL_BLOCKS` (0), so the sea surface is that
block's top face and a column of height 0 is flush with the sea. Chosen so
the top water block and the air above it share one chunk layer,
`SEA_SURFACE_CHUNK_Y` — see the next note. Worldgen fills below it, with an
all-water fast path for open ocean; `GENERATOR_VERSION` went to 2, and the
terrain fingerprint now hashes chunk contents as well as surface heights,
because water changed chunks without moving a single height.

**Streaming keeps the sea surface, not the water column.** Streaming follows
the ground, which over the abyssal plain (~260 blocks down) is eight chunk
layers below the surface — out of reach of the surface window, so the ocean
would never be seen from above. Loading the water column would add ~8 layers of
uniform water per ocean column, all invisible: water-water faces are culled, so
an ocean is its surface and its seabed. `StreamConfig::sea_layer` keeps that
one layer for columns whose ground is below it; a column's window is now up to
three ranges (ground, sea surface, camera). LOD coverage counts the sea layer,
or a node over deep water would be hidden before its sea arrived.

**The seabed is lit through the gap.** With the water column absent, the
seabed chunk had nothing above it and read as dark, although only water, which
passes light (decision 2), lies between it and the sky. When a chunk's +Y
neighbour is absent AND never coming (sealed — the same judgement the mesher
uses), its sky plane is derived from the heightmap: 15 where the column's
highest opaque block is below that neighbour, 0 elsewhere, and 0 for unknown
columns, which stay covered. Not for a neighbour still coming: guessing then
would light a cave under a surface chunk that has not streamed in yet.
(`ColumnHeights::sky_plane_above`.)

**The transparent pass does not cull faces**, so the sea surface is visible
from beneath — looking up from the seabed in spectator, or after falling in
(decision 3).

**Distant water is the LOD's seabed under a translucent surface (decision 1c,
revised).** See decision 1. The LOD's sea surface is drawn by its own
pipeline in the transparent pass (`fs_lod_transparent`), with the same blend,
depth state, culling and Fresnel as near water, and discarding in full-res
columns as the LOD ground does.

**LOD is discarded per chunk column wherever full resolution draws.** The
first in-play test showed, at the edge of the loaded area over ocean, a dark
jagged band, a speckled blue band beyond it and white slivers. The cause was an
assumption translucency breaks: LOD underlaps full resolution, and whole
nodes are hidden only when every column under them is drawn, so nodes
straddling the edge stayed drawn *under* the full-res region. Opaque ground in
front had always hidden that overlap. Translucent near water does not: the
nodes' opaque sea tied in depth with the near sea surface (the speckle), and
their ground-textured skirts showed through it as a dark wall along the
suppression boundary. Depth bias cannot fix a surface that is meant to be seen
through. Now `Renderer::set_fullres_columns` uploads a mask of the chunk
columns full resolution draws — the same set node suppression uses
(`fullres_columns` in vox-app) — and two things follow from it: the LOD
fragment shader discards in those columns, and the transparent pass draws
near water ONLY in them. So LOD and full resolution never overlap in either
direction, whatever either is made of, including in a column still streaming
in (sea surface without its seabed shows LOD water, not near water over
nothing). The set spans every resident column, out to the unload radius: the
first version stopped at the load radius, and the unload band's still-drawn
water sat over unmasked LOD — the fix appeared to change nothing until that
was found. The mask
is group 3 binding 1 (the four-group default limit is already reached) and its
placement is the chunk uniform's fifth vec4, written only by
`set_fullres_columns`; `set_sky` writes the first four.

**Water turns opaque at grazing angles (Fresnel).** Looking across the sea
from low down, the line of sight passes through the near surface and on,
underwater, past the edge of the loaded area — into the empty space beneath
the LOD sea surface, which read as white slivers. Real water is nearly opaque
there anyway, showing the sky rather than what lies below, so the transparent
pass raises alpha toward 1 by Schlick's term, `(1 - |n·v|)^5`, with the face
normal taken from screen-space derivatives. Looking down it changes almost
nothing (0.002 at 45 degrees). It is also the first step toward decision 2's
deferred depth tint, not a substitute for it.
