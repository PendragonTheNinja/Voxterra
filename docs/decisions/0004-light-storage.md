# ADR-0004: Voxel light storage and propagation

- **Status:** Accepted
- **Date:** 2026-06-15
- **Context:** Milestone 04 (block light), forward-looking to M05 (skylight).

## Context

Voxels need a light level for shading. Minecraft-style lighting uses a small
integer level (0..=15) per voxel per channel, propagated by BFS flood-fill,
baked into the mesh. M04 implements **block light** (emitted by blocks); M05
will add **skylight** (a second channel) and solve cubic-chunk sky
propagation. The storage and algorithm chosen now must let skylight slot in
without reshaping anything.

## Decision

### Storage: one packed `u8` per voxel, allocated lazily per chunk

Each voxel's light is a single byte: **high nibble = sky light (0..=15),
low nibble = block light (0..=15)**. M04 uses only the low nibble; M05 fills
the high nibble with zero storage change.

Per chunk, light is `Option<Box<[u8; CHUNK_VOLUME]>>`:

- `None` means **fully dark** (all voxels 0/0) — the common case for the
  vast majority of streamed chunks (no nearby emitters, and no skylight yet).
  Costs nothing, mirroring the uniform-block fast path.
- The array is allocated the first time any voxel is set to nonzero light,
  and may be released back to `None` when light returns entirely to zero
  (a later optimization; not required for correctness).

Light is **runtime-derived state, never serialized.** It is recomputed from
the (persisted) blocks when a chunk is loaded or generated. Consequences:
the chunk save format is unchanged (no `CHUNK_FORMAT_VERSION` bump), and
light can never be stale relative to the blocks it derives from. Setting
light does **not** mark a chunk `modified` (only block edits do).

### Propagation: BFS flood-fill, two-phase removal

- **Add:** a source seeds its emission level; BFS spreads into non-solid
  neighbors, each step one level dimmer, stopping at 0 and at solid blocks.
  Overlapping sources take the **max** (brightest wins), never the sum.
- **Remove:** the standard two-phase algorithm — (1) from the removed
  source, clear voxels whose light could only have come from it (their level
  is exactly one less than the cell being cleared), queueing border voxels
  where brighter light remains; (2) re-propagate from those borders. This
  avoids both stale glow and over-darkening, and is bounded to the affected
  region rather than rebuilding the world.

The engine is **headless and accessor-driven** (same shape as the mesher):
it takes borrowed `is_solid(pos)` and `emission(pos)` closures plus mutable
light access, so it depends on neither the registry nor the `World` type and
is fully unit-testable.

## Consequences

- **M05 skylight is a storage no-op:** it uses the high nibble and a second
  propagation pass; the byte layout, the `Option<Box<…>>` scheme, and the
  recompute-on-load policy all carry over unchanged.
- Memory: a lit chunk costs `CHUNK_VOLUME` bytes (32 KiB). Only chunks that
  actually contain light pay this; dark chunks stay `None`. Acceptable at
  current loaded-chunk counts; revisit with a measurement if it grows.
- The mesher gains light input: it reads the per-face light level and bakes
  it into the existing `brightness` vertex scalar (combination with the
  current directional face shading decided in the M04 task-4 work).
- Recompute-on-load means loading/generating a chunk must trigger
  (re)lighting before it meshes (wired in M04 task 4). A burst of edits or
  loads must keep light work within the per-frame budgets so it can't stall
  the frame.
- Single scalar per channel: **no colored light** (RGB would need 3× storage
  and is out of scope). If ever wanted, it's a separate future decision.
