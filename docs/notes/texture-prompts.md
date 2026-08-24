# Block texture prompts

A base prompt system for generating Voxterra block textures (Flow / Nano Banana
Pro or any image model). Copy the **Base prompt**, paste the **category block**
for what you're making, fill the `[BLOCK]` slot, and generate.

The point of a fixed base is consistency: fifty textures made from fifty ad-hoc
prompts will not sit together in one world, no matter how good each one is on
its own.

---

## Art direction

**Stylised fantasy realism.** Closer to Hytale than Minecraft, and further still
from Vintage Story's muted naturalism — saturated, warm, and readable, but with
real material structure rather than flat colour blocking. The world's *systems*
are grounded in real geology; the *look* is a heightened, storybook version of
Earth.

Concretely:

- **Saturation:** rich but not neon. Colours should look like a well-lit
  illustration of the real material, not a candy version of it.
- **Detail:** every texture should have visible material structure — grain,
  crystal, fibre, weave — at a scale that reads from a few blocks away. Avoid
  noise for its own sake; avoid flat fields of one colour.
- **Contrast:** moderate. The engine's own lighting and ambient occlusion supply
  the drama; textures that arrive pre-dramatised end up muddy.
- **Palette cohesion:** every block should look like it came from the same
  world. Warm, slightly golden light bias across the whole set.

---

## Base prompt (always include)

> A seamless, tileable texture of [BLOCK] for a voxel game, in a stylised
> fantasy-realism art style: rich saturated colour, warm lighting bias, clear
> material structure, hand-crafted illustrated quality rather than a photograph.
> Flat orthographic top-down view of a flat surface, filling the frame edge to
> edge. Perfectly seamless and tileable on all four edges, so the pattern
> continues without any visible seam when repeated in a grid. Even, flat,
> ambient illumination with no directional light, no cast shadows, no highlights
> or hotspots, no vignette, and no darkening toward the edges. No perspective, no
> depth of field, no blur. No border, frame, text, watermark, or object outline.
> A single continuous material filling the entire image.

### Why each constraint is there (do not drop these)

- **Seamless / tileable** — the greedy mesher merges adjacent faces into one
  quad and *repeats* the texture across it. A texture with mismatched edges puts
  a visible grid over every large surface.
- **No baked lighting or shadows** — the engine already computes per-vertex
  smooth lighting and ambient occlusion, and multiplies it into the texture. Any
  shading painted into the image gets multiplied a second time, and the block
  reads as dirty. This is the single most common way generated textures look
  wrong in-engine.
- **No darkening toward the edges** — models love adding a vignette. In a voxel
  world that instantly reads as a grid of outlined tiles.
- **Flat orthographic, no perspective** — the texture is applied to a flat cube
  face; any implied camera angle fights the actual geometry.
- **Fills the frame** — no margins, or you get gaps between blocks.

### Avoiding the "obvious repeat"

Seamless is required; *visibly repeating* is the failure mode. Add for any
natural material:

> Non-repeating organic variation across the surface, with no single dominant
> feature, blob, crack, or bright spot that would draw the eye and become
> obvious when the texture tiles. Evenly distributed detail with no focal point.

That last clause is the trick: a texture reads as a repeating pattern when it
has one memorable landmark. Even, focus-free detail tiles invisibly.

For **manufactured** materials (brick, planks, tiles, fabric) the opposite
applies — a regular repeat is correct and expected. Ask for it explicitly, and
ask for the grid to align to the tile edges so courses line up across blocks.

---

## Category blocks

Paste the matching one after the base prompt.

### Stone / rock / ore

> Dense mineral surface with fine crystalline grain and subtle tonal mottling.
> Small natural pits and micro-fractures distributed evenly. No large cracks or
> single dominant feature.

For **ore**, add:

> Sparse veins and small nuggets of [MINERAL] scattered evenly through the
> stone, distinctly coloured against the rock, small enough to read as flecks
> rather than a pattern, and never clustered into one obvious clump.

### Soil / sand / gravel

> Granular surface with visible particle structure and gentle tonal variation
> between grains. Loose, natural distribution with no directional streaking.

### Grass / foliage (top faces)

