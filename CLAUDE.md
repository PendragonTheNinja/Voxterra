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

## AI collaborator sandbox workflow (how a Claude instance works this repo)

A Claude instance runs in a **Linux sandbox** and cannot see or run the game.
The division of labour:

- **Claude:** clones the repo to `/home/claude/voxterra`, edits and *headlessly
  verifies* `vox-core` and `vox-mesh`, then hands changed files back via
  `present_files`. After Nathan pushes, Claude re-syncs its clone to
  `origin/main` (`git fetch` + `reset --hard origin/main` + `clean -fd`).
- **Nathan:** applies the files on native Windows, runs the gauntlet, compiles
  the full app (incl. `vox-app`/`vox-render`), and is the **only** visual /
  GPU / runtime check. `vox-app` and `vox-render` are **review-only** in the
  sandbox — they need edition-2024 + a GPU and will not compile there.

**Headless test setup (`fixcheck`).** The sandbox's stock toolchain is older
than the project's Rust 1.96 / edition 2024, so to run `vox-core`/`vox-mesh`
tests Claude builds a throwaway `fixcheck/` workspace: copy those two crates,
downgrade `edition = "2024"` → `"2021"` in both Cargo.tomls, restrict the
workspace `members` to just those two, and rewrite any edition-2024-only syntax
to a 2021 equivalent (e.g. let-chains → nested `if`, `Option::is_none_or` →
`map_or(true, …)`). **These shims are sandbox-only and must never be committed
— the real files stay on edition 2024.** Find the current 2024-only constructs
by letting the copy fail to compile; they move around as the code evolves, so
don't hard-code a fixed list.

**Clippy.** Cannot be installed in the sandbox (neither `rustup component add`
nor apt). Claude must scan changed code manually for common 1.96 lints —
`doc_lazy_continuation`, `needless_range_loop`, `manual_range_contains`,
`duplicated_attributes`, `dead_code`, `identity_op`, `unnecessary_cast` — and
Nathan runs the real clippy gauntlet natively before every commit. Lints the
sandbox is blind to are real; treat the native clippy pass as authoritative.

**The gauntlet (Nathan runs before every commit, native PowerShell):**

    cargo build
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt
    cargo test
    $env:RUST_LOG="vox_app=info"
    cargo run -p vox-app

**Recurring gotchas (each has cost a build cycle):**
- `vox-core`'s public API is an **explicit re-export allowlist** in
  `crates/vox-core/src/lib.rs` (`pub use light::{…}`). Any new `pub fn` that
  `vox-app` calls must be added there; the sandbox can't catch the omission
  because it never compiles `vox-app`.
- **Verify file writes by reading back.** A generation script that reports
  success but skips the actual write has shipped a mismatched signature.
- `NEIGHBOR_OFFSETS.iter()` yields `&(i64,i64,i64)`; arithmetic auto-derefs
  (`pos.x + dx` is fine) but building an array for element `+=` needs explicit
  deref (`[*dx, *dy, *dz]`).

## Intentionally deferred (do not build these yet)

Multiplayer/networking, modding API, audio, entities/mobs, combat, survival
mechanics (hunger, temperature), the full LOD octree, real worldgen, and all
skill systems (mining cave-ins, tree felling). They are on the roadmap; their
absence is deliberate, not an oversight. Do not add speculative hooks for them
beyond what the invariants above already require.

## Current status

- **Active milestone:** none — M07 just closed. The headline next candidate is
  **LOD / far view distance** — its architecture is drafted and ratified-pending
  in `docs/decisions/0008-lod-architecture.md`, with the seed-driven-far-terrain
  bet already proven by a headless PoC (~195× cheaper than full-res, meshes with
  the existing greedy mesher unchanged). Proposed split: **M08** single-level LOD
  end-to-end, **M09** full octree + polish. Write the milestone spec before
  building.
- **Last completed milestone:** 07 — Day/Night Cycle (2026-07-06); retrospective
  with numbers at the end of `docs/milestones/07-day-night.md`. Shipped:
  `vox-core::time` (WorldTime + celestial model), two-channel sky/block vertex
  light (ADR-0007), a procedural sky pass (gradient, sun, phase-lit moon,
  world-locked starfield), and day/night wiring with `sky_scale` as a per-frame
  uniform (never baked into meshes). Ambient floor lowered to 0.004 (see the
  invariants note below).
- **Prior milestone:** 06 — Smooth Lighting & Ambient Occlusion (2026-07-03);
  retrospective at the end of `docs/milestones/06-smooth-lighting-ao.md`.
- Completed milestones have retrospectives in `docs/milestones/`.
- Milestones 02 (Infinite World, 2026-06-14) and 03 are complete; see their
  retrospectives in `docs/milestones/`.
