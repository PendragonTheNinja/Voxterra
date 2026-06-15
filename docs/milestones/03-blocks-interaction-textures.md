# Milestone 03 — Blocks, Interaction, and Textures

- **Status:** COMPLETE (2026-06-15)
- **Spec written:** 2026-06-14
- **Depends on:** Milestone 02

## Goal

Turn the placeholder world into one that is **interactive** and **looks
real**: a proper data-driven block registry (retiring the hardcoded
STONE/DIRT/GRASS trio), raycast block breaking and placing (promoting the
debug hole-punch to real gameplay), and per-block, per-face textures via a
texture atlas (replacing the flat baked colors).

Lighting is explicitly **Milestone 04**, not this one (see M02 retro / the
scoping decision). This milestone keeps the existing directional face
shading as the only "lighting" so textures read as 3D.

**Acceptance scene:** spawn into the textured world; look at a block and see
a highlight on the targeted face; left-click breaks the targeted block
(it disappears, neighbors re-mesh, the edit persists per M02); right-click
places the currently-selected block against the targeted face; cycle the
selected block type; fly away and back and confirm both breaks and places
persisted to disk.

## Acceptance criteria

1. **Block registry.** A data-driven registry replaces the placeholder
   `blocks` module. Each block type has: a stable numeric id (the existing
   `BlockId`), a name, per-face texture references, and flags needed now
   (at minimum: `solid` / occludes-neighbors, and `targetable` /
   can-be-broken). The registry is the single source of truth; vox-worldgen
   and vox-app stop hardcoding ids/colors and consult it. Air is always
   id 0. The set of starter blocks is small but real (e.g. stone, dirt,
   grass, plus a couple more so placing is interesting).
2. **Registry drives meshing appearance.** The mesher no longer bakes a
   flat color per block; it emits texture coordinates (atlas tile + face)
   so the shader samples the atlas. Block→appearance lookup goes through the
   registry, passed into meshing the way `color_of` is today (a borrowed
   lookup, keeping vox-mesh headless and registry-agnostic).
3. **Texture atlas.** A single atlas texture holds all block face tiles;
   the renderer samples it. Tiles are addressed by index; the vertex format
   carries per-vertex UV (or a tile index + corner) so each face shows the
   right tile. Nearest-neighbor filtering (crisp voxel pixels), with correct
   handling of tile edges (no bleeding between atlas tiles — half-texel
   inset or a padded atlas).
4. **Raycast targeting.** A voxel raycast (DDA / Amanatides–Woo) from the
   camera finds the first solid block within reach (a few meters) and the
   face hit. The targeted block's face is visually highlighted (wireframe
   box or face overlay). Looking at nothing in range → no highlight.
5. **Break.** Left-click removes the targeted block (set to air), re-meshing
   the affected chunk and its border neighbors via the existing dirty-set
   path, marking the chunk modified (so it persists, M02). No-op when
   nothing is targeted.
6. **Place.** Right-click sets the air cell *adjacent to the hit face* to
   the currently-selected block, but only if that cell is empty and not
   overlapping the player's own position. Re-mesh + persist as for break.
7. **Block selection.** A way to change which block right-click places
   (number keys and/or scroll wheel) with a clear indication of the current
   selection (log line is acceptable; a minimal on-screen indicator is a
   bonus, not required).
8. **Persistence + correctness.** Breaks and places survive unload→reload
   and app restart (M02 save path). Edited chunks serialize correctly
   (format version bumped only if the chunk payload actually changes — it
   should NOT need to this milestone, since blocks are still just `BlockId`s).
9. `cargo test` green workspace-wide; clippy/fmt clean. Headless-testable
   logic — the raycast (given a block-accessor closure), registry lookups,
   atlas UV math, and place/break target computation — is unit-tested in the
   core/headless crates, not only observed in vox-app.

## Non-goals (explicitly out of scope)

- **All lighting** (skylight, block light, propagation). Milestone 04. Keep
  the current directional face-brightness shading.
- Transparency / translucency / non-cube block shapes (slabs, stairs,
  microblocks, water). All blocks remain full opaque cubes this milestone.
- Block *state* beyond identity (orientation, growth stage). `BlockId`
  stays a plain id; no state packing.
- Mining time / tool requirements / block drops / inventory. Break is
  instant, place is from an infinite selected type. (Survival mechanics are
  later.)
