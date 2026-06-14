# Milestone 02 — Infinite World

- **Status:**  COMPLETE (2026-06-14)
- **Spec written:** 2026-06-13
- **Depends on:** Milestone 01, ADR-0001, ADR-0002

## Goal

Turn the fixed 4,096-chunk demo into a genuinely unbounded, explorable world:
chunks stream in and out around the camera on all three axes, generation and
meshing happen off the main thread so movement never stalls, the world
renders camera-relative so it stays crisp arbitrarily far from origin
(ADR-0002), and edits persist to disk in a versioned format.

**Acceptance scene:** spawn into a freshly generated world, fly in any
direction (including straight up/down) indefinitely; chunks load ahead and
unload behind with no hitching and a stable frame rate; punch some holes,
fly far away until those chunks unload, fly back, and the holes are still
there (loaded from disk); travel to ~1,000,000 blocks from origin and confirm
the world renders without jitter, z-fighting, or cracks.

## Acceptance criteria

1. **Floating-origin rendering (ADR-0002).** Vertices upload in local 0..32
   space; per-chunk offset relative to a camera-tracking render origin is
   applied at draw time. `offset_mesh` is removed. Verified: fly to
   `x = 1_000_000` and observe no shimmer/z-fighting/cracks; the same scene
   at origin and at 1e6 looks identically crisp. Simulation coordinates
   remain exact `i64` — a test (or debug assert) confirms no camera-relative
   value ever enters `World`/`WorldPos` space.
2. **Streaming by distance.** A configurable **load radius** (in chunks,
   applied on all three axes) around the camera's chunk. Chunks within radius
   are present; chunks beyond an **unload radius** (slightly larger, for
   hysteresis — never load/unload the same chunk twice in adjacent frames)
   are removed. Moving the camera continuously loads the leading edge and
   unloads the trailing edge.
3. **Asynchronous generation + meshing.** Generation and meshing run on the
   rayon pool (or a dedicated worker pool); the main thread enqueues requests
   for newly-in-range chunks and consumes finished results, uploading a
   bounded number of meshes per frame to avoid upload spikes. Flying through
   the world must not stall the render loop — frame time stays stable while
   chunks stream. A chunk whose neighbors aren't loaded yet is meshed with
   the neighbors it has (the existing `ChunkNeighbors` `None` = air rule);
   when a neighbor later loads, the border chunk is re-meshed (dirty-set +
   neighbor-expansion pattern from M01 task 6).
4. **Versioned save/load.** A world lives in a directory on disk. Chunks are
   persisted in a **versioned** binary format (format byte/version field
   from the first write — non-negotiable, CLAUDE.md). Serialization uses the
   palette representation and the Y-major `LocalPos::index` ordering (the
   ordering contract established in M01 — do NOT diverge). Save/load
   round-trips losslessly (test: random-edited chunk → serialize → deserialize
   → byte-identical). Uniform chunks serialize compactly (palette of one, no
   index array).
5. **Generate-or-load policy.** On entering range, a chunk is **loaded from
   disk if a saved version exists, otherwise generated** from the seed.
   Generated-but-unmodified chunks need not be written to disk (they're
   reproducible from the seed); only **modified** chunks must persist. A
   chunk tracks whether it has been edited since generation (a dirty/modified
   flag) to decide whether it needs saving on unload.
6. **Persistence across unload/reload.** The acceptance-scene guarantee: edit
   a chunk, fly far enough that it unloads (and, if modified, saves), fly
   back, and the edit is present. This exercises the full
   generate-or-load + save-on-unload loop.
7. **World metadata.** The world directory stores at least the **seed** and
   the **format version** in a small metadata file, so a saved world reopens
   deterministically. (Player position, time-of-day, etc. may be added but
   are not required this milestone.)
8. **Bounded resource use.** Loaded-chunk count stays bounded by the load
   radius regardless of how far the player has traveled (no leak of unloaded
   chunks). A debug log line reports loaded-chunk count and
   pending-gen/pending-mesh queue depths.
