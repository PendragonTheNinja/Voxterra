//! The world session: everything that streams, lights and meshes the world,
//! with no window, no GPU and no input.
//!
//! [`Session`] owns the [`World`] and all the bookkeeping around it — the
//! streamer, the skylight heightmap, the relight and mesh queues, the
//! generation channel, the saved-edit state — and [`Session::tick`] is one
//! frame of it: unload, request, receive, relight, mesh. Meshes leave through
//! a callback; nothing here knows what a renderer is.
//!
//! ## Why it is separate (2026-11 audit, A1)
//!
//! Until the audit all of this was methods on `App`, which cannot exist
//! without a window. So the code where every hard bug of this project has
//! lived — chunks arriving in any order, light crossing seams, the mesh
//! waiting on the light (CLAUDE.md, *Hard-won debugging lessons*) — could
//! only be exercised by playing. Lesson 2 says a unit test of one chunk
//! cannot validate a multi-chunk system; there was nowhere to write the
//! other kind.
//!
//! Here there is. The tests at the end of this file fly a scripted camera
//! through a real generated world and deliver generated chunks in whatever
//! order they choose — in order, shuffled, late — and then ask what a player
//! would see: is every chunk lit and meshed, is the light the same whichever
//! order things arrived in, did an edit survive a save.
//!
//! The move was mechanical: the method bodies are `App`'s, unchanged but for
//! the renderer calls, which became the `sink` callback at the same points
//! (so mesh uploads still count against the mesh pass's time budget, as
//! they did). This module is the seed of the planned client/server split
//! (CLAUDE.md, *Workspace layout*); it lives in `vox-app` until a milestone
//! spec calls for the crate.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use vox_core::{
    BlockId, BlockRegistry, Chunk, ChunkPos, ColumnHeights, EditedColumns, LocalPos, Streamer,
    World, WorldPos, WorldStore,
};
use vox_mesh::{mesh_chunk, ChunkNeighbors, MeshData};
use vox_worldgen::Generator;

/// A unit of background work: load or generate one chunk and send it back.
pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

/// Where a session's generation jobs run.
pub(crate) enum Spawner {
    /// On the background thread pool ([`background_pool`]): the game.
    Pool(Arc<rayon::ThreadPool>),
    /// Into a queue the test runs by hand, in whatever order it likes —
    /// which is how a test controls the one thing the game cannot: the
    /// order chunks arrive in.
    #[cfg(test)]
    Queue(Arc<std::sync::Mutex<Vec<(ChunkPos, Job)>>>),
}

impl Spawner {
    /// Run `job`, which loads or generates the chunk at `pos`.
    fn spawn(&self, pos: ChunkPos, job: Job) {
        match self {
            Spawner::Pool(pool) => {
                let _ = pos;
                pool.spawn(job)
            }
            #[cfg(test)]
            Spawner::Queue(queue) => queue.lock().expect("job queue poisoned").push((pos, job)),
        }
    }
}

// The six face-neighbor offsets, shared by streaming and edit re-meshing.
pub(crate) const NEIGHBOR_OFFSETS: [(i64, i64, i64); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Chunk layers kept loaded below and above each column's terrain surface
/// (M10 task 2).
///
/// Streaming follows the surface rather than filling a world-height band,
/// because M10 opens the world to ~640 chunk layers and a full-height cylinder
/// at that scale is ~80x more chunks than the engine can carry. Below covers
/// standing on and digging into the ground; above covers building and the
/// near-ground flight envelope. Fly far higher than `ABOVE` and the terrain
/// below you stays loaded — this window follows the GROUND.
pub(crate) const LOAD_BELOW_CHUNKS: i64 = 3;

pub(crate) const LOAD_ABOVE_CHUNKS: i64 = 3;

/// The camera window (M10 A3): chunk layers kept above and below the
/// CAMERA's layer, in columns within `CAMERA_WINDOW_RADIUS` of it.
///
/// The surface window alone strands a player who leaves it: dig more than
/// `LOAD_BELOW_CHUNKS` down or build more than `LOAD_ABOVE_CHUNKS` up and you
/// walk out of the loaded world. Near the ground this window overlaps the
/// surface one and costs nothing; underground it is ~100 blocks each way.
///
/// The radius is the player's working neighbourhood, not a view distance.
/// Spanning the whole load disc, the window streamed ~1 400 empty chunks
/// through generation, lighting and meshing for a spectator in the sky; at 3
/// chunks it is ~200. A build above the terrain window is visible while the
/// player is near it — the same as before the window existed, from further.
pub(crate) const CAMERA_WINDOW_LAYERS: i64 = 3;

pub(crate) const CAMERA_WINDOW_RADIUS: i64 = 3;

/// Per-frame work budgets, to keep frame time stable while streaming.
/// Generation is async (rayon), but we bound how many jobs we *spawn* per
/// frame so a big initial load fills in progressively rather than spiking.
pub(crate) const GEN_SPAWN_BUDGET: usize = 64;

/// Meshing runs bounded-parallel on the main thread each frame (borrows the
/// World immutably; see the streaming-tick comment). Meshing and relighting
/// are time-budgeted per frame (not capped to a fixed chunk count), so a burst
/// of generation can't blow the frame: each pass runs parallel sub-batches
/// until its time cap, then yields. Heavy chunks → fewer per frame, light
/// chunks → more; frame time stays bounded.
// Sub-batch sizes are deliberately small: the time-cap check runs BETWEEN
// batches, so one batch is the budget-overshoot unit. A mesh batch's GPU
// uploads run serially on the main thread inside the loop — 24 chunks per
// batch could triple the 4ms cap in a single iteration (the streaming-burst
// fps dips). Smaller batches trade a little rayon efficiency for caps that
// hold, which is exactly what "smooth while streaming" means.
pub(crate) const MESH_SUBBATCH: usize = 8;

pub(crate) const RELIGHT_SUBBATCH: usize = 32;

pub(crate) const MESH_TIME_MS: f32 = 4.0;

pub(crate) const RELIGHT_TIME_MS: f32 = 3.0;

/// Chunk side length as i64, for heightmap/world-coordinate math.
pub(crate) const CHUNK_SIZE_I: i64 = vox_core::CHUNK_SIZE as i64;

/// A chunk's heightmap-derived `top_sky` (CHUNK_SIZE², daylight entering each
/// column from directly above: 15 open, 0 occluded). Free fn so it runs inside
/// the parallel relight workers rather than serially on the main thread.
///
/// Full daylight enters a column at the chunk's TOP FACE only when the column
/// is KNOWN and open through the entire chunk — its highest solid lies below
/// the chunk's floor. When the surface lies INSIDE this chunk the top stays 0:
/// daylight then enters from the chunk ABOVE via its -Y sky plane and floods
/// down only through actual air, so a sealed shaft below the surface stays
/// dark (the cave-lit bug). And an UNKNOWN column — its solid chunks not yet
/// streamed in — is COVERED, never open: assuming "open" injects daylight that
/// the uniform-air fast path commits and nothing corrects (the sealed-hole
/// leak). Dark-then-brighten is always safe. `ColumnHeights::sky_top` applies
/// exactly that rule.
pub(crate) fn top_sky_from_heightmap(heights: &ColumnHeights, pos: ChunkPos) -> Vec<u8> {
    heights.sky_top(pos)
}

/// The thread pool for chunk generation and LOD node building.
///
/// Those jobs used to be `rayon::spawn`ed onto the GLOBAL pool — the same
/// pool the frame thread hands its relight and mesh batches to and then
/// waits on. A batch submitted from outside the pool joins rayon's queue
/// BEHIND everything already spawned, so each frame's relight waited for up
/// to a hundred generation and LOD jobs (each ~1–7 ms) to be picked up
/// first. Measured headless (2026-11 audit, 4 cores, dev build): a
/// 32-chunk relight batch took 6 ms on an idle pool and 43 ms behind one
/// frame's worth of spawns — a stall the telemetry booked as relight time,
/// since it reports the batch's wall clock.
///
/// With its own pool, background work is scheduled against the frame's
/// work by the operating system instead of ahead of it. It gets two
/// threads fewer than the machine has cores, at least one: the same
/// measurement gave 11 ms for that batch beside a 2-thread background
/// pool, against 22–26 ms beside a 3- or 4-thread one.
pub(crate) fn background_pool() -> rayon::ThreadPool {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let threads = cores.saturating_sub(2).max(1);
    log::info!("background pool: {threads} threads for generation and LOD ({cores} cores)");
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("vox-background-{i}"))
        .build()
        .expect("failed to start the background thread pool")
}