- Animated textures, mipmaps, anisotropic filtering, normal maps.
- Real geological worldgen (still the placeholder heightmap).
- Sound, particles, block-break feedback beyond the visual removal.

## Key design decisions to make (in the spec/ADR, before coding)

- **Where the registry lives.** Likely a new `vox-block` crate or a
  `registry` module in vox-core. It must stay headless (no wgpu). vox-mesh
  and vox-worldgen depend on it; vox-render consumes only the atlas + the
  resolved UVs. Decide and record (small ADR if it warrants one).
- **Vertex format change.** Replace baked `color: [f32;3]` with texture
  addressing (UV `[f32;2]`, or a packed tile index + per-corner UV). Keep
  the per-face directional brightness (a scalar) so geometry still reads.
  This changes the vertex struct, the WGSL shader, the pipeline vertex
  layout, and the greedy mesher's merge criterion (faces only merge if they
  share block AND face direction AND would tile the texture consistently —
  greedy-meshed rectangles need correct UV tiling across the merged span;
  decide how: per-tile repeat via sampler, or split merges at tile
  boundaries). **This is the subtle part of the milestone — call it out.**
- **Atlas construction.** Build-time static atlas vs. runtime-assembled from
  individual tile images. For a starter set, a small hand-made atlas (or a
  generated solid-color-tile atlas as a first step) is fine; document the
  path so real textures can drop in later.

## Suggested task breakdown

1. **Block registry (headless).** Define block-type data + registry;
   port worldgen and vox-app off hardcoded ids. Unit-test lookups. No visual
   change yet (registry can still resolve to colors as an interim step).
2. **Vertex format + atlas plumbing (no interaction yet).** Change Vertex to
   carry texture coords; build a minimal atlas (even solid-color tiles);
   update shader + pipeline; mesher emits UVs from the registry. Resolve the
   greedy-meshing-UV-tiling question here. Visual result: textured (or
   tiled-color) world, looks the same or better, still no interaction.
3. **Raycast + highlight (headless raycast + render overlay).** DDA voxel
   raycast against a block accessor; unit-test it. Render the targeted-face
   highlight.
4. **Break + place + selection.** Wire clicks to the raycast result through
   the dirty-set re-mesh + persist path; block selection input. Retire the
   debug hole-punch (or keep it as a bulk-debug tool).
5. **Real-ish textures + polish.** Drop in actual block tiles, fix any
   atlas-bleed, tune reach/highlight. Measure, record baseline (atlas size,
   any fps delta from texturing).

## Notes for whoever builds this (human or model)

- Keep vox-mesh headless and registry-agnostic: it should receive an
  appearance lookup (block → per-face tile/UV info) as a borrowed closure or
  small trait, exactly as it receives `color_of` today. Do NOT make vox-mesh
  depend on the renderer or the atlas.
- The greedy mesher + texture tiling interaction is the one genuinely tricky
  bit. A merged W×H face must tile its texture W×H times (not stretch one
  tile across the whole rectangle). Easiest correct approach: let UVs run
  0..W / 0..H across the rect and use a `Repeat` sampler **per tile** — but
  that conflicts with a shared atlas (repeat wraps the whole atlas). Options:
  (a) split greedy merges at unit boundaries for textured faces (lose some
  merging benefit), (b) use a texture array instead of an atlas (each layer
  one tile, `Repeat` works per layer), or (c) compute tiling in the shader
  from a tile index + local face coords. **Pick deliberately and record why**
  — this decision shapes the vertex format. A texture array (option b) is
  often the cleanest for voxel engines; weigh it first.
- Reuse the dirty-set + neighbor-expansion path for break/place — it already
  handles re-meshing and persistence correctly (M02). Don't build a parallel
  edit path.
- Place must reject cells that overlap the player to avoid suffocating the
  camera; a simple AABB-vs-cell check is enough (no real physics yet).
- When this milestone is accepted: update CLAUDE.md status, write the
  retrospective with baseline numbers, then write `04-lighting.md` (with its
  own ADR for the cubic-chunk skylight approach) before any M04 code.


## Retrospective (2026-06-15)

**Status: COMPLETE.** All five tasks landed; all acceptance criteria met.
Voxterra is now interactive and textured: you can look at a block, break it,
place blocks against faces, pick the block type, and everything persists.

### Measured baseline (record for future comparison)