9. **Telemetry.** Extend the existing per-second log: fps, drawn/total,
   loaded chunk count, and streaming queue depths. Record steady-state
   numbers in the retrospective as the new baseline.
10. `cargo test` green workspace-wide; clippy/fmt clean. Headless logic
    (streaming policy math, serialization round-trip, modified-flag
    behavior) is unit-tested in the core/IO crates, not only observed in
    vox-app.

## Non-goals (explicitly out of scope)

- Real geological worldgen (still placeholder heightmap; geology is its own
  milestone after lighting/textures).
- LOD / Distant Horizons-style distant rendering. Streaming gives a moving
  window of *full-resolution* chunks; the LOD octree that lets the load
  radius effectively shrink while render distance grows is a **later
  milestone** that builds on this one. (Design streaming so an LOD layer can
  sit on top — see notes.)
- Lighting, textures, sky, entities, physics/collision, player gravity.
- Multiplayer. (Save format and the exact/relative split should not make
  networking *harder*, but networking is not designed here.)
- Compression of the save format beyond the palette representation (can be
  added under the version field later).
- Multi-region/file-packing optimizations (one-file-per-chunk or a simple
  region scheme is acceptable; choose in implementation, document the
  choice).

## Suggested task breakdown

1. **Floating-origin rendering (ADR-0002).** Shader gains a per-chunk
   `chunk_offset`; renderer computes it from `ChunkPos − render_origin` in
   `i64`, narrows to `f32`; remove `offset_mesh`; build view matrix at the
   render origin; pick render-origin update policy (chunk-crossing
   threshold). Verify crispness at 1e6. Do this FIRST — it changes the vertex
   path that everything else uploads through.
2. **Streaming core (headless).** A `StreamingWorld` (or extend `World`) that,
   given a camera chunk position + radii, computes the set of chunks to load
   and to unload (hysteresis). Pure set math — unit-test it without graphics.
3. **Async gen+mesh pipeline.** Worker pool generates/meshes requested
   chunks; main thread drains results and uploads ≤N meshes/frame; integrate
   with streaming core. Frame stays smooth under motion.
4. **Serialization (headless, versioned).** Chunk (de)serialize with version
   field + palette + Y-major order; round-trip tests including uniform and
   heavily-edited chunks.
5. **World directory + metadata + generate-or-load + save-on-unload.** Wire
   serialization to the streaming loop; modified-flag on chunks; persistence
   round-trip across unload/reload.
6. **Telemetry + measure.** Extend the log; record baseline (load/unload
   rates, queue depths, fps at travel speed, memory) in the retrospective.

## Notes for whoever builds this (human or model)

- **Exact simulation, relative rendering** (ADR-0002) is the invariant that
  must not erode: `World`/`WorldPos` stay `i64`; the render origin is a
  rendering-only concept and must never reach game logic, save data, or
  worldgen.
- The serialized chunk format's byte order is the Y-major `LocalPos::index`
  order — the SAME ordering already load-bearing in palette storage and
  meshing. A version field is mandatory from the first byte written.
- Only **modified** chunks need saving; unmodified generated chunks are
  reproducible from the seed. This keeps the save directory small and makes
  "regenerate vs. load" the default cheap path. Get the modified-flag right —
  it's the thing that decides correctness of persistence.
- Streaming and the future LOD octree are partners: structure the streaming
  window so a coarser LOD layer can later wrap it (full-res near, downsampled
  far) without reworking the load/unload core. Don't build LOD now; just
  don't wall it out.
- Hysteresis (separate load vs. unload radii) is not optional — without it,
  a camera sitting on a chunk boundary will thrash chunks load/unload every
  frame.
- Bound the per-frame mesh uploads; a naive "upload everything ready this
  frame" causes hitches exactly when streaming is busiest.
- When this milestone is accepted: update CLAUDE.md status, write the
  retrospective with new baseline numbers, then write `03-…md` (block
  interaction + lighting + textures, per the M01 roadmap) before any
  Milestone 03 code.

Retrospective (2026-06-14)

