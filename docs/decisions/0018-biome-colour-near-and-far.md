# ADR-0018: Biome colour, near and far

- **Status:** Proposed (2026-11-01, M13 task 4). To be accepted at the M13
  close, after the owner has seen it in play.
- **Context:** M13 paints biomes onto the world. Two things decide how a
  biome looks from a distance: what its ground is made of (sand, snow,
  podzol…), and the colour of its grass, which on Earth runs from the deep
  green of a rainforest through the yellow of a dry savanna to the olive and
  brown of the north. Both must be the same at the player's feet and at
  64 km, or the boundary between full resolution and the LOD shows as a
  change of colour. The owner chose a smooth tint over distinct grass blocks
  (2026-11-01).

## Decision

### 1. The LOD wears each column's top block

`Generator::lod_surface` returns, per LOD cell, its height and its top
block: the block full resolution generates at the cell's lowest sample —
the column that sets the cell's height — classified by the same
`Generator::column`. `vox_mesh::mesh_lod_surface` takes those blocks: a
cell's top and the walls it stands on use that block's faces, and flat-cell
merging joins only cells of one block. A distant desert is sand, an ice cap
snow, a seabed mud or sand.

**Polar seas rise to their ice.** A polar sea cell's height is sea level,
its top ice: ice is the solid top there, and `solid` is the terrain top the
LOD draws (ADR-0011). The LOD's translucent sea surface, drawn only over
cells below sea level, therefore stops at the ice edge exactly as the near
sea does.

Tested: at stride 1, where a cell is one column, every cell's height and top
block equal what full resolution generates, in every biome
(`lod_surface_matches_full_resolution_in_every_biome`).

### 2. Grass is tinted in the shader, from the climate map

The stage-1 climate map (ADR-0017) — sea-level temperature and annual
precipitation per cell — is uploaded once per world as an `nx × nz` RGBA8
texture (`vox_render::encode_climate`: temperature linear over −50..50 °C,
precipitation as the square root of its share of 9 000 mm, fine where rain is
scarce and grass colour changes fastest). One shared shader file,
`climate.wgsl`, appended to both terrain shaders like curvature and
atmosphere (ADRs 0015, 0016), samples it per fragment with a linear,
repeating sampler — the texture repeats exactly as the world does, so the
lookup has no seam — cools the temperature for the fragment's own height at
the generator's lapse rate (passed in, so `vox_worldgen` stays its one
source), and turns temperature and precipitation into a grass colour:
wetness judged against Köppen's aridity threshold as the biome classifier
judges it, then a blend of five colours — hot and wet, hot and dry, mild and
wet, cold and wet, cold and dry. Only the grass layers are tinted.

The tint is a ratio to the grass tile's own colour, taken **in linear
light** (the block textures are sRGB, so the shader samples linear values; a
ratio of sRGB values was far too weak — dry grass stayed green). Mild, wet
grass is exactly the tile as before.

**Why not distinct grass blocks:** colour would change in steps at biome
edges, and the greedy mesher and LOD merging would split at every one.
**Why not a vertex colour:** a new vertex attribute on every face of both
meshers, and merging would break wherever the colour changed — everywhere,
for a smooth gradient. The texture costs one sample per grass fragment and
nothing else; vertex formats, merging and memory are untouched.

### 3. Addressing the map is reading terrain content

The shader needs where the render origin sits on the map. That is computed
in f64 on the CPU as the origin's fraction of the world, wrapped to `[0, 1)`
(`Renderer::write_climate_placement`), and narrowed to f32 only as that
fraction. CLAUDE.md names three places positions are canonicalised —
terrain generation, the save layer, the LOD edit overlay. This is not a
fourth: the climate map *is* terrain generation's content, and reading it
is generation's content addressing, done on the GPU. Nothing the player's
frame, streaming, physics or rendering geometry uses is wrapped; the camera
and every vertex keep their unwrapped coordinates.

## Consequences

- The sky uniform grows from five vec4s to seven (`climate_a`, `climate_b`);
  group 3 gains the climate texture and its sampler. A 1×1 placeholder keeps
  the bind group valid before a world's map arrives, and until then grass
  is untinted (headless tests and harnesses unchanged).
- `vox_core::registry::L_GRASS_TOP` / `L_GRASS_SIDE` are public: the shader
  copies them, and a test pins the copies and the tint's reference colour to
  their sources.
- Leaves and other foliage will use the same tint when vegetation arrives.
- **Seen in headless renders, not yet in play:** a shallow sea over a pale
  sand seabed now reads grey, not green, through translucent water of
  constant opacity. Water that absorbs with depth (`docs/vision.md` §10)
  would make it turquoise over shallows and dark blue over the deep; it is
  not part of this decision.