Dev machine: Windows, NVIDIA GPU, 20 logical cores. Settings unchanged from
M02 (`LOAD_RADIUS = 8`, `UNLOAD_RADIUS = 10`, `MESH_BUDGET = 48`,
`GEN_SPAWN_BUDGET = 64`). Reach for interaction = 6 blocks.

- **fps:** 142–145, vsync-capped — **identical to M02's flat-color
  baseline**. Texturing via the texture array (ADR-0003) cost no measurable
  framerate. This is the headline result: per-block, per-face textures for
  free.
- **drawn/total:** ~190/700 near the surface (higher than M02's ~150/550
  only because the baseline was taken closer to terrain — more non-empty
  chunks in view; not a regression).
- **loaded:** ~2,671 steady (unchanged; streaming untouched this milestone).
- **Editing:** break/place log instantly; `dirty` stays at 0 or
  spikes-and-drains within a frame. Edits route through the existing
  dirty-set re-mesh + persist path — no hitch, no parallel code path.

### What went well

- **Texture array over atlas (ADR-0003) paid off exactly as predicted.** A
  `Repeat` sampler tiles each layer across greedy-merged faces with zero
  bleed, so full greedy meshing was preserved and "atlas-bleed" never became
  a class of bug. The differential test was upgraded to compare per-face
  *layer* coverage, so grass's distinct top/side/bottom is provably correct.
- **The borrowed-closure pattern kept paying dividends.** vox-mesh stayed
  headless: its appearance input changed from `color_of` to
  `layer_of(block, face)` without the crate learning anything about textures
  or the registry. The raycast took the same shape — a borrowed
  `is_solid(WorldPos)` closure — so it's fully unit-testable without a World.
- **Interaction reused the M02 edit path wholesale.** Break and place are
  just `set_block` + `mark_dirty_with_neighbors`; persistence came for free
  because `set_block` already marks chunks modified. No new save logic.
- Headless verification again covered the risky logic: the DDA raycast
  (10 tests: axis hits, diagonals-without-tunneling, face normals, misses,
  start-inside-solid, the place=block+normal invariant) and the
  placement-overlap check (flush contact is not overlap).

### Decisions worth remembering

- **Block ids stayed stable (stone=1, dirt=2, grass=3).** The registry took
  over their definition without renumbering, so M02 saved worlds still load.
  New blocks (sand, cobblestone, planks) took ids 4–6.
- **Registry lives in vox-core, not a `vox-block` crate.** Headless, depended
  on by vox-mesh/vox-worldgen through vox-core already. Revisit if it grows a
  data-file loading pipeline.
- **PNG texture loading deliberately deferred.** The texture-array pipeline
  is ready to receive real art (swap `tile_pixels` for image loading, no
  format change), but with no art authored yet, building the loader now would
  be speculative. Add it when real tiles exist — likely a small task then.
- **Player AABB for place-rejection is a stand-in.** A 0.6³ box at the camera
  point, enough to stop entombing the view. A real player body (with physics)
  replaces it later.

### Process note (for future handoffs)

Two slips this milestone, both the same shape: a one-line `pub use` export
edit in `vox-core/src/lib.rs` (first `cell_overlaps_aabb`, and the M03 spec
file itself) didn't survive the copy from sandbox to the working tree, so a
later step failed to compile / a doc went missing. Lesson: when packaging
vox-core, diff the crate-root re-export lines specifically, and verify
milestone/spec docs are committed (they were applied locally but not always
committed). The two wgpu-version gaps (`ImageCopyTexture` →
`TexelCopyTextureInfo`, and the `unused_assignments` / `collapsible_match`
clippy lints) were caught by the dev-machine gauntlet as designed.

### Carried into Milestone 04 (lighting)

- The dirty-set + neighbor-expansion path is the universal re-mesh route and
  will carry lighting updates too: when light changes in a chunk, mark it +
  affected neighbors dirty and re-mesh. Budget any light propagation work
  per-frame like meshing/gen, and surface it in telemetry.
- The vertex format already carries a `brightness` scalar (currently just
  directional face shading). Lighting will replace/extend this with real
  light values — likely the main vertex-format change of M04.
- The chunk serialization format has a version byte: storing per-voxel light
  (if baked) or block light sources means bumping `CHUNK_FORMAT_VERSION` and
  handling v1. Skylight in cubic chunks with no height limit is the hard part
  and needs its own ADR before any M04 code (per the M03 spec's closing note).
- The registry has a `solid` flag already; lighting will want
  transparency/light-emission fields on `BlockType` (glass, glowstone-likes).