/// Compute both light channels for many chunks in parallel (rayon),
/// read-only. Each worker builds its own heightmap-derived `top_sky` (so that
/// work is parallel, not serial), reads neighbor borders, and returns the
/// packed buffer plus `interior_changed` (→ needs re-mesh) and
/// `border_changed` (→ neighbors need a relight check). Skylight's vertical
/// fill comes from `top_sky`, so there's no dependence on the +Y neighbor
/// being relit first — no vertical cascade (ADR-0005).
///
/// `jobs` pairs each chunk with its SEALED faces, the same mask the mesher
/// gets (`sealed_faces`). A sealed +Y neighbour — absent and never coming —
/// supplies its sky plane from the heightmap instead of nothing: over deep
/// ocean only the seabed and the sea surface are resident, and the seabed
/// would otherwise be dark under water that passes light (M10 A1).
///
/// Also returns the summed CPU time of the jobs, milliseconds, as
/// `mesh_chunks_parallel` does. The pass's wall time — the telemetry's `lt`
/// — includes whatever the frame thread waited for, and for two milestones
/// that waiting was read as lighting getting slower (2026-11 audit).
#[allow(clippy::type_complexity)]
pub(crate) fn relight_chunks_parallel(
    world: &World,
    registry: &BlockRegistry,
    heights: &ColumnHeights,
    jobs: &[(ChunkPos, u8)],
) -> (Vec<(ChunkPos, Vec<u8>, bool, u8)>, f64) {
    const OPPOSITE: [usize; 6] = [1, 0, 3, 2, 5, 4];
    const POS_Y: usize = 2; // NEIGHBOR_OFFSETS[2] == (0, 1, 0)
                            // The workers' own time, summed: what relighting COSTS, as against how
                            // long the frame waited for it (see the return value's docs).
    let cpu_nanos = std::sync::atomic::AtomicU64::new(0);
    let lit = jobs
        .par_iter()
        .filter_map(|&(pos, sealed)| {
            let started = Instant::now();
            let chunk = world.chunk(pos)?;
            let top_sky = top_sky_from_heightmap(heights, pos);
            let mut block_borders: vox_core::NeighborLight = [None, None, None, None, None, None];
            let mut sky_borders: vox_core::NeighborSky = [None, None, None, None, None, None];
            for (face, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
                let npos = ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                if let Some(n) = world.chunk(npos) {
                    block_borders[face] = Some(vox_core::chunk_light_plane(n, OPPOSITE[face]));
                    // Pass all sky borders EXCEPT -Y (face 3): sky never flows
                    // up. The +Y face (2) carries real daylight DOWN from the
                    // chunk above (shafts/caves below the natural surface); the
                    // heightmap top_sky handles the open-air region above it.
                    if face != 3 {
                        sky_borders[face] = Some(vox_core::chunk_sky_plane(n, OPPOSITE[face]));
                    }
                } else if face == POS_Y && sealed & (1 << POS_Y) != 0 {
                    sky_borders[face] = Some(heights.sky_plane_above(pos));
                }
            }
            let (light, border_changed) = vox_core::compute_chunk_light_2ch(
                chunk,
                registry,
                &block_borders,
                &sky_borders,
                &top_sky,
                true,
            );
            let interior_changed = !chunk.light_matches(&light);
            // Which faces actually changed — so the consumer requeues only the
            // neighbor across a changed face, not all six (up to 6x less
            // relight/remesh cascade during streaming convergence).
            let border_faces = if border_changed {
                vox_core::packed_border_changed_faces(chunk, &light)
            } else {
                0
            };
            cpu_nanos.fetch_add(
                started.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            Some((pos, light, interior_changed, border_faces))
        })
        .collect();
    let cpu_ms = cpu_nanos.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1.0e6;
    (lit, cpu_ms)
}

/// `jobs` pairs each chunk with its SEALED faces: a bit per entry of
/// `NEIGHBOR_OFFSETS`, set where that face neighbour is absent and never
/// coming (M10 A3). The mesher continues the chunk's own edge there: no face
/// into the void, and edge light and AO match the interior instead of fading
/// toward an unlit nothing (the dark line around the loaded disc). The
/// judgement needs the streamer, so the caller makes it on the main thread;
/// the workers only read it.
///
/// Also returns the summed CPU time of the jobs, milliseconds: the wall time
/// of the pass hides how much work the workers did, and per-chunk cost is
/// what the M12 profile optimises.
pub(crate) fn mesh_chunks_parallel(
    world: &World,
    registry: &BlockRegistry,
    jobs: &[(ChunkPos, u8)],
) -> (Vec<(ChunkPos, MeshData)>, f64) {
    let results: Vec<(Option<(ChunkPos, MeshData)>, f64)> = jobs
        .par_iter()
        .map(|&job| {
            let start = Instant::now();
            let mesh = mesh_one_chunk(world, registry, job);
            (mesh, start.elapsed().as_secs_f64() * 1000.0)
        })
        .collect();
    let cpu_ms = results.iter().map(|(_, ms)| ms).sum();
    (results.into_iter().filter_map(|(m, _)| m).collect(), cpu_ms)
}

pub(crate) fn mesh_one_chunk(
    world: &World,
    registry: &BlockRegistry,
    (pos, sealed): (ChunkPos, u8),
) -> Option<(ChunkPos, MeshData)> {
    let chunk = world.chunk(pos)?;
    let mut neighbors = ChunkNeighbors::of(world, pos);
    for (bit, &(dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
        if sealed & (1 << bit) != 0 {
            neighbors = neighbors.with_sealed(dx, dy, dz);
        }
    }
    // Texture array (ADR-0003): resolve each face's layer via the
    // registry. The closure borrows the registry (Sync), shared
    // across the rayon workers.
    // VISUAL: faces cull against OPAQUE neighbours (ADR-0011).
    let mesh = mesh_chunk(
        chunk,
        &neighbors,
        |b, face| registry.face_layer(b, face),
        |b| registry.is_opaque(b),
    );
    if mesh.is_empty() {
        None
    } else {
        Some((pos, mesh))
    }
}

/// The resident world and the pipeline that keeps it loaded, lit and meshed
/// around a camera. See the module docs.
pub(crate) struct Session {
    /// Where generation jobs run.
    spawner: Spawner,
    pub(crate) world: World,
    // --- Streaming (M02 task 3) ---
    pub(crate) generator: Generator,
    pub(crate) streamer: Streamer,
    /// Surface spans the streamer has asked for, kept from frame to frame:
    /// re-sampling every column of the disc each frame was ~0.2 ms in flight
    /// (M11 task 0).
    pub(crate) spans: vox_core::SpanCache,
    /// Block definitions: appearance + flags, the single source of truth
    /// (M03 task 1). Shared into meshing (Sync).
    pub(crate) registry: BlockRegistry,
    /// On-disk world: generate-or-load on chunk-in, save modified on
    /// chunk-out (M02 task 5).
    pub(crate) store: WorldStore,
    /// Chunks whose generation has been spawned but whose result hasn't been
    /// received yet — prevents re-spawning the same chunk every frame.
    pub(crate) gen_in_flight: HashSet<ChunkPos>,
    /// Chunks needing a (re)mesh: newly generated chunks and any loaded
    /// neighbor whose border faces may have changed.
    pub(crate) dirty: HashSet<ChunkPos>,
    /// Chunks needing a light recompute, kept separate from `dirty` so that
    /// cross-chunk light convergence does cheap parallel relighting without
    /// forcing an expensive re-mesh of every rippled chunk (ADR-0005).
    pub(crate) relight: HashSet<ChunkPos>,
    /// Chunks that have been meshed at least once (M09 task 1). A chunk's
    /// FIRST mesh is deferred until its still-pending neighbors arrive, so it
    /// is never baked dark/with wrong border faces from incomplete data; later
    /// re-meshes are unrestricted. Pruned with the other per-chunk sets.
    pub(crate) meshed_once: HashSet<ChunkPos>,
    /// Per-column heightmap: world `(x, z)` → highest solid block world-Y
    /// among loaded chunks. Drives the skylight top boundary directly, so each
    /// chunk computes its daylight in one pass without waiting on its vertical
    /// neighbors to be relit (avoids the skylight cascade; ADR-0005). Unknown
    /// column = COVERED. Bounded by residency: a chunk column's heights are
    /// dropped with its last resident chunk (M10 A3), so every insert into and
    /// removal from `world` must be reported to it.
    pub(crate) column_heights: ColumnHeights,
    /// Columns the player has dug or built, as a sparse overlay on the seed
    /// heights every LOD node starts from (M09 ghost-block fix).
    ///
    /// `column_heights` cannot serve this: it only knows columns whose chunks
    /// have loaded, and consulting it per column would cost 32x32xstride^2
    /// lookups per node (65 536 at stride 8), nearly all misses. Shared into
    /// the rayon build jobs behind an `Arc` — edits are rare, node builds are
    /// constant, so copy-on-write is the cheap direction. Loaded from and saved
    /// with the world, so earlier sessions' edits reach the LOD too.
    pub(crate) edited_columns: Arc<EditedColumns>,
    /// `edited_columns` has changed since it was last saved. Flushed wherever
    /// chunks are saved, so the overlay on disk is never behind the chunks it
    /// describes.
    pub(crate) edits_unsaved: bool,
    /// Async generation results arrive here from the rayon pool.
    pub(crate) gen_tx: Sender<(ChunkPos, Chunk)>,
    pub(crate) gen_rx: Receiver<(ChunkPos, Chunk)>,
    /// Milliseconds spent in the relight / mesh streaming passes since the
    /// last telemetry line — shows where frame time goes during bursts.
    pub(crate) relight_ms_accum: f32,
    pub(crate) mesh_ms_accum: f32,
    /// Relight and mesh time since startup, never reset — the benchmark
    /// takes differences across a phase.
    pub(crate) relight_ms_total: f64,
    pub(crate) mesh_ms_total: f64,
    /// Chunks meshed since startup, and the summed CPU time of their jobs
    /// (`mesh_chunks_parallel`) — the benchmark's per-chunk mesh cost.
    pub(crate) meshed_total: u64,
    pub(crate) mesh_cpu_ms_total: f64,
    /// Chunks relit and the workers' CPU time doing it, as for meshing.
    pub(crate) relit_total: u64,
    pub(crate) relight_cpu_ms_total: f64,
}

impl Session {
    /// A session over an opened world, with nothing resident yet. `store`
    /// and `generator` must be the same world's (the store's seed and
    /// shape); `edited_columns` is that world's saved LOD edit overlay.
    pub(crate) fn new(
        store: WorldStore,
        generator: Generator,
        edited_columns: EditedColumns,
        load_radius: i64,
        spawner: Spawner,
    ) -> Self {
        let (gen_tx, gen_rx) = std::sync::mpsc::channel();
        Self {
            spawner,
            world: World::new(),
            generator,
            // Cylindrical horizontally, surface-following vertically.
            streamer: Streamer::new(Self::stream_config(load_radius)),
            spans: vox_core::SpanCache::new(),
            registry: BlockRegistry::default_set(),
            store,
            gen_in_flight: HashSet::new(),
            dirty: HashSet::new(),
            relight: HashSet::new(),
            meshed_once: HashSet::new(),
            column_heights: ColumnHeights::new(),
            edited_columns: Arc::new(edited_columns),
            edits_unsaved: false,
            gen_tx,
            gen_rx,
            relight_ms_accum: 0.0,
            mesh_ms_accum: 0.0,
            relight_ms_total: 0.0,
            mesh_ms_total: 0.0,
            meshed_total: 0,
            mesh_cpu_ms_total: 0.0,
            relit_total: 0,
            relight_cpu_ms_total: 0.0,
        }
    }

    /// Change one block as the player does: write it, refresh the column's
    /// height both ways (see `recompute_height_column` and
    /// `recompute_column_height` — the two rules of audit finding D1, in the
    /// order the game has always run them), and queue the relight and
    /// re-mesh. The caller still owes the LOD its invalidation: that ring is
    /// not the session's.
    pub(crate) fn set_block(&mut self, pos: WorldPos, block: BlockId) {
        self.world.set_block(pos, block);
        self.recompute_height_column(pos.x, pos.z);
        self.mark_dirty_with_neighbors(pos.chunk());
        self.recompute_column_height(pos.x, pos.z);
    }

    /// Whether nothing is left to do for the camera where it is: no chunk
    /// being generated, none waiting for light or a mesh.
    #[cfg(test)]
    pub(crate) fn is_settled(&self) -> bool {
        self.gen_in_flight.is_empty() && self.dirty.is_empty() && self.relight.is_empty()
    }

    /// Mark a chunk and its six face-neighbors dirty (re-mesh) and queued for
    /// relight. Border faces and border light both depend on neighbors.
    pub(crate) fn mark_dirty_with_neighbors(&mut self, c: ChunkPos) {
        self.dirty.insert(c);
        self.relight.insert(c);
        for (dx, dy, dz) in NEIGHBOR_OFFSETS {
            let n = ChunkPos::new(c.x + dx, c.y + dy, c.z + dz);
            self.dirty.insert(n);
            self.relight.insert(n);
        }
    }

    /// Fold a freshly-loaded/edited chunk's columns into the heightmap. For
    /// each column, raise the stored world-Y to the chunk's highest solid
    /// block in that column. If a column's height rises, the chunk(s) below in
    /// that column become newly shadowed and must relight — so we queue the
    /// chunk directly below for relight (the shadow then propagates further
    /// down through normal border convergence, but only where it actually
    /// changes). Returns nothing; updates `column_heights` and `relight`.
    pub(crate) fn update_heightmap_for(&mut self, pos: ChunkPos) {
        let Some(chunk) = self.world.chunk(pos) else {
            return;
        };
        let heights = vox_core::chunk_column_heights(chunk, &self.registry);
        let origin = pos.origin();
        let mut any_raised = false;
        for lz in 0..(CHUNK_SIZE_I) {
            for lx in 0..(CHUNK_SIZE_I) {
                let local_top = heights[(lx + lz * CHUNK_SIZE_I) as usize];
                let Some(ly) = local_top else { continue };
                let world_y = origin.y + ly as i64;
                if self
                    .column_heights
                    .raise(origin.x + lx, origin.z + lz, world_y)
                {
                    any_raised = true;
                }
            }
        }
        if any_raised {
            // Some column height in this chunk's (x,z) footprint just became
            // known or rose. Every loaded chunk in the SAME VERTICAL STACK must
            // relight: a chunk lit while a column was still unknown (top_sky
            // defaulted to covered/0, or previously to daylit under the old
            // rule) has stale light that only a recompute can correct — this
            // is what un-freezes the sealed-hole daylight leak. All changed
            // columns lie within this chunk's own footprint, so the affected
            // chunks are exactly those with matching chunk (x,z) (the chunk
            // directly below, which the new surface may now shadow, is in the
            // stack too). The heightmap lists them: walking every loaded
            // chunk to find them cost thousands of visits per arriving chunk,
            // on the frame thread (2026-11 audit).
            for &cy in self.column_heights.resident_layers(pos.x, pos.z) {
                self.relight.insert(ChunkPos::new(pos.x, cy, pos.z));
            }
        }
    }

    /// Recompute one world column's height by scanning the loaded chunks in
    /// that column from the top down (used after an edit, which can raise OR
    /// lower a column). Updates the heightmap and queues the column's loaded
    /// chunks for relight so shadows appear/clear correctly.
    /// Recompute one world column's height by scanning the loaded chunks in
    /// that column from the top down (used after an edit, which can raise OR
    /// lower a column). Updates the heightmap, then re-relights the affected
    /// region so player builds cast correct shadows.
    ///
    /// This is only ever called from an edit (place/break), which always
    /// changes block topology, so we relight unconditionally — NOT only when
    /// the column's max height moved. A build can change interior light
    /// (hollowing a room, roofing an area) without changing the column max, and
    /// those cells still need recomputing. We relight:
    ///   - every loaded chunk in the edited column (the vertical shadow), and
    ///   - their ±X/±Z horizontal neighbors, because skylight under an overhang
    ///     is fed by diffuse light bleeding in from open sides; if those
    ///     neighbors aren't relit, level-15 sky keeps leaking under the build.
    ///
    /// Border-change convergence in `stream_tick` then carries any further
    /// ripple, but only where light actually changes.
    /// Re-relight the region affected by an edit at world column (wx, wz), and
    /// RAISE the heightmap if the edit added solid above the recorded surface.
    ///
    /// Crucially this never LOWERS the heightmap. The heightmap models the
    /// natural sky surface (highest solid that has open sky above it); digging
    /// down must NOT mark the column "open" below the dig, or daylight would
    /// flood the shaft floor and bleed into side tunnels (the old bug). Below
    /// the surface, skylight is computed honestly by downward propagation from
    /// the chunk above (the +Y sky plane), so a dug shaft stays correctly lit
    /// while sealed/side regions go dark — no heightmap lowering required.
    pub(crate) fn recompute_height_column(&mut self, wx: i64, wz: i64) {
        let lx = wx.rem_euclid(CHUNK_SIZE_I) as u8;
        let lz = wz.rem_euclid(CHUNK_SIZE_I) as u8;
        let cx = wx.div_euclid(CHUNK_SIZE_I);
        let cz = wz.div_euclid(CHUNK_SIZE_I);

        let mut highest: Option<i64> = None;
        let mut column_chunks: Vec<ChunkPos> = Vec::new();
        // The column's resident chunks, from the heightmap's own record of
        // them rather than a walk of every loaded chunk.
        let layers = self.column_heights.resident_layers(cx, cz).to_vec();
        for cy in layers {
            let cpos = ChunkPos::new(cx, cy, cz);
            let Some(chunk) = self.world.chunk(cpos) else {
                continue;
            };
            column_chunks.push(cpos);
            for ly in (0..CHUNK_SIZE_I as u8).rev() {
                let p = LocalPos::new(lx, ly, lz);
                // The skylight heightmap: where light stops, so OPAQUE
                // (ADR-0011), not solid.
                if self.registry.is_opaque(chunk.get(p)) {
                    let wy = cpos.origin().y + ly as i64;
                    highest = Some(highest.map_or(wy, |h| h.max(wy)));
                    break;
                }
            }
        }

        // Raise-only: a placed block above the surface extends it; a break can
        // only lower the physical top, which we deliberately ignore here.
        if let Some(h) = highest {
            self.column_heights.raise(wx, wz, h);
        }

        // Relight the edited column's chunks plus their horizontal neighbors so
        // shafts/caves resolve via honest propagation. Border-change
        // convergence in stream_tick carries any further vertical ripple.
        for p in column_chunks {
            self.relight.insert(p);
            for (dx, dy, dz) in NEIGHBOR_OFFSETS {
                if dy != 0 {
                    continue; // horizontal neighbors only
                }
                let n = ChunkPos::new(p.x + dx, p.y + dy, p.z + dz);
                if self.world.chunk(n).is_some() {
                    self.relight.insert(n);
                }
            }
        }
    }

    /// Save every loaded chunk holding edits that are not on disk yet, and
    /// the LOD edit overlay — on exit and every [`AUTOSAVE_SECS`], so edits
    /// to chunks still resident (not yet unloaded) survive a crash.
    ///
    /// "Not on disk yet" is `Chunk::is_unsaved`, not `is_modified`: a chunk
    /// stays modified for as long as it differs from generation, and writing
    /// every such chunk on every save would rewrite the player's whole base
    /// once a minute. A chunk whose write fails stays unsaved, so the next
    /// save retries it.
    pub(crate) fn save_unsaved(&mut self, why: &str) {
        let unsaved: Vec<ChunkPos> = self
            .world
            .chunks()
            .filter(|(_, chunk)| chunk.is_unsaved())
            .map(|(pos, _)| pos)
            .collect();
        let mut saved = 0;
        for pos in unsaved {
            let Some(chunk) = self.world.chunk_mut(pos) else {
                continue;
            };
            match self.store.save_chunk(pos, chunk) {
                Ok(()) => {
                    chunk.mark_saved();
                    saved += 1;
                }
                Err(e) => log::error!("{why}: saving {:?} failed: {e}", (pos.x, pos.y, pos.z)),
            }
        }
        if saved > 0 {
            log::info!("{why}: saved {saved} edited chunks");
        }
        self.save_edit_overlay();
    }

    /// Write the LOD edit overlay if it changed since it was last written.
    ///
    /// Called wherever chunks are saved, never per edit: the whole overlay is
    /// rewritten each time, which is cheap at the rate chunks save and would
    /// not be at the rate blocks break. On failure it stays marked unsaved, so
    /// the next save retries.
    pub(crate) fn save_edit_overlay(&mut self) {
        if !self.edits_unsaved {
            return;
        }
        match self.store.save_edited_columns(&self.edited_columns) {
            Ok(()) => self.edits_unsaved = false,
            Err(e) => log::error!("failed to save the LOD edit overlay: {e}"),
        }
    }

    /// The chunk-Y range resident around a column's terrain surface,
    /// whatever the camera does. LOD coverage asks this: whether the ground
    /// under a node is drawn. Deferring to the streamer keeps one source of
    /// truth — anything judging it independently gets it wrong the moment the
    /// margins or the clamping change.
    pub(crate) fn surface_window_chunks(&self, cx: i64, cz: i64) -> vox_core::ColumnWindow {
        self.streamer.surface_window(self.span(cx, cz))
    }

    /// A column's surface span, from the streamer's cache when it holds it.
    /// Every judge of residency must take the same span, and here they do:
    /// the cache holds exactly what `surface_span_chunks` returns. It
    /// matters for cost (M14 bench): the span now reads rivers, lakes and
    /// wetlands (`Generator::ground`, several times dearer than elevation
    /// alone), and `fullres_columns` asks for ~300 columns every frame —
    /// uncached, that cost a quarter of the frame rate standing still.
    pub(crate) fn span(&self, cx: i64, cz: i64) -> (i64, i64) {
        self.spans
            .peek(cx, cz)
            .unwrap_or_else(|| self.generator.surface_span_chunks(cx, cz))
    }

    /// The streaming configuration for a load radius. The one place it is
    /// built, so startup and a settings change cannot disagree.
    pub(crate) fn stream_config(load_radius: i64) -> vox_core::StreamConfig {
        vox_core::StreamConfig {
            load_radius,
            unload_radius: load_radius + vox_core::UNLOAD_MARGIN_CHUNKS,
            below_surface: LOAD_BELOW_CHUNKS,
            above_surface: LOAD_ABOVE_CHUNKS,
            camera_layers: CAMERA_WINDOW_LAYERS,
            // A radius setting below the window's would otherwise be refused.
            camera_radius: CAMERA_WINDOW_RADIUS.min(load_radius),
            // Keep the sea surface over deep water, not the water under it.
            sea_layer: Some(vox_core::SEA_SURFACE_CHUNK_Y),
        }
    }

    /// One streaming step, run every frame. Keeps the resident chunk set
    /// centered on the camera and the GPU meshes in sync, within per-frame
    /// budgets so the frame never stalls.
    ///
    /// Generation is async (rayon `spawn` → channel). Meshing is
    /// bounded-parallel on the main thread: it borrows the World immutably
    /// via `mesh_chunks_parallel` (all cores), time-budgeted per frame
    /// chunks/frame so the cost stays well under a millisecond. This honors
    /// "runs on the rayon pool, bounded, no stall" without needing to clone
    /// chunk data into mesh jobs or share the World across threads.
    ///
    /// Every mesh this step produces, and every mesh it withdraws (an empty
    /// one), goes to `sink` at the moment the old in-`App` code called the
    /// renderer. Returns the chunks that arrived from disk carrying edits:
    /// the caller's LOD nodes over them were built from the seed and must be
    /// rebuilt (the LOD ring is not the session's).
    pub(crate) fn tick(
        &mut self,
        camera_chunk: ChunkPos,
        sink: &mut dyn FnMut(ChunkPos, &MeshData),
    ) -> Vec<ChunkPos> {
        let mut edited_arrivals = Vec::new();
        // 1. Ask the streamer what should change. A still camera over a
        //    settled world costs nothing; otherwise spans come from the cache,
        //    so only columns new to the neighbourhood sample the terrain.
        let generator = self.generator.clone();
        self.spans
            .prune(camera_chunk, self.streamer.unload_radius() + 1);
        let spans = &mut self.spans;
        let update = self.streamer.update(camera_chunk, |cx, cz| {
            spans.get(cx, cz, |x, z| generator.surface_span_chunks(x, z))
        });

        // 2. Unloads: save the chunk if it was modified, then drop its data
        //    + GPU mesh; neighbors may now expose a border face, so they're
        //    marked dirty below.
        let mut saved_any = false;
        for pos in &update.to_unload {
            if let Some(chunk) = self.world.chunk(*pos) {
                // Only what is not on disk already: a chunk that came from
                // disk and was not touched since has nothing to write.
                if chunk.is_unsaved() {
                    saved_any = true;
                    if let Err(e) = self.store.save_chunk(*pos, chunk) {
                        log::error!("failed to save chunk {:?}: {e}", (pos.x, pos.y, pos.z));
                    }
                }
            }
            if self.world.remove_chunk(*pos).is_some() {
                self.column_heights.chunk_unloaded(*pos);
            }
            self.streamer.mark_unloaded(*pos);
            self.dirty.remove(pos);
            sink(*pos, &MeshData::default());
        }
        if saved_any {
            self.save_edit_overlay();
        }
        // Separate pass so neighbor marking isn't undone by the removal loop.
        for pos in &update.to_unload {
            for (dx, dy, dz) in NEIGHBOR_OFFSETS {
                let n = ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                if self.streamer.is_loaded(n) {
                    self.dirty.insert(n);
                }
            }
        }

        // 3. Spawn async generation for newly-in-range chunks, bounded.
        let mut spawned = 0;
        for pos in update.to_load {
            if spawned >= GEN_SPAWN_BUDGET {
                break;
            }
            if self.gen_in_flight.contains(&pos) || self.streamer.is_loaded(pos) {
                continue;
            }
            self.gen_in_flight.insert(pos);
            let generator = self.generator.clone();
            let store = self.store.clone();
            let tx = self.gen_tx.clone();
            self.spawner.spawn(
                pos,
                Box::new(move || {
                    // Generate-or-load: a saved (edited) chunk takes precedence
                    // over regeneration; otherwise generate from the seed. A
                    // load error falls back to generation so a corrupt file can't
                    // wedge streaming.
                    let chunk = match store.load_chunk(pos) {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => generator.generate_chunk(pos),
                        Err(e) => {
                            log::error!(
                                "load chunk {:?} failed: {e}; regenerating",
                                (pos.x, pos.y, pos.z)
                            );
                            generator.generate_chunk(pos)
                        }
                    };
                    let _ = tx.send((pos, chunk));
                }),
            );
            spawned += 1;
        }

        // 4. Drain finished generation: insert into the World, mark loaded,
        //    and dirty the chunk + its neighbors so borders resolve.
        let mut newly_generated = Vec::new();
        while let Ok((pos, chunk)) = self.gen_rx.try_recv() {
            self.gen_in_flight.remove(&pos);
            // Count residency only for a chunk that was not already there, so
            // the heightmap's per-column counts stay exact.
            if self.world.insert_chunk(pos, chunk).is_none() {
                self.column_heights.chunk_loaded(pos);
            }
            self.streamer.mark_loaded(pos);
            newly_generated.push(pos);
        }
        for pos in newly_generated {
            // A chunk that arrives already MODIFIED came from disk carrying
            // player edits. LOD nodes stream in immediately at startup, well
            // before saved chunks finish loading, so the node covering this
            // column was built from seed heights and shows the terrain as it
            // was before those edits — and nothing else would ever rebuild it.
            let edited = self.world.chunk(pos).is_some_and(|c| c.is_modified());
            // Fold the new chunk into the column heightmap BEFORE marking it
            // dirty, so its first relight uses a correct sky top boundary.
            self.update_heightmap_for(pos);
            self.mark_dirty_with_neighbors(pos);
            if edited {
                edited_arrivals.push(pos);
            }
        }

        // 5. Mesh a bounded batch of dirty chunks and upload.
        //
        // First prune "ghost" dirty entries — positions that are no longer
        // resident (unloaded, or marked dirty as a neighbor before they were
        // ever generated). A non-resident chunk has no mesh to build; if it
        // later loads, the generation drain (step 4) re-marks it dirty.
        // Without this prune the dirty set leaks unbounded as you travel
        // (it accumulates every unloaded chunk's former neighbors).
        {
            let world = &self.world;
            self.dirty.retain(|p| world.chunk(*p).is_some());
            self.relight.retain(|p| world.chunk(*p).is_some());
            // Bounded with residency: an unloaded chunk must re-earn its first
            // mesh (it will be regenerated and re-lit from scratch).
            self.meshed_once.retain(|p| world.chunk(*p).is_some());
        }

        // --- Relight pass (time-budgeted, parallel). ---
        // Recompute light in parallel sub-batches until a per-frame time cap.
        // top_sky is built per-chunk inside the workers (parallel), not here.
        // Apply only where light actually changed (→ re-mesh via `dirty`); a
        // changed border just queues a neighbor relight check (no re-mesh).
        if !self.relight.is_empty() {
            let start = Instant::now();
            // Select the nearest candidates ONCE per frame, then consume them
            // in sub-batches. Re-scanning the (thousands-strong) set for every
            // sub-batch would burn more time than the relight itself.
            let candidates = vox_core::nearest_first(
                self.relight.iter().copied(),
                camera_chunk,
                RELIGHT_SUBBATCH * 8,
            );
            for sub in candidates.chunks(RELIGHT_SUBBATCH) {
                let sub: Vec<ChunkPos> = sub.to_vec();
                if sub.is_empty() {
                    break;
                }
                for p in &sub {
                    self.relight.remove(p);
                }
                let jobs: Vec<(ChunkPos, u8)> = sub
                    .iter()
                    .map(|&p| (p, self.sealed_faces(p, camera_chunk)))
                    .collect();
                let (lit, cpu_ms) = relight_chunks_parallel(
                    &self.world,
                    &self.registry,
                    &self.column_heights,
                    &jobs,
                );
                self.relight_cpu_ms_total += cpu_ms;
                self.relit_total += jobs.len() as u64;
                for (pos, light, interior_changed, border_faces) in lit {
                    if interior_changed {
                        if let Some(chunk) = self.world.chunk_mut(pos) {
                            vox_core::apply_chunk_light(chunk, &light);
                        }
                        self.dirty.insert(pos);
                    }
                    // Requeue ONLY the neighbor across each face whose border
                    // light actually changed (bit i = face i, same order as
                    // NEIGHBOR_OFFSETS). Requeuing all six per change is what
                    // made the convergence cascade balloon the queues during
                    // streaming bursts. The neighbor gets BOTH a relight (its
                    // light may now differ) and a re-mesh: its mesh samples
                    // this chunk's border cells across the seam, so a changed
                    // border makes it stale (the dark seam-line artifact). The
                    // defer-while-relighting rule coalesces the two so it
                    // still meshes once, after light settles.
                    if border_faces != 0 {
                        for (face, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
                            if border_faces & (1 << face) == 0 {
                                continue;
                            }
                            // The face neighbor is relit (its light may change,
                            // since sky/block light crosses faces) AND remeshed
                            // (its faces sample our border across the seam).
                            let n = ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                            if self.world.chunk(n).is_some() {
                                self.relight.insert(n);
                                self.dirty.insert(n);
                            }
                            // Smooth lighting (M06) also samples DIAGONALLY, so
                            // the chunks sharing an EDGE with this face read our
                            // border cells too. Light doesn't propagate
                            // diagonally, so these need re-MESH only, not
                            // relight. Edge neighbors = the changed-face offset
                            // plus one step along each perpendicular axis.
                            let ax = face / 2; // 0:X 1:Y 2:Z (face pairs)
                            for perp in 0..3 {
                                if perp == ax {
                                    continue;
                                }
                                for step in [-1i64, 1] {
                                    let mut off = [*dx, *dy, *dz];
                                    off[perp] += step;
                                    let e = ChunkPos::new(
                                        pos.x + off[0],
                                        pos.y + off[1],
                                        pos.z + off[2],
                                    );
                                    if self.world.chunk(e).is_some() {
                                        self.dirty.insert(e);
                                    }
                                }
                            }
                        }
                    }
                }
                if start.elapsed().as_secs_f32() * 1000.0 >= RELIGHT_TIME_MS {
                    break;
                }
            }
            let ms = start.elapsed().as_secs_f32() * 1000.0;
            self.relight_ms_accum += ms;
            self.relight_ms_total += f64::from(ms);
        }

        // --- Mesh pass (time-budgeted, parallel). ---
        if !self.dirty.is_empty() {
            let start = Instant::now();
            // Nearest-camera-first (M09 task 1), selected ONCE per frame: under
            // a deep backlog the player's surroundings must converge before
            // distant work, or freshly streamed chunks sit dark for seconds.
            // Re-scanning the whole dirty set per sub-batch would cost more
            // than the meshing. Two deferrals apply:
            //  - chunks still queued for relight (mesh once, already lit —
            //    otherwise every streamed chunk is meshed twice and flashes
            //    dark in between);
            //  - chunks awaiting their first complete neighborhood
            //    (`ready_for_first_mesh`).
            // Deferred chunks stay in `dirty` for a later frame.
            let candidates: Vec<ChunkPos> = vox_core::nearest_first(
                self.dirty
                    .iter()
                    .copied()
                    .filter(|p| !self.relight.contains(p))
                    .filter(|p| self.ready_for_first_mesh(*p, camera_chunk)),
                camera_chunk,
                MESH_SUBBATCH * 8,
            );
            for batch in candidates.chunks(MESH_SUBBATCH) {
                let batch: Vec<ChunkPos> = batch.to_vec();
                if batch.is_empty() {
                    break;
                }
                for p in &batch {
                    self.dirty.remove(p);
                    // This chunk has now had a complete first mesh; later
                    // re-meshes are not gated.
                    self.meshed_once.insert(*p);
                }

                let jobs: Vec<(ChunkPos, u8)> = batch
                    .iter()
                    .map(|&p| (p, self.sealed_faces(p, camera_chunk)))
                    .collect();
                let (meshes, cpu_ms) = mesh_chunks_parallel(&self.world, &self.registry, &jobs);
                self.mesh_cpu_ms_total += cpu_ms;
                self.meshed_total += jobs.len() as u64;
                let produced: HashSet<ChunkPos> = meshes.iter().map(|(p, _)| *p).collect();
                // A batch chunk that produced no mesh (all air, or fully
                // occluded) must have any stale GPU mesh cleared.
                for p in &batch {
                    if !produced.contains(p) {
                        sink(*p, &MeshData::default());
                    }
                }
                for (pos, mesh) in &meshes {
                    sink(*pos, mesh);
                }
                if start.elapsed().as_secs_f32() * 1000.0 >= MESH_TIME_MS {
                    break;
                }
            }
            let ms = start.elapsed().as_secs_f32() * 1000.0;
            self.mesh_ms_accum += ms;
            self.mesh_ms_total += f64::from(ms);
        }
        edited_arrivals
    }

    /// M09 task 1: is this chunk ready for its FIRST mesh?
    ///
    /// Meshing a chunk before its neighbors exist bakes two errors: border
    /// faces computed against phantom air, and (the visible one) light sampled
    /// before skylight/blocklight can cross the seams — the "black chunk"
    /// that then persists until a neighbor's arrival happens to re-dirty it.
    ///
    /// So a chunk's first mesh waits for every face neighbor that is still
    /// *coming*. A neighbor the streamer does not want is never coming, so it
    /// is not waited on — otherwise the outermost shell would never mesh at
    /// all and the full-res region would end in a permanent hole. Re-meshes of
    /// an already-meshed chunk are never gated (an edit must show immediately).
    pub(crate) fn ready_for_first_mesh(&self, p: ChunkPos, camera_chunk: ChunkPos) -> bool {
        if self.meshed_once.contains(&p) {
            return true;
        }
        // Wait only on neighbours that are absent AND still coming. Waiting on
        // one that never arrives leaves every chunk gated on it dark: the M09
        // black-chunk defect, reintroduced once by a taller world.
        NEIGHBOR_OFFSETS.iter().all(|(dx, dy, dz)| {
            let n = ChunkPos::new(p.x + dx, p.y + dy, p.z + dz);
            self.world.chunk(n).is_some() || !self.neighbor_coming(n, camera_chunk)
        })
    }

    /// Will this ABSENT chunk arrive while the camera stays where it is?
    ///
    /// The streamer's own load set — a cylinder horizontally, the surface
    /// window plus the camera window vertically. The first-mesh gate (don't
    /// wait on it if not) and sealing (mesh the chunk's edge as continuing if
    /// not) both ask exactly this, so both ask here: two independent answers
    /// drifting apart is how a gate deadlocks or an edge opens onto the void.
    pub(crate) fn neighbor_coming(&self, n: ChunkPos, camera_chunk: ChunkPos) -> bool {
        self.streamer.wants(n, camera_chunk, self.span(n.x, n.z))
    }

    /// The faces of `p` to seal for meshing: a bit per `NEIGHBOR_OFFSETS`
    /// entry, set where that neighbour is absent and not coming (M10 A3).
    ///
    /// The seal is re-judged at every mesh, and every change of a neighbour's
    /// residency re-meshes this chunk (arrival and unload both dirty their
    /// neighbours), so a seal never outlives the absence it describes. The one
    /// gap: a neighbour that was coming and stops being so without ever
    /// loading (the camera left first) keeps its open, dimmed edge until
    /// something else re-meshes the chunk — the pre-A3 behaviour, and rare.
    pub(crate) fn sealed_faces(&self, p: ChunkPos, camera_chunk: ChunkPos) -> u8 {
        let mut sealed = 0u8;
        for (bit, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
            let n = ChunkPos::new(p.x + dx, p.y + dy, p.z + dz);
            if self.world.chunk(n).is_none() && !self.neighbor_coming(n, camera_chunk) {
                sealed |= 1 << bit;
            }
        }
        sealed
    }

    /// Recompute one column's surface height from the world.
    ///
    /// `update_heightmap_for` only ever RAISES a column (correct for
    /// streaming, where chunks arrive in any order and the max wins), so it
    /// cannot see a block being mined away. Without this the heightmap keeps
    /// the pre-edit surface, and anything derived from it — LOD nodes, skylight
    /// — rebuilds the terrain the player just removed: a "ghost block".
    pub(crate) fn recompute_column_height(&mut self, x: i64, z: i64) {
        // Scan exactly the chunks that ARE resident in this column, top
        // down. Against the world band this would be a 20 000-block walk on
        // every block break, and blocks that are not loaded would read as
        // air anyway.
        //
        // The heightmap's own record of the column's chunks, not the layers
        // the streamer LOADS (`column_window_chunks`, which this used until
        // the 2026-11 audit): the streamer KEEPS one layer more than it
        // loads around the camera (its unload hysteresis), and a block in
        // such a chunk — a roof built just above, the player having stepped
        // down a layer since — was invisible to the scan. The height was then
        // set BELOW the real top, which is the one direction the skylight
        // must never err in (CLAUDE.md: a too-low height invents daylight).
        //
        // TWO tops, because they answer different questions (ADR-0011): the
        // skylight heightmap wants the highest OPAQUE block (where light
        // stops), the LOD's terrain the highest SOLID one (the ground, which
        // under an ocean is the seabed). Identical for every block today.
        let mut layers = self
            .column_heights
            .resident_layers(x.div_euclid(CHUNK_SIZE_I), z.div_euclid(CHUNK_SIZE_I))
            .to_vec();
        layers.sort_unstable_by(|a, b| b.cmp(a));
        let (mut opaque_top, mut solid_top) = (None, None);
        'scan: for cy in layers {
            for y in (cy * CHUNK_SIZE_I..(cy + 1) * CHUNK_SIZE_I).rev() {
                let b = self.world.get_block(WorldPos::new(x, y, z));
                if opaque_top.is_none() && self.registry.is_opaque(b) {
                    opaque_top = Some(y);
                }
                if solid_top.is_none() && self.registry.is_solid(b) {
                    solid_top = Some(y);
                }
                if opaque_top.is_some() && solid_top.is_some() {
                    break 'scan;
                }
            }
        }
        // i64::MIN: known, and nothing there — a column mined out entirely.
        self.column_heights
            .set(x, z, opaque_top.unwrap_or(i64::MIN));
        // Sparse, never pruned and saved with the world, so every LOD level
        // sees the edit, this session and every later one. Clamped to the
        // world floor: a fully mined column has no top at all.
        Arc::make_mut(&mut self.edited_columns).record(
            x,
            z,
            solid_top
                .unwrap_or(i64::MIN)
                .max(vox_core::WORLD_Y_MIN_BLOCKS) as i32,
        );
        self.edits_unsaved = true;
    }

    /// Chunk columns whose full-resolution terrain is DRAWN: every layer of
    /// the column's surface window — its ground, and over deep water the sea
    /// surface — resident AND meshed at least once. Searched out to the
    /// UNLOAD radius: the hysteresis band holds drawn chunks too, and leaving
    /// it out left LOD under their water (M10 A1).
    ///
    /// Complete columns only. A column with its sea surface but not yet its
    /// seabed (or the reverse) is not full resolution's to draw: the renderer
    /// draws near water only in these columns and LOD everywhere else, so the
    /// two never overlap in either direction.
    ///
    /// Residency alone is not enough: a chunk's first mesh is deliberately
    /// held back until its neighbours arrive (so it never bakes dark), and
    /// sits behind a meshing queue. Counting it earlier hides the coarse
    /// terrain while the real chunk draws nothing, and the sky shows through —
    /// a flash at the edge of the full-res region at radius 8, a band hundreds
    /// of blocks wide at radius 24.
    ///
    /// The one answer to "does full resolution draw here?", for both of its
    /// consumers: whole LOD nodes skipped when every column under them is in
    /// it (`covered_lod_nodes`), and per-column discarding in the LOD shader
    /// for the nodes that straddle the edge (ADR-0011 notes).
    pub(crate) fn fullres_columns(&self, camera_chunk: ChunkPos) -> HashSet<(i64, i64)> {
        // The streamer's own reach: everything it can hold resident.
        let r = self.streamer.unload_radius();
        let mut out = HashSet::new();
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                let (cx, cz) = (camera_chunk.x + dx, camera_chunk.z + dz);
                // Only the layers that will ever be resident for THIS column.
                // Walking the world band would be ~640 layers and would never
                // succeed, because streaming does not load them.
                let drawn = self.surface_window_chunks(cx, cz).layers().all(|cy| {
                    let c = ChunkPos::new(cx, cy, cz);
                    self.world.chunk(c).is_some() && self.meshed_once.contains(&c)
                });
                if drawn {
                    out.insert((cx, cz));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    //! The session as a system (CLAUDE.md, lesson 2): a real generated
    //! world, a scripted camera, and chunk arrival in an order the test
    //! chooses. Each test asks what a player would see at the end.

    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    use vox_core::{WorldMeta, WorldShape, CHUNK_VOLUME};
    use vox_mesh::Vertex;

    use super::*;

    const SEED: u64 = 0x5E55_1011;
    /// Small on purpose: a few hundred chunks resident, seconds per test.
    const RADIUS: i64 = 3;

    fn shape() -> WorldShape {
        WorldShape::new(32_768, 32_768).expect("a legal world size")
    }

    /// One generator for every test: its stage-1 maps are built once, and
    /// every clone shares them (and the ground cache, which is only a cache).
    fn generator() -> Generator {
        static SHARED: OnceLock<Generator> = OnceLock::new();
        SHARED.get_or_init(|| Generator::new(SEED, shape())).clone()
    }

    /// A camera chunk standing just above dry land.
    fn land_camera() -> ChunkPos {
        let g = generator();
        let (x, z) = vox_core::find_spawn(shape(), 0, 0, 256, 64, |x, z| {
            g.surface_height(x, z) > vox_core::SEA_LEVEL_BLOCKS + 4
        })
        .expect("the test world has land");
        WorldPos::new(x, g.surface_height(x, z) + 12, z).chunk()
    }

    /// How a test delivers the chunks the session asked for.
    #[derive(Clone, Copy)]
    enum Delivery {
        /// Every requested chunk arrives before the next tick, in request
        /// order: the kindest case.
        Prompt,
        /// Requests are shuffled and only some arrive each tick; the rest
        /// wait, and sometimes nothing arrives at all. Chunks of a column
        /// come in any order, air before the ground under it or after.
        Late(u64),
        /// One chunk layer at a time, highest first, each only once the
        /// world has finished lighting what came before: the sky, then the
        /// ground under it. What nearest-first streaming does to a camera
        /// high in the air — and the order in which random arrival's lucky
        /// healing (a neighbour happening to arrive later and re-light the
        /// chunk) never comes.
        TopDown,
        /// The same, lowest layer first: what nearest-first streaming does
        /// to a camera deep underground, whose cave arrives before the
        /// surface far above it.
        BottomUp,
    }

    type Meshes = HashMap<ChunkPos, (Vec<Vertex>, Vec<u32>)>;

    /// A session over a world folder of its own, with the generation queue
    /// in the test's hands and every mesh it emits collected.
    struct Rig {
        session: Session,
        jobs: Arc<Mutex<Vec<(ChunkPos, Job)>>>,
        meshes: Meshes,
        dir: PathBuf,
        rng: u64,
        /// The queue's length at the last staged delivery: a staged order
        /// starts only once the session has asked for everything it wants
        /// (the queue has stopped growing), so "highest first" is the
        /// highest of all of it.
        queued_before: usize,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("voxterra-session-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self::open(dir)
        }

        /// A fresh session over an existing (or new) world folder: what
        /// quitting and starting the game again is.
        fn open(dir: PathBuf) -> Self {
            let meta = WorldMeta {
                seed: SEED,
                shape: shape(),
                generator_version: vox_worldgen::GENERATOR_VERSION,
            };
            let store = WorldStore::open(&dir, meta).expect("the world folder opens");
            let edits = store.load_edited_columns().expect("the overlay loads");
            let jobs = Arc::new(Mutex::new(Vec::new()));
            let session = Session::new(
                store,
                generator(),
                edits,
                RADIUS,
                Spawner::Queue(Arc::clone(&jobs)),
            );
            Self {
                session,
                jobs,
                meshes: HashMap::new(),
                dir,
                rng: 0x9E37_79B9_7F4A_7C15,
                queued_before: usize::MAX,
            }
        }

        /// A session over a copy of `other`'s world folder as it is on disk
        /// now: the same saved world, started somewhere else.
        fn from_copy_of(other: &Rig, name: &str) -> Self {
            fn copy(from: &std::path::Path, to: &std::path::Path) {
                std::fs::create_dir_all(to).unwrap();
                for entry in std::fs::read_dir(from).unwrap() {
                    let entry = entry.unwrap();
                    let target = to.join(entry.file_name());
                    if entry.file_type().unwrap().is_dir() {
                        copy(&entry.path(), &target);
                    } else {
                        std::fs::copy(entry.path(), target).unwrap();
                    }
                }
            }
            let dir = std::env::temp_dir()
                .join(format!("voxterra-session-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            copy(&other.dir, &dir);
            Self::open(dir)
        }

        fn sky_at(&self, p: WorldPos) -> u8 {
            let (c, l) = p.split();
            self.session.world.chunk(c).expect("resident").sky_light(l)
        }

        fn reopen(mut self) -> Self {
            // Taking the path leaves `Drop` nothing to delete.
            let dir = std::mem::take(&mut self.dir);
            drop(self);
            Self::open(dir)
        }

        fn next(&mut self) -> u64 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            self.rng
        }

        fn tick(&mut self, camera: ChunkPos) {
            let meshes = &mut self.meshes;
            self.session.tick(camera, &mut |pos, mesh| {
                if mesh.is_empty() {
                    meshes.remove(&pos);
                } else {
                    meshes.insert(pos, (mesh.vertices.clone(), mesh.indices.clone()));
                }
            });
        }

        /// Run queued generation jobs as `delivery` says; their chunks are
        /// received by the next tick.
        fn deliver(&mut self, delivery: Delivery) {
            let mut queued: Vec<(ChunkPos, Job)> = std::mem::take(&mut *self.jobs.lock().unwrap());
            match delivery {
                Delivery::Prompt => queued.into_iter().for_each(|(_, job)| job()),
                Delivery::TopDown | Delivery::BottomUp => {
                    // Nothing new until every request is in and the last
                    // layer delivered has been received and lit.
                    let busy = !self.session.relight.is_empty()
                        || self.session.gen_in_flight.len() != queued.len()
                        || self.queued_before != queued.len();
                    self.queued_before = queued.len();
                    let layer = match delivery {
                        Delivery::TopDown => queued.iter().map(|(p, _)| p.y).max(),
                        _ => queued.iter().map(|(p, _)| p.y).min(),
                    };
                    let (now, rest): (Vec<_>, Vec<_>) = queued
                        .into_iter()
                        .partition(|(p, _)| !busy && Some(p.y) == layer);
                    if !now.is_empty() {
                        self.queued_before = usize::MAX; // the queue is about to shrink
                    }
                    now.into_iter().for_each(|(_, job)| job());
                    self.jobs.lock().unwrap().extend(rest);
                }
                Delivery::Late(seed) => {
                    if self.rng == 0x9E37_79B9_7F4A_7C15 {
                        self.rng ^= seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1;
                    }
                    // Shuffle, then run a random share: none one tick in
                    // five, otherwise between one job and all of them.
                    for i in (1..queued.len()).rev() {
                        let j = (self.next() % (i as u64 + 1)) as usize;
                        queued.swap(i, j);
                    }
                    let run = if queued.is_empty() || self.next().is_multiple_of(5) {
                        0
                    } else {
                        1 + (self.next() % queued.len() as u64) as usize
                    };
                    let rest = queued.split_off(run);
                    queued.into_iter().for_each(|(_, job)| job());
                    self.jobs.lock().unwrap().extend(rest);
                }
            }
        }

        fn queue_is_empty(&self) -> bool {
            self.jobs.lock().unwrap().is_empty()
        }

        /// Tick and deliver until nothing is left to do at `camera`.
        fn settle(&mut self, camera: ChunkPos, delivery: Delivery) {
            for _ in 0..50_000 {
                self.tick(camera);
                self.deliver(delivery);
                if self.session.is_settled() && self.queue_is_empty() {
                    // One more tick: a chunk delivered just now is received
                    // by the next one.
                    self.tick(camera);
                    if self.session.is_settled() && self.queue_is_empty() {
                        return;
                    }
                }
            }
            panic!(
                "the world never settled: {} generating, {} to relight, {} to mesh",
                self.session.gen_in_flight.len(),
                self.session.relight.len(),
                self.session.dirty.len()
            );
        }

        /// Fly `steps` chunks along +X from `start`, three ticks a chunk —
        /// faster than the world can keep up — then settle where it stops.
        fn fly(&mut self, start: ChunkPos, steps: i64, delivery: Delivery) -> ChunkPos {
            let mut camera = start;
            self.settle(camera, delivery);
            for _ in 0..steps {
                camera = ChunkPos::new(camera.x + 1, camera.y, camera.z);
                for _ in 0..3 {
                    self.tick(camera);
                    self.deliver(delivery);
                }
            }
            self.settle(camera, delivery);
            camera
        }

        fn positions(&self) -> Vec<ChunkPos> {
            let mut all: Vec<ChunkPos> = self.session.world.chunk_positions().collect();
            all.sort_by_key(|p| (p.x, p.y, p.z));
            all
        }

        fn light_of(&self, pos: ChunkPos) -> Vec<u8> {
            let chunk = self.session.world.chunk(pos).expect("resident");
            (0..CHUNK_VOLUME).map(|i| chunk.packed_light(i)).collect()
        }

        /// The invariants of a settled world, whatever happened before.
        fn assert_consistent(&self, camera: ChunkPos) {
            let s = &self.session;
            let positions = self.positions();
            assert!(
                positions.len() > 100,
                "only {} chunks resident",
                positions.len()
            );

            // The streamer, the world and the heightmap agree on what is
            // resident, chunk for chunk.
            assert_eq!(s.streamer.loaded_count(), positions.len());
            let mut columns: HashMap<(i64, i64), Vec<i64>> = HashMap::new();
            for p in &positions {
                assert!(
                    s.streamer.is_loaded(*p),
                    "{p:?} in the world, not the streamer"
                );
                columns.entry((p.x, p.z)).or_default().push(p.y);
            }
            assert_eq!(s.column_heights.resident_columns(), columns.len());
            for ((cx, cz), mut want) in columns {
                let mut have = s.column_heights.resident_layers(cx, cz).to_vec();
                have.sort_unstable();
                want.sort_unstable();
                assert_eq!(have, want, "heightmap residency of column ({cx}, {cz})");
            }

            // Every chunk has had its first mesh, and no mesh outlives its
            // chunk.
            for p in &positions {
                assert!(s.meshed_once.contains(p), "{p:?} was never meshed");
            }
            assert_eq!(s.meshed_once.len(), positions.len());
            assert!(!self.meshes.is_empty(), "nothing was drawn");
            for p in self.meshes.keys() {
                assert!(s.world.chunk(*p).is_some(), "a mesh for unloaded {p:?}");
            }

            // The light is a fixed point: relighting any chunk from its
            // neighbours as they stand changes nothing. A chunk left with
            // light its neighbours no longer justify — the M05 class of bug
            // — fails here.
            let jobs: Vec<(ChunkPos, u8)> = positions
                .iter()
                .map(|&p| (p, s.sealed_faces(p, camera)))
                .collect();
            let (lit, _) = relight_chunks_parallel(&s.world, &s.registry, &s.column_heights, &jobs);
            assert_eq!(lit.len(), positions.len());
            for (pos, _, changed, _) in lit {
                assert!(!changed, "{pos:?} holds light a relight would change");
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            if !self.dir.as_os_str().is_empty() {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
    }

    /// Soak: fly faster than the world loads, with chunks arriving promptly,
    /// and everything a settled world promises still holds at the end.
    #[test]
    fn a_flight_ends_lit_meshed_and_consistent() {
        let mut rig = Rig::new("soak");
        let camera = rig.fly(land_camera(), 10, Delivery::Prompt);
        rig.assert_consistent(camera);
        // Something is actually lit: daylight reached the ground.
        let lit = rig
            .positions()
            .iter()
            .filter(|p| rig.session.world.chunk(**p).is_some_and(|c| c.has_light()))
            .count();
        assert!(lit > 20, "only {lit} chunks hold any light");
    }

    /// What a player built, for the tests that need more than open ground:
    /// today's terrain is a heightfield with no caves, so nothing in it can
    /// be wrongly lit by an early guess about what lies above. These can.
    struct Built {
        /// Air cells of a sealed room carved in solid rock, two chunks tall
        /// and across a chunk border: no light may ever reach them.
        sealed: Vec<WorldPos>,
        /// The floor of an open shaft through a chunk boundary: full
        /// daylight stands on it.
        shaft_floor: WorldPos,
        /// Ground under the middle of a wide slab laid above the surface,
        /// across a chunk border: shaded.
        under_slab: WorldPos,
    }

    /// Carve and build at the chunk border east of `camera`'s chunk, in a
    /// settled world. The caller settles again afterwards.
    fn build(rig: &mut Rig, camera: ChunkPos) -> Built {
        let stone = vox_core::registry::STONE;
        let origin = camera.origin();
        // The chunk border in X, mid-chunk in Z.
        let (bx, bz) = (origin.x + CHUNK_SIZE_I, origin.z + 12);

        // A sealed room: 6 x 5 in plan across the X border, from 44 to 10
        // blocks under the lowest ground above it — so it crosses a chunk
        // boundary vertically too, and stays inside the loaded window.
        let lowest = (bx - 3..bx + 3)
            .flat_map(|x| (bz..bz + 5).map(move |z| (x, z)))
            .map(|(x, z)| top_solid(rig, x, z).y)
            .min()
            .unwrap();
        let mut sealed = Vec::new();
        for x in bx - 3..bx + 3 {
            for z in bz..bz + 5 {
                for y in lowest - 44..=lowest - 10 {
                    let p = WorldPos::new(x, y, z);
                    assert!(rig
                        .session
                        .registry
                        .is_opaque(rig.session.world.get_block(p)));
                    rig.session.set_block(p, BlockId::AIR);
                    sealed.push(p);
                }
            }
        }

        // An open shaft 2 x 2, 40 deep, well clear of the room.
        let (sx, sz) = (bx - 1, bz + 14);
        let shaft_top = (sx..sx + 2)
            .flat_map(|x| (sz..sz + 2).map(move |z| (x, z)))
            .map(|(x, z)| top_solid(rig, x, z).y)
            .min()
            .unwrap();
        for x in sx..sx + 2 {
            for z in sz..sz + 2 {
                let top = top_solid(rig, x, z).y;
                for y in shaft_top - 40..=top {
                    rig.session.set_block(WorldPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        let shaft_floor = WorldPos::new(sx, shaft_top - 41, sz);

        // A slab 16 x 9, four blocks above the highest ground under it.
        let (px, pz) = (bx - 8, bz - 11);
        let slab_y = (px..px + 16)
            .flat_map(|x| (pz..pz + 9).map(move |z| (x, z)))
            .map(|(x, z)| top_solid(rig, x, z).y)
            .max()
            .unwrap()
            + 4;
        let under_slab = top_solid(rig, px + 8, pz + 4);
        for x in px..px + 16 {
            for z in pz..pz + 9 {
                rig.session.set_block(WorldPos::new(x, slab_y, z), stone);
            }
        }
        Built {
            sealed,
            shaft_floor,
            under_slab,
        }
    }

    /// The light a correct world gives what was built, however it loaded.
    fn assert_built_is_lit_correctly(rig: &Rig, built: &Built, how: &str) {
        for p in &built.sealed {
            let (c, l) = p.split();
            let chunk = rig.session.world.chunk(c).expect("resident");
            assert_eq!(
                (chunk.sky_light(l), chunk.block_light(l)),
                (0, 0),
                "{how}: light in a sealed room at {p:?}"
            );
        }
        let floor = built.shaft_floor;
        assert!(rig
            .session
            .registry
            .is_solid(rig.session.world.get_block(floor)));
        assert_eq!(
            rig.sky_at(WorldPos::new(floor.x, floor.y + 1, floor.z)),
            15,
            "{how}: the shaft's floor is in full daylight"
        );
        let u = built.under_slab;
        let shade = rig.sky_at(WorldPos::new(u.x, u.y + 1, u.z));
        assert!(
            (1..15).contains(&shade),
            "{how}: under the slab is shaded, not dark or sunlit (got {shade})"
        );
    }

    /// Order independence — the M05 saga as a test. A world with a sealed
    /// room, an open shaft and an overhang in it is saved, and then
    /// approached three times: with chunks arriving promptly, and twice
    /// shuffled and late (air before its ground, a room before its roof,
    /// nothing at all on some frames). Every approach ends in exactly the
    /// same world — the same chunks, the same blocks, the same light in
    /// every voxel — and the room is dark in all of them.
    #[test]
    fn arrival_order_does_not_change_the_world() {
        let site = land_camera();
        let mut builder = Rig::new("order-build");
        builder.settle(site, Delivery::Prompt);
        let built = build(&mut builder, site);
        builder.settle(site, Delivery::Prompt);
        builder.assert_consistent(site);
        assert_built_is_lit_correctly(&builder, &built, "where it was built");
        builder.session.save_unsaved("test");

        // Approach from 8 chunks west, so the site streams in under way.
        let start = ChunkPos::new(site.x - 8, site.y, site.z);
        let mut prompt = Rig::from_copy_of(&builder, "order-prompt");
        assert_eq!(prompt.fly(start, 8, Delivery::Prompt), site);
        prompt.assert_consistent(site);
        assert_built_is_lit_correctly(&prompt, &built, "arriving promptly");

        for seed in [1, 2] {
            let mut late = Rig::from_copy_of(&builder, &format!("order-late-{seed}"));
            assert_eq!(late.fly(start, 8, Delivery::Late(seed)), site);
            late.assert_consistent(site);
            assert_built_is_lit_correctly(&late, &built, "arriving late and shuffled");

            assert_eq!(
                late.positions(),
                prompt.positions(),
                "seed {seed}: resident set"
            );
            for pos in prompt.positions() {
                let (a, b) = (
                    prompt.session.world.chunk(pos).unwrap(),
                    late.session.world.chunk(pos).unwrap(),
                );
                assert_eq!(
                    a.serialize(),
                    b.serialize(),
                    "seed {seed}: blocks of {pos:?}"
                );
                assert!(
                    prompt.light_of(pos) == late.light_of(pos),
                    "seed {seed}: light of {pos:?} depends on arrival order"
                );
            }
        }
    }

    /// Over deep ocean, seen from high above, a column's resident chunks are
    /// three separate groups — the seabed, the sea surface, the air around
    /// the camera — with nothing loaded between. A sea-surface chunk holds
    /// no opaque block, so it cannot know it is under open sky until its
    /// seabed arrives and gives the column a height; and nothing adjacent
    /// to it changes when that happens. Only the rule "when a column's
    /// height becomes known, relight its whole stack" (CLAUDE.md) lights
    /// it. Top-down — the order a high camera's nearest-first streaming
    /// really produces — is the one that leaves the sea dark without it;
    /// shuffled arrival nearly always heals by luck.
    #[test]
    fn the_sea_surface_is_lit_whatever_arrives_first() {
        let g = generator();
        let sea = vox_core::SEA_LEVEL_BLOCKS;
        let (x, z) = vox_core::find_spawn(shape(), 0, 0, 256, 64, |x, z| {
            // Deep everywhere the test will load, so every column has the gap.
            (-4..=4).all(|i| (-4..=4).all(|j| g.surface_height(x + i * 32, z + j * 32) < sea - 200))
        })
        .expect("the test world has deep ocean");
        let camera = WorldPos::new(x, sea + 600, z).chunk();

        let orders = [
            ("top-down", Delivery::TopDown),
            ("bottom-up", Delivery::BottomUp),
            ("late", Delivery::Late(3)),
        ];
        for (seed, delivery) in orders {
            let mut rig = Rig::new(&format!("ocean-{seed}"));
            rig.settle(camera, delivery);
            rig.assert_consistent(camera);
            // The cell just above the water, in every loaded column under
            // the camera's own chunk: daylight.
            let surface_layer = vox_core::SEA_SURFACE_CHUNK_Y;
            let mut checked = 0;
            for pos in rig.positions() {
                if pos.y != surface_layer {
                    continue;
                }
                let above = WorldPos::new(pos.origin().x + 7, sea + 1, pos.origin().z + 9);
                assert_eq!(above.chunk(), pos, "the layer above the water is the sea's");
                assert_eq!(
                    rig.sky_at(above),
                    15,
                    "seed {seed}: sea surface dark at {pos:?}"
                );
                checked += 1;
            }
            assert!(checked > 20, "only {checked} sea-surface chunks resident");
            // The three groups really are separate: no chunk between the
            // seabed's window and the surface.
            let gap = rig
                .positions()
                .iter()
                .filter(|p| p.x == camera.x && p.z == camera.z)
                .map(|p| p.y)
                .collect::<Vec<_>>();
            assert!(
                !gap.contains(&(surface_layer - 1)) && gap.iter().any(|&y| y < surface_layer - 1),
                "expected a gap under the sea surface, got layers {gap:?}"
            );
        }
    }

    /// A shaft far underground, reached by the camera and open at the top of
    /// what is loaded around it: the rock that really roofs it is hundreds
    /// of blocks up, in chunks that are not resident, and the only thing
    /// that knows the shaft is not under open sky is the heightmap, from
    /// the surface chunks far above. Those arrive AFTER the shaft when the
    /// camera is down here (nearest first), which is the M05 sealed-cave
    /// bug's own recipe: guess "open" for a column no chunk has reported
    /// yet and the shaft fills with daylight; fail to relight the stack
    /// when the surface arrives and it stays that way. Two rules each
    /// catch it (CLAUDE.md: unknown is covered; a risen column relights
    /// its stack) — breaking both fails this test.
    #[test]
    fn a_deep_shaft_stays_dark_whatever_arrives_first() {
        let surface = land_camera();
        // Twelve layers down: the camera's window (3 either side) and the
        // ground's (3 below it) do not meet.
        let camera = ChunkPos::new(surface.x, surface.y - 12, surface.z);
        let mut builder = Rig::new("shaft-build");
        builder.settle(camera, Delivery::Prompt);

        // 4 x 4 in plan, from one layer below the camera to the very top of
        // the camera's window.
        let origin = camera.origin();
        let (y0, y1) = (
            (camera.y - 1) * CHUNK_SIZE_I,
            (camera.y + CAMERA_WINDOW_LAYERS + 1) * CHUNK_SIZE_I - 1,
        );
        let mut shaft = Vec::new();
        for x in origin.x + 10..origin.x + 14 {
            for z in origin.z + 10..origin.z + 14 {
                for y in y0..=y1 {
                    let p = WorldPos::new(x, y, z);
                    assert!(
                        builder.session.world.chunk(p.chunk()).is_some(),
                        "{p:?} resident"
                    );
                    builder.session.set_block(p, BlockId::AIR);
                    shaft.push(p);
                }
            }
        }
        let assert_dark = |rig: &Rig, how: &str| {
            for p in &shaft {
                assert_eq!(
                    rig.sky_at(*p),
                    0,
                    "{how}: daylight in the deep shaft at {p:?}"
                );
            }
            // The camera's chunks and the ground's really are apart.
            let layers: Vec<i64> = rig
                .positions()
                .iter()
                .filter(|p| p.x == camera.x && p.z == camera.z)
                .map(|p| p.y)
                .collect();
            let top = camera.y + CAMERA_WINDOW_LAYERS + 1;
            assert!(
                !layers.contains(&(top + 1)) && layers.iter().any(|&y| y > top + 1),
                "{how}: expected unloaded rock above the shaft, got layers {layers:?}"
            );
        };
        builder.settle(camera, Delivery::Prompt);
        builder.assert_consistent(camera);
        assert_dark(&builder, "where it was dug");
        builder.session.save_unsaved("test");

        let orders = [
            ("bottom-up", Delivery::BottomUp),
            ("top-down", Delivery::TopDown),
            ("late", Delivery::Late(5)),
        ];
        for (name, delivery) in orders {
            let mut rig = Rig::from_copy_of(&builder, &format!("shaft-{name}"));
            rig.settle(camera, delivery);
            rig.assert_consistent(camera);
            assert_dark(&rig, name);
        }
    }

    /// A block placed high above the ground shades the ground under it, and
    /// taking it away gives the daylight back — at once, and still after the
    /// player has come down and the chunks that held it have unloaded.
    ///
    /// This is the case that decides audit finding D1. The heightmap is the
    /// only thing that knows what is above a column once the chunks up
    /// there are gone, so if removing the block does not LOWER the height
    /// (the "raise-only" rule CLAUDE.md states for edits), the column keeps
    /// the shadow of a block that no longer exists for as long as any of
    /// its chunks stay loaded. The code lowers it; with that one line
    /// removed, this test fails at the last assertion.
    #[test]
    fn removing_a_high_block_returns_daylight_to_the_ground() {
        let ground_camera = land_camera();
        // Six layers up: the camera's window reaches well above the
        // ground's, and still touches it.
        let high_camera = ChunkPos::new(ground_camera.x, ground_camera.y + 6, ground_camera.z);
        let mut rig = Rig::new("high-block");
        rig.settle(high_camera, Delivery::Prompt);

        let origin = ground_camera.origin();
        let (x, z) = (origin.x + 16, origin.z + 16);
        let ground = top_solid(&rig, x, z);
        let above_ground = WorldPos::new(x, ground.y + 1, z);
        assert_eq!(
            rig.sky_at(above_ground),
            15,
            "open ground starts in daylight"
        );

        let high = WorldPos::new(x, (high_camera.y + 2) * CHUNK_SIZE_I + 5, z);
        assert!(
            rig.session.world.chunk(high.chunk()).is_some(),
            "the high chunk is resident"
        );
        assert!(
            high.y > ground.y + 4 * CHUNK_SIZE_I,
            "well above the ground's own window"
        );
        rig.session.set_block(high, vox_core::registry::STONE);
        rig.settle(high_camera, Delivery::Prompt);
        rig.assert_consistent(high_camera);
        assert_eq!(
            rig.sky_at(above_ground),
            14,
            "directly under the block: lit from beside"
        );

        rig.session.set_block(high, BlockId::AIR);
        rig.settle(high_camera, Delivery::Prompt);
        rig.assert_consistent(high_camera);
        assert_eq!(
            rig.sky_at(above_ground),
            15,
            "the block is gone: daylight is back"
        );

        // Come down. The chunk that held the block unloads; the ground's
        // chunks stay, and so does their column's height.
        rig.settle(ground_camera, Delivery::Prompt);
        assert!(
            rig.session.world.chunk(high.chunk()).is_none(),
            "the high chunk unloaded"
        );
        rig.assert_consistent(ground_camera);
        assert_eq!(
            rig.sky_at(above_ground),
            15,
            "no shadow is left by a block that no longer exists"
        );
    }

    /// The highest solid block of world column `(x, z)` among resident
    /// chunks, with its position.
    fn top_solid(rig: &Rig, x: i64, z: i64) -> WorldPos {
        let s = &rig.session;
        let mut layers = s
            .column_heights
            .resident_layers(x.div_euclid(CHUNK_SIZE_I), z.div_euclid(CHUNK_SIZE_I))
            .to_vec();
        layers.sort_unstable_by(|a, b| b.cmp(a));
        for cy in layers {
            for y in (cy * CHUNK_SIZE_I..(cy + 1) * CHUNK_SIZE_I).rev() {
                let p = WorldPos::new(x, y, z);
                if s.registry.is_solid(s.world.get_block(p)) {
                    return p;
                }
            }
        }
        panic!("column ({x}, {z}) has no solid block resident");
    }

    /// Edits across a chunk seam: a pit dug on both sides of a chunk border
    /// and a roof laid over part of it. After the world settles the light is
    /// a fixed point, and it is the light a fresh start computes for the
    /// same blocks — so nothing about HOW the edits happened is left in it.
    /// Then the edits survive a save and a restart, block for block.
    #[test]
    fn edits_across_a_seam_light_correctly_and_survive_a_restart() {
        let camera = land_camera();
        let mut rig = Rig::new("edits");
        rig.settle(camera, Delivery::Prompt);

        // A chunk border in X under the camera: x = 31 | 32 of the origin.
        let origin = camera.origin();
        let (bx, bz) = (origin.x + CHUNK_SIZE_I, origin.z + 16);
        let registry = BlockRegistry::default_set();
        let stone = vox_core::registry::STONE;
        assert!(registry.is_solid(stone));

        // Dig a pit 4 wide, 3 long and 6 deep straddling the border.
        let mut edited = Vec::new();
        for x in bx - 2..bx + 2 {
            for z in bz..bz + 3 {
                let top = top_solid(&rig, x, z);
                for depth in 0..6 {
                    let p = WorldPos::new(x, top.y - depth, z);
                    rig.session.set_block(p, BlockId::AIR);
                    edited.push(p);
                }
            }
        }
        // Roof half of it, two blocks above the old surface, again on both
        // sides of the border.
        for x in bx - 2..bx + 2 {
            let top = top_solid(&rig, x, bz - 1);
            let p = WorldPos::new(x, top.y + 2, bz);
            rig.session.set_block(p, stone);
            edited.push(p);
        }
        assert!(
            edited
                .iter()
                .map(|p| p.chunk().x)
                .collect::<HashSet<_>>()
                .len()
                >= 2,
            "the edits must straddle a chunk border"
        );
        rig.settle(camera, Delivery::Prompt);
        rig.assert_consistent(camera);

        // The pit's floor under open sky is in daylight; under the roof,
        // dimmer.
        let open = top_solid(&rig, bx - 1, bz + 2);
        let s = &rig.session;
        let sky_above = |p: WorldPos| {
            let above = WorldPos::new(p.x, p.y + 1, p.z);
            let (c, l) = above.split();
            s.world.chunk(c).expect("resident").sky_light(l)
        };
        assert_eq!(
            sky_above(open),
            15,
            "the open pit floor is in full daylight"
        );
        let roofed = (0..40)
            .map(|d| WorldPos::new(bx - 1, open.y + 12 - d, bz))
            .skip_while(|p| !s.registry.is_solid(s.world.get_block(*p))) // the roof
            .skip_while(|p| s.registry.is_solid(s.world.get_block(*p)))
            .find(|p| s.registry.is_solid(s.world.get_block(*p)))
            .expect("ground under the roof");
        assert!(
            sky_above(roofed) < 15,
            "the pit floor under the roof is shaded (got {})",
            sky_above(roofed)
        );

        // Save, as the autosave does. Nothing is left unsaved, and a second
        // save writes nothing.
        let before: HashMap<ChunkPos, (Vec<u8>, Vec<u8>)> = rig
            .positions()
            .into_iter()
            .map(|p| {
                let blocks = rig.session.world.chunk(p).unwrap().serialize();
                (p, (blocks, rig.light_of(p)))
            })
            .collect();
        rig.session.save_unsaved("test");
        assert!(rig.session.world.chunks().all(|(_, c)| !c.is_unsaved()));
        assert!(
            !rig.session.edits_unsaved,
            "the LOD overlay was written too"
        );

        // Start again from the folder.
        let mut again = rig.reopen();
        again.settle(camera, Delivery::Late(7));
        again.assert_consistent(camera);
        assert_eq!(again.positions().len(), before.len());
        let mut from_disk = 0;
        for pos in again.positions() {
            let chunk = again.session.world.chunk(pos).unwrap();
            let (blocks, light) = &before[&pos];
            assert_eq!(
                &chunk.serialize(),
                blocks,
                "blocks of {pos:?} after a restart"
            );
            assert!(
                &again.light_of(pos) == light,
                "light of {pos:?} differs between the session that made the edits and a restart"
            );
            from_disk += usize::from(chunk.is_modified());
        }
        assert!(from_disk >= 2, "the edited chunks came back from disk");
        for p in &edited {
            assert!(
                again.session.edited_columns.get(p.x, p.z).is_some(),
                "the LOD overlay remembers column ({}, {})",
                p.x,
                p.z
            );
        }
    }
}
