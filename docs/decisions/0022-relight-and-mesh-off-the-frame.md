# ADR-0022: Relight and mesh results arrive over channels

- **Status:** Proposed (2026-11-10). **Not to be built until the owner's
  `--bench` on the audited build says it is needed** — see *The gate*.
  Backlog package P1; audit finding P1.
- **Context:** Each frame the session hands a batch of chunks to a thread
  pool to relight, waits for it, applies the results, then does the same
  for meshing (`Session::tick`; `relight_chunks_parallel`,
  `mesh_chunks_parallel`). The 2026-11 audit removed the worst of the
  waiting — those batches used to queue behind background generation — but
  the frame still blocks for as long as its own batches take, under budgets
  (3 ms relight, 4 ms mesh) that are only checked *between* sub-batches, so
  one heavy sub-batch overruns them. Generation and LOD building never
  block the frame: their jobs take what they need by value and send results
  back on a channel. This ADR gives relight and mesh the same shape.

## The gate

"Optimise from measurements" (CLAUDE.md). What this would buy on a real GPU
is not known: the only numbers since the audit are from a software
rasteriser at 5 fps. The bench now separates work from waiting. On the
owner's machine, in the sprint phases:

- **wall** = `Relight ms/s` + `Mesh ms/s` — what the frame thread spends
  in the two passes each second;
- **work** = (`Relit /s` × `Relight µs/chunk` + `Meshed /s` × `Mesh
  µs/chunk`) ÷ 1 000 ÷ cores — what it would spend if the pool were its
  alone.

Build this if wall is more than a few percent of the second *and* the
worst-frame or 1%-low columns show it (a blocked batch is a long frame).
If wall is ~20 ms/s at 700 fps, this is a large change for a result nobody
could see, and the performance milestone's GPU work (P2–P5) comes first.

## Decision (proposed)

1. **Chunks are shared, not borrowed.** `World` holds `Arc<Chunk>`. A job
   takes clones of the `Arc`s it reads (its chunk and neighbours); an edit
   or a light write goes through `Arc::make_mut`, which copies the chunk
   only if a job still holds it. No job ever sees a chunk change under it,
   and the frame never waits for a job to let go.
2. **A relight job** takes its chunk, its six face neighbours, the
   heightmap planes it needs (`sky_top`, and `sky_plane_above` where the
   +Y neighbour is sealed — both 1 KB, read on the frame thread when the
   job is made, because the heightmap is the frame's), and returns the
   light buffer with the CPU time it took.
3. **A mesh job** takes its chunk, its 26 neighbours and its sealed mask,
   builds the 34³ snapshot on the worker (today the frame's pool does),
   and returns the mesh.
4. **Results are applied by the frame thread**, as many per frame as a
   time budget allows, nearest first.
5. **Every chunk carries two counters**, bumped by the frame thread:
   `blocks_rev` on any block change, `light_rev` on any light change. A job
   records the revisions of everything it read. On arrival:
   - a relight result whose own `blocks_rev` has moved is dropped (the
     edit re-queued the chunk);
   - otherwise it is applied, even if a neighbour's light has moved since
     — the neighbour's border change has already re-queued this chunk, and
     applying the slightly stale light now keeps the world converging
     instead of starving under a stream of changes;
   - a mesh result is applied unless its chunk has unloaded; if anything
     it read has moved, the chunk is already dirty again and will be
     meshed again.
6. **In flight counts as queued.** A chunk with a relight job in flight is
   not handed to the mesher ("never mesh a chunk that is still queued for
   relight" — CLAUDE.md), and a chunk re-queued while its job is in flight
   gets a fresh job when that one lands, never two at once.
7. **Bounded by jobs in flight**, not by time: at most a few per worker
   outstanding, so the nearest-first order stays meaningful and a camera
   that turns round is not waiting on a hundred jobs for where it was.

## What must not change

Each of these has a test in `session::tests` or `vox-core`, and every one
must pass unmodified:

- the final world is the same whatever order results arrive in
  (`arrival_order_does_not_change_the_world`) — and to this ADR's point,
  that test gains a delivery mode for *relight and mesh results* arriving
  late and shuffled, as it has for generated chunks;
- a risen column relights its stack; an unknown column is covered; the
  border ring never conducts (`the_sea_surface_…`, `a_deep_shaft_…`);
- a changed border re-queues and re-meshes its neighbours, edge
  neighbours included;
- the first-mesh gate, and nearest-first.

## Costs

- A copy of a chunk (10–60 KB) whenever light is applied to one a mesh
  job is still reading. Measure it; if it shows, light moves out of
  `Chunk` into its own `Arc` so blocks and light copy separately.
- `World::chunk_mut` becomes copy-on-write, which every caller must
  understand: holding an `Arc<Chunk>` across a frame is now a cost, not a
  bug.
- More states per chunk (queued, in flight, in flight and re-queued), in
  the code where the M05 bugs lived. The session tests are what make this
  affordable; it should not have been attempted before them.
