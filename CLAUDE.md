# CLAUDE.md — Project Constitution

Read this file in full before doing anything else in this repository. It is the
single source of truth for what this project is, how it is built, and the
invariants that must never be violated. If a request conflicts with this file,
raise the conflict instead of silently complying.

## What this project is

A voxel-based survival/building game (working title: **Voxterra**) in the spirit of
Minecraft and Vintage Story, with three pillars that rank above everything else:

1. **Performance first.** The game must outperform Minecraft: very high render
   distances via a built-in LOD system (Distant Horizons-style), playable on
   low-end hardware. Every architectural decision is weighed against this.
2. **Realistic world generation.** Geology-driven worldgen: tectonic-informed
   terrain, real stratigraphy (sedimentary/igneous/metamorphic), Whittaker/Köppen
   biomes, and associative ore generation (ore appears where real geology would
   put it, e.g. porphyry copper near intrusions, placer gold in river gravels).
3. **Engaging, not tedious.** Realism must create interesting decisions
   (shore the tunnel or risk a cave-in?), never repetition. When realism and
   fun conflict, fun wins. Hardcore mechanics (cave-ins, tree felling danger)
   are toggleable options.

This is a long-horizon solo project (multi-year). There are no deadlines.
"Do it right" beats "do it fast" in every tradeoff.

## Core technical decisions (see docs/decisions/ for rationale)

- **Custom engine.** No Unity, no Unreal, no engine forks. We use libraries for
  generic problems (see below) and write the voxel-specific core ourselves.
- **Language: Rust.** Edition 2021+. Idiomatic Rust; no `unsafe` without an
  ADR-level justification and a `// SAFETY:` comment.
- **Graphics: wgpu.** Windowing: winit. Parallelism: rayon for data
  parallelism, dedicated job/thread patterns where appropriate.
- **Voxels are 1m³.** Realistic player jump height (~0.5m) with auto-step for
  half-block heights; slopes/stairs matter. No sub-meter voxel grid.
- **Chunks are 32×32×32 and CUBIC.** The world is a 3D grid of cube chunks in
  all axes, including Y. There are no column-based chunks anywhere. Never write
  code that assumes a vertical column, a global sea-level-relative heightmap in
  storage, or a fixed world height. Build height is effectively unlimited.
- **Chunk storage is palette-compressed.** Each chunk stores a palette of block
  states plus a packed index array. This is how we afford hundreds of rock and
  mineral types at low memory cost. Code must never assume a fixed
  bits-per-voxel.
- **Lighting is true 3D propagation.** Skylight cannot be a top-down column
  scan (cubic chunks make that impossible). Heightmap hints are an optimization
  layer only, never the source of truth.
- **LOD is an octree over chunks.** Full-resolution chunks at the leaves,
  downsampled voxel data at higher nodes. The chunk system and worldgen must
  stay compatible with generating/meshing at reduced resolution.
- **Worldgen is a two-stage pipeline.** Stage 1 (world creation): low-res
  planetary simulation — tectonics, climate, biome maps. Stage 2 (chunk gen):
  local detail derived deterministically from stage-1 maps. Any chunk at any
  coordinate must be generatable independently, without generating neighbors
  or anything "below" it.
- **All serialized formats are versioned.** Chunk format, save format, network
  protocol (future): every one carries a version field from day one.

## Workspace layout

```
voxterra/
├── CLAUDE.md             # this file
├── docs/
│   ├── decisions/        # ADRs: numbered, immutable once accepted
│   └── milestones/       # one spec per milestone, written BEFORE building
├── crates/
│   ├── vox-core          # voxel/block types, chunk storage, palette, coords
│   ├── vox-mesh          # meshing algorithms; headless, unit-testable
│   ├── vox-worldgen      # generation pipeline
│   ├── vox-render        # wgpu renderer, camera, chunk render pipeline
│   └── vox-app           # binary: winit window, game loop, wiring
```

Dependency direction: `vox-app` → everything; `vox-render`/`vox-mesh`/
`vox-worldgen` → `vox-core`; `vox-core` depends on nothing in the workspace.
`vox-core`, `vox-mesh`, and `vox-worldgen` must never depend on wgpu, winit, or
anything graphical — they must compile and test headlessly.

Do not create new crates until a milestone spec calls for them. Planned future
crates: `vox-sim`, `vox-net`, `vox-client`, `vox-server`.

## Coordinate conventions

- Right-handed, **Y-up**. World coordinates are `i64` per axis (effectively
  unbounded world). Local (in-chunk) coordinates are `0..32` per axis.
- `ChunkPos` = world position >> 5 (arithmetic shift; correct for negatives).
  Local = world & 31. NEVER use `/ 32` and `% 32` on signed integers for this —
  it is wrong for negative coordinates. Use the shared helpers in
  `vox-core::coords`; do not reimplement coordinate math at call sites.
- Distinct newtypes for `WorldPos`, `ChunkPos`, `LocalPos`. No raw tuples for
  positions crossing function boundaries.

## Code standards

- Every system of nontrivial complexity gets unit tests. Meshing, palette
  storage, coordinate math, and worldgen determinism are the highest-value
  test targets (e.g. greedy mesher output must equal naive mesher output on
  randomized chunks, palette round-trips must be lossless, same seed must
  produce byte-identical chunks).
- `cargo fmt` and `cargo clippy -D warnings` must pass before any commit.
- Performance-sensitive code paths (meshing, lighting, gen) get benchmarks
  (criterion) once they stabilize. Optimize from measurements, not vibes.
- Prefer plain data + functions over deep trait hierarchies. Avoid premature
  abstraction; three concrete uses before generalizing.
- Comments explain *why*, not *what*. Anything surprising gets a comment
  pointing at the relevant ADR.

## Workflow rules (human + LLM)

- Nathan applies all changes and runs all commands himself; provide exact,
  copy-pasteable edits and commands.
- Every milestone gets a spec in `docs/milestones/` (goal, acceptance criteria,
  non-goals) **before** implementation begins.
- Every significant architectural choice gets an ADR in `docs/decisions/`
  before or alongside the code that implements it. ADRs are immutable once
  accepted; supersede with a new ADR rather than editing history.
- When picking up the project cold: read this file, then the milestone spec
  currently in progress, then the most recent ADRs. Ask before deviating from
  any of them.
- Batch related changes; commits include a short changelog.
- "Primary dev platform is native Windows (PowerShell); commands should be PowerShell-compatible."

## Intentionally deferred (do not build these yet)

Multiplayer/networking, modding API, audio, entities/mobs, combat, survival
mechanics (hunger, temperature), the full LOD octree, real worldgen, and all
skill systems (mining cave-ins, tree felling). They are on the roadmap; their
absence is deliberate, not an oversight. Do not add speculative hooks for them
beyond what the invariants above already require.

## Current status

- **Active milestone:** 04 — Lighting (spec not yet written)
- **Last completed milestone:** 03 — Blocks, Interaction, Textures (2026-06-15)
- Milestones 02 (Infinite World, 2026-06-14) and 03 are complete; see their
  retrospectives in `docs/milestones/`.
- Update this section at the end of every working session.