- Update this section at the end of every working session.
## Lighting & streaming invariants (added post-M05 — do not violate)

These encode the root causes of the M05 sealed-cave daylight saga. Each one
was a real shipped bug. Regression tests exist for all of them (vox-core).

- **The padded border ring is light SOURCES only, never a CONDUIT.** Border
  cells of the 34³ relight volume stand in for neighbor light; they seed
  inward but must never receive or relay light (`LightVolume::accepts_spread`).
  Letting the ring conduct fabricates paths along chunk boundaries through
  space that is actually the neighbor's (possibly solid) territory — full
  daylight rode the ring down into sealed caves.
- **An unknown heightmap column is COVERED (top_sky = 0), never open.** During
  async streaming, air chunks load before the terrain below them; a missing
  column height must default dark. Assuming "open" injects daylight that the
  uniform fast path commits and nothing ever corrects. Dark-then-brighten is
  always safe; lit-then-frozen is the bug.
- **When a column's height becomes known or rises, relight its whole loaded
  chunk stack** (same chunk x,z) — chunks lit during the unknown window hold
  stale light that only a recompute clears. Keep this O(loaded) with cheap
  integer compares; never scan per-column against all loaded chunks.
- **Never mesh a chunk that is still queued for relight.** Light first, mesh
  once. Meshing before lighting bakes zero light (dark chunk checkerboard)
  and forces a second mesh — the single biggest streaming-burst cost found.
- **A changed border must re-mesh neighbors, not just re-light them.**
  Neighbor meshes sample this chunk's border cells; relight alone leaves
  stale dark seam faces.
- **The heightmap is raise-only** (edits/digging never lower it). This is a
  deliberate conservative choice: a too-high height only makes columns darker
  than truth (honest propagation from the overhead plane still lights them);
  a too-low height invents daylight. Preserve this asymmetry.
- The live relight entry point is `compute_chunk_light_2ch` via
  `relight_chunks_parallel`. Older single-channel functions in light.rs are
  legacy — do not extend them.
- **Day/night is a per-frame UNIFORM, never baked into the mesh** (added M07).
  `sky_scale`, sun/moon directions live in shader uniforms; the sun moving must
  re-mesh nothing. Vertices carry sky and block light as SEPARATE channels
  (ADR-0007) so night dims sky without touching torches; the shader combines
  them as `max(light_curve(sky) × sky_scale, light_curve(block))` — curve each
  source, THEN dim sky, so the falloff exponent and day/night dimming stay
  independent (monotonicity makes daytime identical to the pre-M07 formula).
- **Ambient floor is 0.004 ("almost black"), NOT 0.035** (changed M07,
  reversing the M06 "dark but not pure black" choice at the owner's request).
  Enclosed space with no light source now reads genuinely dark and requires a
  torch. A moonless open field is kept faintly visible ABOVE this floor via
  `vox_core::NIGHT_SKY_MIN` (skylight × sky_scale), not by the floor. The floor
  lives in two synced spots: `vox_mesh::light_curve_f` and the WGSL
  `light_curve()`; the falloff exponent (1.8) lives with them. Keep them in
  sync.

## Hard-won debugging lessons (M05 saga — read before any lighting work)

1. **A fix ships only after a reproduction test that FAILS on current code.**
   Multiple confident fixes shipped during M05 changed nothing in-game
   because they were built against passing tests instead of a failing one.
   The bug that ended the saga was found the same day this rule was applied.
2. **Isolated unit tests cannot validate multi-chunk systems.** Every M05
   root cause lived in interactions: chunk seams, async load order, the
   relight/mesh pipeline. Write tests that model the seam and the streaming
   order (feed real neighbor planes, vary load order), not just one chunk.
3. **When in-game behavior contradicts passing tests, believe the game.**
   The gap IS the diagnosis: the tests are feeding inputs the game doesn't.
   Instrument the live game (temporary probes in `break_block` logging via
   RUST_LOG worked well: VPROBE/SRCPROBE/PLANEDUMP pattern) instead of
   theorizing another round. Remove probes before commit.
4. **Take the user's bug reports literally.** Twice during M05, real bugs
   were rationalized as "actually correct behavior" (horizontal light spread,
   thin-terrain bleed). If Nathan says a sealed hole is lit, a sealed hole is
   lit; find the mechanism.
5. **"The recompute produces the same wrong value" is a diagnosis, not a
   dead end.** If forced relights change nothing, either the inputs are
   self-consistently contaminated or the computation itself has a
   path-fabricating flaw — check what the algorithm treats as traversable.
6. **Measure performance under streaming, not standing still**, and keep
   per-chunk-load work near O(1): no scans of all loaded chunks or all
   columns per event. Uniform chunks are ~86% of the stream — fast-path them
   (relight, column heights), and verify the fast path against the slow path
   in tests.
