# Milestone 02 — Infinite World

- **Status:** Spec accepted, not started
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
