# ADR-0001: Custom engine in Rust + wgpu, cubic chunks, 1m voxels

- **Status:** Accepted
- **Date:** 2026-06-12

## Context

The project is a voxel survival/building game whose top priority is
performance: better-than-Minecraft framerates, extreme render distance via
built-in LOD, unlimited build height, and viability on low-end hardware.
Secondary priorities are geologically realistic worldgen and long-term
(multi-year, possibly decade) maintainability by a solo developer working
heavily with LLM assistance. There are no deadlines.

Options considered:

1. **Unity / Unreal.** Rejected. Both are mesh/scene-graph engines; voxel
   worlds fight their core architecture (GC pressure or scene management
   overhead, lighting models, streaming). No serious game in this genre ships
   on them; Minecraft, Vintage Story, Veloren, and Luanti are all custom.
2. **Fork Luanti (Minetest).** Seriously considered: it would save an
   estimated 1.5–2.5 years (chunk storage, networking, serialization, content
   pipeline, renderer, modding API exist). Rejected because its
   Irrlicht-derived renderer is architecturally incompatible with the
   project's #1 goal — no modern GPU-driven pipeline, largely single-threaded
   rendering, no foundation for a Distant Horizons-style LOD system — and the
   renderer is entangled with client/scene architecture, so replacing it
   inside a ~15-year-old legacy C++ codebase is plausibly harder than building
   fresh. Forking optimizes time-to-walkable-world, not time-to-stated-goals.
   Luanti and Veloren remain reference material: read their solutions, write
   ours clean.
3. **Bevy.** Defensible, but its renderer is not designed for voxel-scale
   geometry; a custom render pipeline would be needed inside it anyway.
   Rejected in favor of full control. May revisit ECS patterns independently.
4. **Custom engine on Rust + wgpu.** Accepted.

## Decision

- **Language: Rust.** Memory safety and fearless multithreading are decisive
  for a heavily multithreaded engine (worldgen, meshing, lighting, IO on job
  systems) maintained solo for years. Greenfield idiomatic Rust is also the
  strongest setting for LLM-assisted development, a stated project goal.
- **Graphics: wgpu** (windowing via winit). Modern API, portable
  (Vulkan/Metal/DX12), good Rust ecosystem, supports the GPU-driven techniques
  the LOD goal requires.
- **"Custom engine" ≠ everything from scratch.** Generic problems use
  libraries (winit, rayon, egui for debug UI, possibly rapier/renet later).
  The from-scratch surface is exactly the voxel-specific core: chunk system,
  meshing, lighting, LOD, worldgen.
- **Chunks are 32³ and cubic in all axes from day one.** Cubic chunks cannot
  be retrofitted affordably (Minecraft's history demonstrates this).
  Consequences accepted up front: true 3D light propagation (no top-down
  skylight scan), worldgen decomposable per-chunk at any Y, LOD as an octree
  over chunks.
- **Voxels are 1m³.** Sub-meter voxels scale cost cubically (0.5m = 8×, 0.25m
  = 64× data) and are incompatible with the performance goals; they also
  increase gameplay tedium. Perceived realism is achieved instead via reduced
  jump height (~0.5m) with auto-step, meaningful slopes/stairs, and a possible
  future chisel/microblock system that pays detail cost only where used.
- **Chunk storage is palette-compressed** with a versioned serialization
  format, enabling hundreds of block types at low memory cost.

## Consequences

- Estimated 6–12 months before a fully walkable streaming world; accepted, as
  the project has no deadline and the foundation is the product.
- A 3–6 month systems/graphics learning curve is part of the plan, not a
  delay. Minimum bar before heavy implementation: the Learn wgpu tutorial
  through depth buffer + camera.
- All code must respect the invariants in CLAUDE.md (no column assumptions,
  no fixed bits-per-voxel, headless core crates, versioned formats).
- Superseding any decision here requires a new ADR, not an edit to this one.
