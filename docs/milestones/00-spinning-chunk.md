# Milestone 00 — Spinning Chunk

- **Status:** In progress
- **Spec written:** 2026-06-12
- **Depends on:** ADR-0001

## Goal

A window opens. One hardcoded 32³ chunk is meshed and rendered with flat
per-block-type colors. A fly camera moves around it. That's it.

This milestone forces the entire vertical slice into existence — windowing,
the wgpu pipeline (surface, shaders, vertex/index/uniform buffers, depth
buffer), the chunk data structure, the chunk→mesh→GPU data flow, and the game
loop. Every line is load-bearing for the next several years. It is also the
first real test of the LLM-assisted workflow on systems code, at the smallest
possible scale.

## Acceptance criteria

1. `cargo run -p vox-app` opens a window with a sky-colored clear background
   and a visible depth-tested 3D scene. Resizing the window does not crash or
   distort the projection.
2. A single `Chunk` (32³, from `vox-core`) is filled procedurally in code —
   e.g. stone below a sine-wave surface, dirt for the top layer, air above.
   At least 3 distinct block types are visible.
3. The chunk is converted to a mesh by `vox-mesh` using **naive culled
   meshing**: one quad per solid-block face that borders a non-solid block;
   interior faces are not emitted. (Greedy meshing is Milestone 01.)
4. Faces are flat-shaded with a per-face-direction brightness factor (top
   brightest, bottom darkest) so geometry is readable without lighting.
5. Fly camera: WASD + mouse look + Space/Shift for up/down, with a
   perspective projection. Cursor is captured; Esc releases it.
6. Frame loop renders at vsync; no per-frame heap allocation in the render
   path once the mesh is built (build once, draw many).
7. `vox-core` and `vox-mesh` compile with no graphics dependencies, and have
   at least: a palette round-trip test placeholder is NOT required yet, but
   `vox-core` has unit tests for coordinate helpers (`world↔chunk↔local`,
   including negative coordinates), and `vox-mesh` has a unit test asserting
   a single isolated block produces exactly 6 quads and a fully solid 2×1×1
   pair produces 10.
8. `cargo fmt --check` and `cargo clippy -D warnings` pass.

## Non-goals (explicitly out of scope)

- Palette compression (Milestone 01 — chunk storage may be a flat
  `[BlockId; 32768]` array for now, but behind a `Chunk` API so the swap is
  invisible to callers).
- Greedy meshing, multithreaded meshing, multiple chunks, streaming.
- Textures, texture atlas, real lighting, sky rendering.
- Block interaction, physics, collision (the camera flies through blocks).
- Any worldgen beyond the hardcoded test fill.

## Task breakdown

1. **Workspace setup.** Cargo workspace, five crates per CLAUDE.md layout,
   CI-ready lints (`clippy`, `fmt`). Commit: "skeleton".
2. **vox-core.** `BlockId`, `Chunk` (flat array behind an API), `WorldPos` /
   `ChunkPos` / `LocalPos` newtypes, `coords` helpers with tests.
3. **vox-app shell.** winit window + event loop, wgpu surface/device/queue,
   clear-color render pass, resize handling.
4. **Camera.** Perspective projection, fly controller, uniform buffer +
   bind group.
5. **vox-mesh.** Culled mesher producing `Vec<Vertex>` + `Vec<u32>` indices;
   tests from acceptance criterion 7.
6. **Wire-up.** Test-fill a chunk, mesh it, upload buffers, draw. Per-face
   brightness in the shader.

## Notes for whoever builds this (human or model)

- Follow the Learn wgpu tutorial structure for steps 3–4; do not invent novel
  renderer architecture at this stage. Boring and correct beats clever.
- Vertex format: position (f32×3), normal-or-face-id, color/brightness. Keep
  it minimal; it will be redesigned when textures arrive (expected, fine).
- Put the chunk test-fill in `vox-app`, not `vox-core` — it's scaffolding,
  not engine.
- When this milestone is accepted: update CLAUDE.md "Current status", write
  a one-paragraph retrospective at the bottom of this file (what was harder
  than expected, what the next spec should account for), then write
  `01-real-chunk-system.md` before writing any Milestone 01 code.