Status: COMPLETE. All six tasks landed; all acceptance criteria met.
Voxterra is now a genuinely unbounded, persistent, streamed world.

Measured baseline (record for future comparison)

Dev machine: Windows, NVIDIA GPU, 20 logical cores. LOAD_RADIUS = 8,
UNLOAD_RADIUS = 10 chunks. GEN_SPAWN_BUDGET = 64, MESH_BUDGET = 48.

Steady state, standing still:


fps: 144 (vsync-capped; not an engine ceiling).
loaded chunks: ~2,671 (bounded; sits between the load and unload
spheres, as hysteresis intends).
non-empty meshes (total): ~520–550 of those 2,671 (the rest are
uniform air/stone → no mesh, the fast path doing its job).
drawn after frustum cull: ~110–180 of ~520 (frustum culling working).


Fast flight (Ctrl-sprint, sustained):


fps: holds 142–145, no hitching or drops observed.
gen queue: blips to ~60 when crossing into new regions, else 0 —
async generation keeps ahead of travel.
dirty queue: spikes to a few hundred when chunks stream in, drains
back to 0 within 1–2 frames. Bounded, never accumulates (see leak note).


Far travel: world stays crisp at large coordinates (floating origin,
ADR-0002); fps unaffected. Persistence verified: edits survive
unload→reload and full app restart.

Telemetry caught a real bug (the headline lesson)

Task 6's telemetry immediately exposed a dirty-set leak: the per-frame
mesh batch filtered out non-resident "ghost" dirty entries but never removed
them, so the set grew unbounded (observed climbing past 16,000 during a ~2
min flight while the loaded set stayed ~2,671). It was invisible to
rendering — fps and visuals were perfect — which is exactly why a metric was
needed to see it. Fixed by pruning non-resident entries each frame
(dirty.retain(|p| world.chunk(p).is_some())); a non-resident chunk has no
mesh to build and gets re-marked dirty if it later generates. Verified in a
sandbox sim (200 chunk-steps: 13,001 leaked → 2,061 bounded) and on the dev
machine (dirty now spikes-and-drains to 0). This validated building
telemetry as its own task: the first real measurement found something.

What went well


The exact/relative coordinate split (ADR-0002) held cleanly: i64
everywhere in simulation, camera-relative only at draw. No precision
issues at distance, no leakage of the render origin into game logic.
ChunkNeighbors-by-borrow (M01) again paid off — async generation +
bounded-parallel meshing fell out without sharing the World across
threads or cloning chunk data.
The "only modified chunks persist" design keeps saves tiny: an explored
but unedited world writes almost nothing to disk.
Headless verification covered the high-risk logic (streaming policy,
serialization round-trip, the full generate→edit→save→reload loop, and
the leak fix) before any of it touched the dev machine.


Decisions worth remembering


Meshing is bounded-parallel-per-frame, not fire-and-forget. It borrows
the World immutably via mesh_chunks_parallel (all cores), capped at
MESH_BUDGET/frame. This honors "runs on the rayon pool, bounded, no
stall" without cloning 7 chunks per mesh job or locking the World. If a
future workload makes per-frame meshing too bursty, revisit toward true
async with snapshotting — but only with a measurement showing the need.
Storage lives in vox-core's storage module, not a vox-io crate
(yet). Thin wrapper over Chunk::serialize; graduate to its own crate if
region files / compression / threading make it grow.


Carried into Milestone 03


The dirty-set + neighbor-expansion pattern is now the universal re-mesh
path (streaming, edits, and — next — lighting updates will all feed it).
MESH_BUDGET / GEN_SPAWN_BUDGET are the per-frame throttles; any new
per-frame work (lighting propagation, etc.) should be budgeted the same
way and surfaced in telemetry.
The chunk serialization format has a version byte: adding lighting data or
block-state to chunks means bumping CHUNK_FORMAT_VERSION and handling v1.
Block IDs are still the placeholder STONE/DIRT/GRASS trio mirrored between
vox-worldgen and vox-app. A real block registry is overdue and should come
with (or before) textures in M03.