> Dense fine blades in varied greens, tightly packed and evenly distributed,
> with subtle variation in tone between clumps. Rich saturated green with warm
> undertones. No bare patches, no flowers, no single standout feature.

### Grass / dirt side faces (the transition strip)

> The upper portion is a band of dense grass with an irregular natural fringe
> hanging down, over a lower portion of granular soil. The grass band and the
> soil must each tile seamlessly left-to-right, and the top and bottom edges
> must match a pure grass texture above and a pure soil texture below.

That last sentence matters: side textures sit between the top and bottom
textures and have to meet both.

### Wood — bark (side)

> Vertical bark texture with natural ridges and fissures running top to bottom,
> varied in width and depth, with warm brown tones. Ridges continue cleanly
> across the top and bottom edges so vertically stacked blocks form an unbroken
> trunk.

### Wood — cut end (top/bottom)

> Concentric growth rings radiating from a centre, warm honey-toned wood with
> fine radial grain.

*(Note: growth rings are inherently centred, so this face will not tile
seamlessly — that's correct and expected. It's a cut end, not a continuous
surface.)*

### Wood — planks (crafted)

> Parallel sawn planks of varied width running in one direction, each with
> distinct grain and slight tonal variation between boards, separated by fine
> dark seams. Plank ends align exactly to the tile edges so boards continue
> across adjacent blocks.

### Masonry — brick / cut stone

> A regular course of [MATERIAL] blocks with visible mortar joints, slight
> variation in tone and weathering between individual blocks. The course pattern
> aligns exactly to the tile edges so the bond continues unbroken across
> adjacent blocks.

### Foliage — leaves

> Dense overlapping leaf cluster filling the frame, layered depth, varied greens
> with warm highlights, small gaps of darkness between leaves. Even distribution
> with no single branch or standout leaf.

### Metal / crafted / refined

> Worked [METAL] surface with subtle hammered or brushed texture, gentle tonal
> variation, light patina in recesses. Clean and crafted rather than industrial.

### Liquid (water, lava)

> Smooth flowing surface with soft undulation and subtle internal colour
> variation, gentle translucency implied by tonal depth. No specular highlights,
> no reflections, no foam or edge detail.

---

## Technical requirements

- **Square, power of two.** 32×32 or 64×64 is the sweet spot for this style —
  enough for real material structure, small enough to stay stylised. Whatever
  you pick, **every texture in the set must be identical in size**: they go into
  a GPU texture array, which requires uniform dimensions.
- **PNG, RGBA.** Opaque for solid blocks; alpha only where a block is genuinely
  see-through (leaves, glass).
- **Six faces per block.** The registry stores a texture layer per face, so a
  block can use one texture on all six, or differ (grass: top / side / bottom).
- **Generate large, downsample.** Models produce better structure at 512–1024,
  then reduce to target size with a good filter. Direct generation at 32×32
  usually produces mush.

## Checking a texture before committing to it

1. **Tile it 3×3 in an image editor.** Any seam, or any feature your eye jumps
   to, means regenerate. This catches more problems than anything else.
2. **Squint at it.** It should still read as the material. If it turns into flat
   colour, it needs more structure; if it turns into noise, it needs less.
3. **Check it beside its neighbours.** Grass next to dirt next to stone — do they
   look like the same world?
4. **Look at it in-engine at distance**, not just up close. Voxterra's LOD means
   most of what a player sees is far away.

## Prompt template

```
A seamless, tileable texture of [BLOCK] for a voxel game, in a stylised
fantasy-realism art style: rich saturated colour, warm lighting bias, clear
material structure, hand-crafted illustrated quality rather than a photograph.
Flat orthographic top-down view of a flat surface, filling the frame edge to
edge. Perfectly seamless and tileable on all four edges, so the pattern
continues without any visible seam when repeated in a grid. Even, flat, ambient
illumination with no directional light, no cast shadows, no highlights or
hotspots, no vignette, and no darkening toward the edges. No perspective, no
depth of field, no blur. No border, frame, text, watermark, or object outline. A
single continuous material filling the entire image.

[CATEGORY BLOCK]

[ANY BLOCK-SPECIFIC DETAIL]
```
