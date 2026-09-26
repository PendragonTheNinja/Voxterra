//! vox-app: the game binary. Window, event loop, fly camera, and the
//! Milestone 00 test scene: one sine-wave chunk.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec3, Mat4, Vec3};
use rayon::prelude::*;
use vox_core::{
    cell_overlaps_aabb, BlockId, BlockRegistry, Chunk, ChunkPos, ColumnHeights, EditedColumns,
    LocalPos, LodNodeId, LodRing, RayHit, Streamer, World, WorldMeta, WorldPos, WorldShape,
    WorldStore,
};
use vox_mesh::{mesh_chunk, ChunkNeighbors, LodMeshData, MeshData};

mod settings_ui;
use settings_ui::SettingsUi;
use vox_render::Renderer;
use vox_worldgen::{Generator, LodSampling};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

// ---------------------------------------------------------------------------
// Block appearance — sourced from the vox-core block registry (M03 task 1).
// Flat per-block color is interim; task 2 replaces it with textures.
// ---------------------------------------------------------------------------

// The six face-neighbor offsets, shared by streaming and edit re-meshing.
const NEIGHBOR_OFFSETS: [(i64, i64, i64); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Streaming radii, in chunks (ADR-0002 / M02). Chunks within `LOAD_RADIUS`
/// of the camera's chunk are generated/loaded; chunks beyond `UNLOAD_RADIUS`
/// are dropped. The gap between them is hysteresis to prevent boundary
/// thrash. With 32-block chunks, radius 8 ≈ 256 blocks of full-res world in
/// every direction (~2,000 chunks resident).
const LOAD_RADIUS: i64 = 8;
const UNLOAD_RADIUS: i64 = 10;

// --- LOD (M09). LOD is configured as a list of levels (finest to coarsest). Radii are in
// CHUNKS from the camera and must be multiples of the coarsest stride (the ring
// asserts this). LOD starts at 0 — it underlaps the full-res region, which
// simply draws on top; see the vox-core::lod docs on why a flush inner edge
// would open a gap. Multi-level tuning lands with the octree wiring; today this
// is the single level M08 shipped.
const LOD_INNER_CHUNKS: i64 = 0;
// Finest to coarsest. Radii are CHUNKS from the camera and must be multiples of
// the coarsest stride (the ring asserts it).
//
// Sizing follows the standard screen-space-error idea: a cell should subtend
// roughly constant pixels, so each level's switch distance scales with its cell
// size. Here every level runs out to 256 blocks of distance per block of cell
// size (stride 2 -> 512 blocks, stride 4 -> 1024, stride 8 -> 2048) — twice the
// detail-per-distance of the first M09 pass, which read as too chunky too close.
// Raise the multiplier for finer distant terrain at the cost of node count
// (nodes per level are constant, so adding detail means adding levels).
const LOD_LEVELS: [vox_core::LodLevel; 3] = [
    vox_core::LodLevel {
        stride: 2,
        outer_chunks: 16,
    },
    vox_core::LodLevel {
        stride: 4,
        outer_chunks: 32,
    },
    vox_core::LodLevel {
        stride: 8,
        outer_chunks: 64,
    },
];
const LOD_UNLOAD_MARGIN_CHUNKS: i64 = 8; // multiple of the coarsest stride
                                         // World vertical band LOD must cover. ONE node spans it at every level: cells
                                         // are `stride` blocks wide but `band/32` blocks tall, so horizontal detail and
                                         // vertical resolution are independent. Widen if worldgen's range grows (the
                                         // terrain currently spans about -59..108).
/// The world's vertical extent, and therefore the LOD nodes' (M10 task 3).
///
/// Opened from 256 blocks to ~20 000 so Everest (+8 848) and Challenger Deep
/// (−10 935) both fit at one block per metre. This is affordable ONLY because
/// streaming follows the surface (task 2): nothing loads a full column, so the
/// world's height costs nothing. Anything that still walks this whole band per
/// column is a bug — see `surface_window_chunks`.
const LOD_WORLD_Y_BLOCKS: (i64, i64) = (vox_core::WORLD_Y_MIN_BLOCKS, vox_core::WORLD_Y_MAX_BLOCKS);

// Fog (M09 amendment A2) is tuned live from the settings menu; its defaults
// live in `vox_core::Settings`.
const LOD_SKIRT_DEPTH_CELLS: i32 = 3; // multiplied by stride for block depth
                                      // Coarse nodes generated+meshed per frame. Sized so a full ring (up to ~2400
                                      // nodes with extra levels) fills in well under a second instead of appearing in
                                      // visibly staggered rings; the work is on the rayon pool, not the frame thread.
const LOD_SPAWN_BUDGET: usize = 48;

/// Chunk layers kept loaded below and above each column's terrain surface
/// (M10 task 2).
///
/// Streaming follows the surface rather than filling a world-height band,
/// because M10 opens the world to ~640 chunk layers and a full-height cylinder
/// at that scale is ~80x more chunks than the engine can carry. Below covers
/// standing on and digging into the ground; above covers building and the
/// near-ground flight envelope. Fly far higher than `ABOVE` and the terrain
/// below you stays loaded — this window follows the GROUND.
const LOAD_BELOW_CHUNKS: i64 = 3;
const LOAD_ABOVE_CHUNKS: i64 = 3;
/// Chunk layers kept loaded above and below the CAMERA's layer, across the
/// whole load disc (M10 A3).
///
/// The surface window alone strands a player who leaves it: dig more than
/// `LOAD_BELOW_CHUNKS` down or build more than `LOAD_ABOVE_CHUNKS` up and you
/// walk out of the loaded world. Near the ground this window overlaps the
/// surface one and costs nothing; underground it is ~100 blocks of view each
/// way, the same reach the surface window gives the ground.
const LOAD_AROUND_CAMERA_CHUNKS: i64 = 3;

/// Spacing and reach of the spawn search, in blocks and rings (M10 task 1).
///
/// Coarse on purpose: landmasses are tens of thousands of blocks across, so a
/// 256-block step cannot miss one, and each candidate costs a full
/// `surface_height` evaluation. A 64-block step would be four times the
/// fidelity for sixteen times the startup cost, to answer a question whose
/// answer is "somewhere on that continent".
const SPAWN_SEARCH_STRIDE: i64 = 256;
const SPAWN_SEARCH_RINGS: i64 = 512;
/// How far above the ground the camera starts.
const SPAWN_EYE_HEIGHT: f64 = 12.0;

/// How many `lod_tick`s a retired LOD node keeps drawing while its replacement
/// builds, before being dropped anyway.
///
/// Retirement exists because unloading a node the instant the ring stops
/// wanting it leaves a HOLE: the replacement takes frames to generate and mesh,
/// and until it arrives you see fog through the ground. A snapped-centre jump
/// retires ~150 nodes at once, so the hole is a visible flash across the whole
/// level-0 disc.
///
/// Normally retirees are dropped as soon as the pending queue drains, which is
/// well inside this cap. The cap only bites while flying fast enough that the
/// queue never empties, and bounds how much stale geometry can accumulate.
const LOD_RETIRE_MAX_TICKS: u32 = 60;

/// Per-frame work budgets, to keep frame time stable while streaming.
/// Generation is async (rayon), but we bound how many jobs we *spawn* per
/// frame so a big initial load fills in progressively rather than spiking.
const GEN_SPAWN_BUDGET: usize = 64;
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
const MESH_SUBBATCH: usize = 8;
const RELIGHT_SUBBATCH: usize = 32;
const MESH_TIME_MS: f32 = 4.0;
const RELIGHT_TIME_MS: f32 = 3.0;
/// Chunk side length as i64, for heightmap/world-coordinate math.
const CHUNK_SIZE_I: i64 = vox_core::CHUNK_SIZE as i64;

/// How far (world units / blocks) the targeting raycast reaches from the
/// camera for break/place interaction (M03).
const REACH: f64 = 6.0;

/// Translate every vertex of a locally-meshed chunk into world space by
/// adding the chunk's origin. The mesher emits positions in 0..32 local
/// space; this is where the chunk's world offset is baked in (see the
/// Milestone 01 spec's note on f32 precision far from origin — fine at
/// this scale, revisited via a future ADR before continent-scale worlds).
/// Mesh the given chunk positions in parallel (rayon), returning one
/// `(pos, mesh)` per non-empty result. Meshes are in LOCAL chunk space
/// (0..32); world placement is done by the renderer's per-chunk offset
/// (floating origin, ADR-0002), so no world offset is baked here. Each task
/// borrows its chunk and its six neighbors immutably from `world` — no
/// shared mutable state — which is why [`ChunkNeighbors`] takes borrowed
/// chunks rather than `&World` by value.
/// Map a number-row key to a 1-based block-selection slot (1..=6).
fn digit_slot(code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Digit1 => Some(1),
        KeyCode::Digit2 => Some(2),
        KeyCode::Digit3 => Some(3),
        KeyCode::Digit4 => Some(4),
        KeyCode::Digit5 => Some(5),
        KeyCode::Digit6 => Some(6),
        _ => None,
    }
}

/// Build a chunk's heightmap-derived `top_sky` (CHUNK_SIZE², daylight
/// entering each column from directly above: 15 open, 0 occluded). Free fn so
/// it can run inside the parallel relight workers rather than serially on the
/// main thread (the serial version was ~12 ms/frame at full budget).
fn top_sky_from_heightmap(heights: &ColumnHeights, pos: ChunkPos) -> Vec<u8> {
    let origin = pos.origin();
    let mut top = vec![0u8; (CHUNK_SIZE_I * CHUNK_SIZE_I) as usize];
    for lz in 0..CHUNK_SIZE_I {
        for lx in 0..CHUNK_SIZE_I {
            // Distinguish a KNOWN column height from an UNKNOWN one. A column is
            // unknown when its solid chunks have not streamed in yet (no entry
            // in the heightmap). An unknown column must be treated as COVERED
            // (top_sky = 0), never as open: assuming "open" here injects full
            // daylight that the uniform-air fast path then commits straight down
            // a column that may actually be sealed/underground, and because the
            // heightmap is raise-only and relight is local, that bogus daylight
            // gets frozen into already-lit chunks (the sealed-hole leak). It is
            // always safe to start an unknown column dark and let it brighten
            // honestly once the real terrain loads and relights it.
            let known_h = heights.get(origin.x + lx, origin.z + lz);

            // Inject full daylight at the chunk's TOP FACE only when the column
            // is KNOWN and open through this ENTIRE chunk — i.e. the highest
            // solid is below the chunk's floor. Then 15 legitimately fills the
            // column top-to-bottom (it's all air to the surface, which lies
            // below).
            //
            // When the surface lies INSIDE this chunk (origin.y <= h), we must
            // NOT blanket the ceiling with 15 — that would flood daylight down
            // past the surface into sealed air below it (the cave-lit bug).
            // Instead leave the top face at 0 here; the real daylight enters
            // from the chunk ABOVE via its -Y sky plane (the +Y neighbor border)
            // and BFS-floods down only through actual air, stopping at the
            // surface solid. Sealed shafts below the surface stay dark.
            top[(lx + lz * CHUNK_SIZE_I) as usize] = match known_h {
                Some(h) if h < origin.y => vox_core::MAX_LIGHT,
                _ => 0,
            };
        }
    }
    top
}

/// Compute both light channels for many chunks in parallel (rayon),
/// read-only. Each worker builds its own heightmap-derived `top_sky` (so that
/// work is parallel, not serial), reads neighbor borders, and returns the
/// packed buffer plus `interior_changed` (→ needs re-mesh) and
/// `border_changed` (→ neighbors need a relight check). Skylight's vertical
/// fill comes from `top_sky`, so there's no dependence on the +Y neighbor
/// being relit first — no vertical cascade (ADR-0005).
#[allow(clippy::type_complexity)]
fn relight_chunks_parallel(
    world: &World,
    registry: &BlockRegistry,
    heights: &ColumnHeights,
    positions: &[ChunkPos],
) -> Vec<(ChunkPos, Vec<u8>, bool, u8)> {
    const OPPOSITE: [usize; 6] = [1, 0, 3, 2, 5, 4];
    positions
        .par_iter()
        .filter_map(|&pos| {
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
            let interior_changed = (0..light.len()).any(|i| {
                let p = LocalPos::from_index(i);
                let cur = (chunk.sky_light(p) << 4) | (chunk.block_light(p) & 0x0F);
                cur != light[i]
            });
            // Which faces actually changed — so the consumer requeues only the
            // neighbor across a changed face, not all six (up to 6x less
            // relight/remesh cascade during streaming convergence).
            let border_faces = if border_changed {
                vox_core::packed_border_changed_faces(chunk, &light)
            } else {
                0
            };
            Some((pos, light, interior_changed, border_faces))
        })
        .collect()
}

/// `jobs` pairs each chunk with its SEALED faces: a bit per entry of
/// `NEIGHBOR_OFFSETS`, set where that face neighbour is absent and never
/// coming (M10 A3). The mesher continues the chunk's own edge there: no face
/// into the void, and edge light and AO match the interior instead of fading
/// toward an unlit nothing (the dark line around the loaded disc). The
/// judgement needs the streamer, so the caller makes it on the main thread;
/// the workers only read it.
fn mesh_chunks_parallel(
    world: &World,
    registry: &BlockRegistry,
    jobs: &[(ChunkPos, u8)],
) -> Vec<(ChunkPos, MeshData)> {
    jobs.par_iter()
        .filter_map(|&(pos, sealed)| {
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
            let mesh = mesh_chunk(chunk, &neighbors, |b, face| registry.face_layer(b, face));
            if mesh.is_empty() {
                None
            } else {
                Some((pos, mesh))
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fly camera
// ---------------------------------------------------------------------------

/// How the player moves through the world.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MoveMode {
    /// Free-fly noclip (the original behavior): WASD relative to yaw,
    /// Space/Shift for vertical, Ctrl to sprint. Passes through blocks.
    Spectator,
    /// Grounded survival movement: gravity, jumping, AABB collision against
    /// solid blocks, Minecraft-style auto-step over single-block ledges.
    Survival,
}

/// The player's eye and body.
///
/// Everything here is `f64` (CLAUDE.md, ADR-0002): the camera can be hundreds
/// of kilometres from the origin in the unwrapped frame, where an `f32`
/// resolves only ~0.01 blocks and a high-frame-rate step rounds away to
/// nothing. Narrowing to `f32` happens in exactly one place —
/// [`FlyCamera::render_relative`], after subtracting the render origin.
struct FlyCamera {
    /// Eye position in world blocks, in the unwrapped frame (ADR-0012 §4):
    /// never canonicalised. In survival the player AABB hangs below this by
    /// `EYE_HEIGHT`.
    position: DVec3,
    /// Radians. 0 looks along +X; positive turns toward +Z.
    yaw: f64,
    /// Radians, clamped to ±~89° so the view never flips.
    pitch: f64,
    /// World-space velocity (m/s). Used by survival physics; spectator ignores
    /// it (moves position directly).
    velocity: DVec3,
    /// Whether the player is standing on solid ground (survival).
    on_ground: bool,
    mode: MoveMode,
}

impl FlyCamera {
    const SPEED: f64 = 30.0; // m/s (spectator)
    const SPRINT_MULTIPLIER: f64 = 4.0; // hold Ctrl (spectator)
    const SENSITIVITY: f64 = 0.0022; // radians per mouse count
    const PITCH_LIMIT: f64 = 1.55; // just under PI/2

    // --- Survival tuning (Minecraft-like) ---
    /// Player collision box: 0.6 × 1.8 × 0.6 blocks.
    const HALF_WIDTH: f64 = 0.3;
    const HEIGHT: f64 = 1.8;
    /// Eye sits near the top of the box (MC eye height ~1.62).
    const EYE_HEIGHT: f64 = 1.62;
    const WALK_SPEED: f64 = 4.317; // m/s, MC walking
    const SPRINT_SPEED: f64 = 5.612; // m/s, MC sprinting
    const GRAVITY: f64 = 28.0; // m/s² (tuned for snappy MC-ish fall)
    const JUMP_SPEED: f64 = 9.0; // m/s initial (clears ~1.25 blocks)
    const STEP_HEIGHT: f64 = 0.6; // auto-step over single blocks
    const TERMINAL_FALL: f64 = 78.0; // m/s clamp

    fn forward(&self) -> DVec3 {
        DVec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
    }

    /// The block containing the eye. Streaming, the render origin and the
    /// debug tools all key off this; floor, not truncation, so negative
    /// coordinates land in the right block.
    fn block_pos(&self) -> WorldPos {
        WorldPos::new(
            self.position.x.floor() as i64,
            self.position.y.floor() as i64,
            self.position.z.floor() as i64,
        )
    }

    /// The eye relative to `render_origin`, narrowed to `f32` for the GPU.
    ///
    /// The ONLY place the camera becomes `f32` (ADR-0002): the subtraction
    /// happens in `f64`, so the result is a small number that `f32` holds
    /// exactly enough, however far out the camera is.
    fn render_relative(&self, render_origin: WorldPos) -> Vec3 {
        let origin = DVec3::new(
            render_origin.x as f64,
            render_origin.y as f64,
            render_origin.z as f64,
        );
        (self.position - origin).as_vec3()
    }

    fn mouse_look(&mut self, dx: f64, dy: f64) {
        self.yaw += dx * Self::SENSITIVITY;
        self.pitch =
            (self.pitch - dy * Self::SENSITIVITY).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }

    /// Spectator free-fly: move the eye directly, no collision.
    fn update_spectator(&mut self, keys: &HashSet<KeyCode>, dt: f64) {
        // Horizontal movement follows yaw only (classic fly-cam feel);
        // Space/Shift move straight up/down in world space.
        let flat_forward = DVec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let right = DVec3::new(-self.yaw.sin(), 0.0, self.yaw.cos());

        let mut dir = DVec3::ZERO;
        if keys.contains(&KeyCode::KeyW) {
            dir += flat_forward;
        }
        if keys.contains(&KeyCode::KeyS) {
            dir -= flat_forward;
        }
        if keys.contains(&KeyCode::KeyD) {
            dir += right;
        }
        if keys.contains(&KeyCode::KeyA) {
            dir -= right;
        }
        if keys.contains(&KeyCode::Space) {
            dir += DVec3::Y;
        }
        if keys.contains(&KeyCode::ShiftLeft) {
            dir -= DVec3::Y;
        }

        if dir != DVec3::ZERO {
            let speed = if keys.contains(&KeyCode::ControlLeft) {
                Self::SPEED * Self::SPRINT_MULTIPLIER
            } else {
                Self::SPEED
            };
            self.position += dir.normalize() * speed * dt;
        }
    }

    /// The player AABB (min, max) in world blocks, derived from the eye.
    fn aabb(&self) -> ([f64; 3], [f64; 3]) {
        let feet_y = self.position.y - Self::EYE_HEIGHT;
        let min = [
            self.position.x - Self::HALF_WIDTH,
            feet_y,
            self.position.z - Self::HALF_WIDTH,
        ];
        let max = [
            self.position.x + Self::HALF_WIDTH,
            feet_y + Self::HEIGHT,
            self.position.z + Self::HALF_WIDTH,
        ];
        (min, max)
    }

    /// View-projection matrix built with the camera positioned **relative to
    /// the render origin** (ADR-0002). `render_origin` is the world position
    /// of the render origin; subtracting it (in `f64`) keeps the numbers fed
    /// to the matrix small regardless of absolute distance.
    /// `fov_degrees` and `far` come from settings: the far plane MUST cover the
    /// LOD horizon, or distant terrain is generated, meshed, uploaded — and
    /// then clipped away by the projection, which reads as the world ending at
    /// a hard line and as a "render distance" slider that does nothing.
    fn view_proj(&self, aspect: f32, render_origin: WorldPos, fov_degrees: f32, far: f32) -> Mat4 {
        // The renderer's projection, not glam's directly: depth is reversed-Z
        // and its compares expect exactly this mapping.
        let proj = vox_render::perspective(fov_degrees.to_radians(), aspect, 0.1, far);
        let rel_pos = self.render_relative(render_origin);
        let view = Mat4::look_to_rh(rel_pos, self.forward().as_vec3(), Vec3::Y);
        proj * view
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: World,
    camera: FlyCamera,
    keys: HashSet<KeyCode>,
    cursor_captured: bool,
    last_frame: Instant,

    // --- Streaming (M02 task 3) ---
    generator: Generator,
    streamer: Streamer,
    /// Coarse LOD ring policy (M08): which distant LOD nodes are loaded.
    lod_ring: LodRing,

    // --- Settings menu (M09 amendment A3) ---
    /// Live-tunable values; the menu edits these and the loop applies them.
    settings: vox_core::Settings,
    /// Settings as of last frame, to detect edits that need a re-stream.
    settings_applied: vox_core::Settings,
    /// egui context + menu state. Created with the window in `resumed`.
    ui: Option<SettingsUi>,
    /// Outermost LOD radius in chunks; sizes the camera far plane.
    lod_far_chunks: i64,
    /// Block definitions: appearance + flags, the single source of truth
    /// (M03 task 1). Shared into meshing (Sync).
    registry: BlockRegistry,
    /// On-disk world: generate-or-load on chunk-in, save modified on
    /// chunk-out (M02 task 5).
    store: WorldStore,
    /// Chunks whose generation has been spawned but whose result hasn't been
    /// received yet — prevents re-spawning the same chunk every frame.
    gen_in_flight: HashSet<ChunkPos>,
    /// Chunks needing a (re)mesh: newly generated chunks and any loaded
    /// neighbor whose border faces may have changed.
    dirty: HashSet<ChunkPos>,
    /// Chunks needing a light recompute, kept separate from `dirty` so that
    /// cross-chunk light convergence does cheap parallel relighting without
    /// forcing an expensive re-mesh of every rippled chunk (ADR-0005).
    relight: HashSet<ChunkPos>,
    /// Chunks that have been meshed at least once (M09 task 1). A chunk's
    /// FIRST mesh is deferred until its still-pending neighbors arrive, so it
    /// is never baked dark/with wrong border faces from incomplete data; later
    /// re-meshes are unrestricted. Pruned with the other per-chunk sets.
    meshed_once: HashSet<ChunkPos>,
    /// Per-column heightmap: world `(x, z)` → highest solid block world-Y
    /// among loaded chunks. Drives the skylight top boundary directly, so each
    /// chunk computes its daylight in one pass without waiting on its vertical
    /// neighbors to be relit (avoids the skylight cascade; ADR-0005). Unknown
    /// column = COVERED. Bounded by residency: a chunk column's heights are
    /// dropped with its last resident chunk (M10 A3), so every insert into and
    /// removal from `world` must be reported to it.
    column_heights: ColumnHeights,
    /// Columns the player has dug or built, as a sparse overlay on the seed
    /// heights every LOD node starts from (M09 ghost-block fix).
    ///
    /// `column_heights` cannot serve this: it only knows columns whose chunks
    /// have loaded, and consulting it per column would cost 32x32xstride^2
    /// lookups per node (65 536 at stride 8), nearly all misses. Shared into
    /// the rayon build jobs behind an `Arc` — edits are rare, node builds are
    /// constant, so copy-on-write is the cheap direction. Loaded from and saved
    /// with the world, so earlier sessions' edits reach the LOD too.
    edited_columns: Arc<EditedColumns>,
    /// `edited_columns` has changed since it was last saved. Flushed wherever
    /// chunks are saved, so the overlay on disk is never behind the chunks it
    /// describes.
    edits_unsaved: bool,
    /// Block currently under the crosshair (raycast result), or none.
    targeted: Option<RayHit>,
    /// Block type placed on right-click (M03 task 4). Cycled with number
    /// keys / scroll among the registry's placeable blocks.
    selected_block: BlockId,

    /// Async generation results arrive here from the rayon pool.
    gen_tx: Sender<(ChunkPos, Chunk)>,
    gen_rx: Receiver<(ChunkPos, Chunk)>,

    /// Async LOD-node gen+mesh results (M08): coarse generation and meshing run
    /// on the rayon pool; the main thread only uploads.
    lod_tx: Sender<(LodNodeId, LodMeshData)>,
    lod_rx: Receiver<(LodNodeId, LodMeshData)>,
    lod_in_flight: HashSet<LodNodeId>,
    /// Nodes the ring no longer wants, whose meshes are still drawn.
    ///
    /// A node's geometry depends only on its own id and the terrain, never on
    /// the camera, so a retired mesh stays CORRECT indefinitely — it is only
    /// redundant. That makes it safe to keep on screen until the replacement
    /// exists, and safe to adopt again unchanged if the ring asks for it back.
    lod_retired: HashSet<LodNodeId>,
    /// Ticks since the last retirement flush, against `LOD_RETIRE_MAX_TICKS`.
    lod_retire_age: u32,
    /// Debug (K): build LOD nodes with or without border skirts. A diagnostic
    /// for the faint grid lines on distant terrain (M10 A3): if they vanish
    /// with skirts off, they are skirts losing the depth test to their
    /// neighbour's top face. Not a setting; always starts on.
    lod_skirts: bool,
    /// Nodes wanted but not yet spawned (the ring is requested all at once but
    /// generated a few per frame). Drained nearest-camera-first, so this is a
    /// set rather than a queue — insertion order carries no meaning.
    lod_pending_set: HashSet<LodNodeId>,

    /// Telemetry: accumulate frames over ~1s to log FPS + drawn/total.
    telemetry_accum: f32,
    /// Milliseconds spent in the relight / mesh streaming passes since the
    /// last telemetry line — shows where frame time goes during bursts.
    relight_ms_accum: f32,
    mesh_ms_accum: f32,
    telemetry_frames: u32,
    /// Longest single frame in the current telemetry interval, in ms. Average
    /// fps hides hitches — a 144 fps average with one 40 ms frame reads as
    /// smooth in the average and feels like a stutter. This is the number to
    /// watch when optimizing streaming.
    worst_frame_ms: f32,

    // --- Day/night (M07 task 3, ADR-0007) ---
    /// Current world time; drives sky brightness, and later sun/moon.
    world_time: vox_core::WorldTime,
    /// Fractional game-tick accumulator so slow real frames still advance time
    /// smoothly without integer rounding drift.
    time_accum: f64,
    /// Debug: freeze time (key `T`).
    time_paused: bool,
    /// Debug: 60× fast-forward so a full cycle takes ~24s (key `\`).
    time_fast: bool,
}

impl Default for App {
    fn default() -> Self {
        let (gen_tx, gen_rx) = std::sync::mpsc::channel();
        let (lod_tx, lod_rx) = std::sync::mpsc::channel();

        // Open (or create) the world directory. What `world.meta` records is
        // authoritative: a new world is created with these values, an existing
        // one keeps its own seed and size so terrain regenerates identically.
        // A world made by a different terrain generator is refused rather than
        // opened with its edited chunks stranded in new terrain.
        const DEFAULT_SEED: u64 = 0x0007_E22A_C0DE;
        let new_world = WorldMeta {
            seed: DEFAULT_SEED,
            shape: WorldShape::DEFAULT,
            generator_version: vox_worldgen::GENERATOR_VERSION,
        };
        // Display, not `expect`: `expect` prints the error's Debug form, which
        // turns "this world predates…; move or delete it" into `LegacyWorld`.
        let store = WorldStore::open("world", new_world)
            .unwrap_or_else(|e| panic!("cannot open the world at \"world\": {e}"));
        let seed = store.seed();
        let shape = store.shape();
        // The LOD's record of every edit ever made in this world. A corrupt
        // overlay costs only the distant display of old edits — the chunks
        // hold the edits themselves — so it is reported and replaced, not
        // fatal. It is rewritten from scratch at the next save.
        let edited_columns = store.load_edited_columns().unwrap_or_else(|e| {
            log::error!("{e}; distant terrain will not show earlier edits");
            EditedColumns::new(shape)
        });
        log::info!(
            "world at {:?}, seed {:#x}, {} x {} km",
            store.root(),
            seed,
            shape.size_x() / 1000,
            shape.size_z() / 1000
        );

        // Find land before placing the camera. The world centre is on the
        // equator by construction, but since M10 it is as likely to be ocean as
        // anything else — and the old hard-coded (0, 60, 0) left the player
        // hanging thousands of blocks above a seabed with nothing in view.
        //
        // Streaming follows the SURFACE, so a camera far from any surface loads
        // terrain it cannot see: the chunks are resident, just 3 km below.
        let generator = Generator::new(seed, shape);
        let (spawn_x, spawn_z) = vox_core::find_spawn(
            shape,
            0,
            0,
            SPAWN_SEARCH_STRIDE,
            SPAWN_SEARCH_RINGS,
            |x, z| generator.surface_height(x, z) > vox_core::SEA_LEVEL_BLOCKS + 4,
        )
        .unwrap_or((0, 0));
        let spawn_y = generator.surface_height(spawn_x, spawn_z) as f64 + SPAWN_EYE_HEIGHT;
        // The search returns the unwrapped position near the origin, which is
        // where the camera goes; the log shows the canonical one, since that is
        // the address a player would recognise and return to.
        log::info!(
            "spawn at ({}, {:.0}, {}), latitude {:.1} deg",
            shape.canonical_x(spawn_x),
            spawn_y,
            shape.canonical_z(spawn_z),
            shape.latitude_degrees(spawn_z)
        );

        Self {
            window: None,
            renderer: None,
            world: World::new(),
            // On the ground at the resolved spawn, looking out across it.
            camera: FlyCamera {
                position: DVec3::new(spawn_x as f64, spawn_y, spawn_z as f64),
                yaw: std::f64::consts::FRAC_PI_4,
                pitch: -0.45,
                velocity: DVec3::ZERO,
                on_ground: false,
                mode: MoveMode::Spectator,
            },
            keys: HashSet::new(),
            cursor_captured: false,
            last_frame: Instant::now(),

            generator,
            // Cylindrical horizontally, surface-following vertically.
            streamer: Streamer::surface_following(
                LOAD_RADIUS,
                UNLOAD_RADIUS,
                LOAD_BELOW_CHUNKS,
                LOAD_ABOVE_CHUNKS,
                LOAD_AROUND_CAMERA_CHUNKS,
            ),
            lod_ring: LodRing::new(
                LOD_INNER_CHUNKS,
                &LOD_LEVELS,
                LOD_UNLOAD_MARGIN_CHUNKS,
                LOD_WORLD_Y_BLOCKS,
            ),
            settings: vox_core::Settings::default(),
            settings_applied: vox_core::Settings::default(),
            ui: None,
            lod_far_chunks: LOD_LEVELS[LOD_LEVELS.len() - 1].outer_chunks,
            registry: BlockRegistry::default_set(),
            store,
            gen_in_flight: HashSet::new(),
            dirty: HashSet::new(),
            relight: HashSet::new(),
            meshed_once: HashSet::new(),
            column_heights: ColumnHeights::new(),
            edited_columns: Arc::new(edited_columns),
            edits_unsaved: false,
            targeted: None,
            selected_block: vox_core::registry::STONE,
            gen_tx,
            gen_rx,
            lod_tx,
            lod_rx,
            lod_in_flight: HashSet::new(),
            lod_retired: HashSet::new(),
            lod_retire_age: 0,
            lod_skirts: true,
            lod_pending_set: HashSet::new(),

            telemetry_accum: 0.0,
            relight_ms_accum: 0.0,
            mesh_ms_accum: 0.0,
            telemetry_frames: 0,
            worst_frame_ms: 0.0,

            // Start mid-morning (0.30 of the day) so the world opens in clear
            // daylight and the first cycle heads toward a visible dusk.
            world_time: vox_core::WorldTime::from_ticks(
                (0.30 * vox_core::TICKS_PER_DAY as f64) as u64,
            ),
            time_accum: 0.30 * vox_core::TICKS_PER_DAY as f64,
            time_paused: false,
            time_fast: false,
        }
    }
}

impl App {
    /// True if the given world-space AABB overlaps any solid block. Scans the
    /// integer block cells the box spans (loaded chunks only; unloaded reads as
    /// air, so you won't fall through the world unless it's genuinely ungen).
    fn aabb_hits_solid(&self, min: [f64; 3], max: [f64; 3]) -> bool {
        let lo = [
            min[0].floor() as i64,
            min[1].floor() as i64,
            min[2].floor() as i64,
        ];
        let hi = [
            (max[0] - 1e-6).floor() as i64,
            (max[1] - 1e-6).floor() as i64,
            (max[2] - 1e-6).floor() as i64,
        ];
        for bx in lo[0]..=hi[0] {
            for by in lo[1]..=hi[1] {
                for bz in lo[2]..=hi[2] {
                    let wp = WorldPos::new(bx, by, bz);
                    if self.registry.is_solid(self.world.get_block(wp))
                        && cell_overlaps_aabb(wp, min, max)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Move the player's eye by `delta` along one axis with AABB collision: if
    /// the moved box hits a solid, cancel the motion on that axis. Returns
    /// whether a collision blocked it.
    fn move_axis(&self, cam: &mut FlyCamera, axis: usize, delta: f64) -> bool {
        if delta == 0.0 {
            return false;
        }
        let saved = cam.position;
        cam.position[axis] += delta;
        let (min, max) = cam.aabb();
        if self.aabb_hits_solid(min, max) {
            cam.position = saved;
            true
        } else {
            false
        }
    }

    /// Survival physics step: gravity, jump, WASD (yaw-relative), per-axis AABB
    /// collision, and Minecraft-style auto-step over single-block ledges.
    fn physics_update(&mut self, dt: f64) {
        let mut cam = FlyCamera {
            position: self.camera.position,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
            velocity: self.camera.velocity,
            on_ground: self.camera.on_ground,
            mode: self.camera.mode,
        };

        // Horizontal velocity from input (yaw-relative).
        let flat_forward = DVec3::new(cam.yaw.cos(), 0.0, cam.yaw.sin());
        let right = DVec3::new(-cam.yaw.sin(), 0.0, cam.yaw.cos());
        let mut wish = DVec3::ZERO;
        if self.keys.contains(&KeyCode::KeyW) {
            wish += flat_forward;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            wish -= flat_forward;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            wish += right;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            wish -= right;
        }
        let speed = if self.keys.contains(&KeyCode::ControlLeft) {
            FlyCamera::SPRINT_SPEED
        } else {
            FlyCamera::WALK_SPEED
        };
        let horiz = if wish != DVec3::ZERO {
            wish.normalize() * speed
        } else {
            DVec3::ZERO
        };
        cam.velocity.x = horiz.x;
        cam.velocity.z = horiz.z;

        // Jump only when grounded.
        if cam.on_ground && self.keys.contains(&KeyCode::Space) {
            cam.velocity.y = FlyCamera::JUMP_SPEED;
            cam.on_ground = false;
        }

        // Gravity (clamped to terminal velocity).
        cam.velocity.y -= FlyCamera::GRAVITY * dt;
        if cam.velocity.y < -FlyCamera::TERMINAL_FALL {
            cam.velocity.y = -FlyCamera::TERMINAL_FALL;
        }

        // Vertical first, with collision → sets grounded state.
        let dy = cam.velocity.y * dt;
        let hit_y = self.move_axis(&mut cam, 1, dy);
        if hit_y {
            cam.on_ground = dy < 0.0;
            cam.velocity.y = 0.0;
        } else {
            cam.on_ground = false;
        }

        // Then horizontal, with auto-step over single-block ledges.
        let dx = cam.velocity.x * dt;
        let dz = cam.velocity.z * dt;
        self.move_horizontal_with_step(&mut cam, dx, dz);

        self.camera.position = cam.position;
        self.camera.velocity = cam.velocity;
        self.camera.on_ground = cam.on_ground;
    }

    /// Horizontal motion on X and Z; if an axis is blocked while grounded,
    /// retry after stepping up (auto-step over 1-block ledges), then drop back
    /// down onto the ledge.
    fn move_horizontal_with_step(&self, cam: &mut FlyCamera, dx: f64, dz: f64) {
        for (axis, d) in [(0usize, dx), (2usize, dz)] {
            if d == 0.0 {
                continue;
            }
            let before_axis = cam.position[axis];
            let blocked = self.move_axis(cam, axis, d);
            if blocked && cam.on_ground {
                let lifted = self.move_axis(cam, 1, FlyCamera::STEP_HEIGHT);
                if !lifted {
                    let still_blocked = self.move_axis(cam, axis, d);
                    let _ = self.move_axis(cam, 1, -FlyCamera::STEP_HEIGHT);
                    if still_blocked {
                        cam.position[axis] = before_axis;
                    }
                }
            }
        }
    }

    /// Mark a chunk and its six face-neighbors dirty (re-mesh) and queued for
    /// relight. Border faces and border light both depend on neighbors.
    fn mark_dirty_with_neighbors(&mut self, c: ChunkPos) {
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
    fn update_heightmap_for(&mut self, pos: ChunkPos) {
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
            // chunks are exactly those with matching chunk (x,z); the check is
            // two integer compares per loaded chunk (the chunk directly below,
            // which the new surface may now shadow, is in the stack too).
            let mut to_relight: Vec<ChunkPos> = Vec::new();
            for (cpos, _) in self.world.chunks() {
                if cpos.x == pos.x && cpos.z == pos.z {
                    to_relight.push(cpos);
                }
            }
            for c in to_relight {
                self.relight.insert(c);
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
    fn recompute_height_column(&mut self, wx: i64, wz: i64) {
        let lx = wx.rem_euclid(CHUNK_SIZE_I) as u8;
        let lz = wz.rem_euclid(CHUNK_SIZE_I) as u8;
        let cx = wx.div_euclid(CHUNK_SIZE_I);
        let cz = wz.div_euclid(CHUNK_SIZE_I);

        let mut highest: Option<i64> = None;
        let mut column_chunks: Vec<ChunkPos> = Vec::new();
        for (cpos, chunk) in self.world.chunks() {
            if cpos.x != cx || cpos.z != cz {
                continue;
            }
            column_chunks.push(cpos);
            for ly in (0..CHUNK_SIZE_I as u8).rev() {
                let p = LocalPos::new(lx, ly, lz);
                if self.registry.is_solid(chunk.get(p)) {
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

    /// Save every loaded chunk that's been modified, and the LOD edit overlay —
    /// called on exit so edits to chunks still resident (not yet unloaded)
    /// aren't lost.
    fn save_all_modified(&mut self) {
        let mut saved = 0;
        for (pos, chunk) in self.world.chunks() {
            if chunk.is_modified() {
                if let Err(e) = self.store.save_chunk(pos, chunk) {
                    log::error!("exit save failed for {:?}: {e}", (pos.x, pos.y, pos.z));
                } else {
                    saved += 1;
                }
            }
        }
        if saved > 0 {
            log::info!("saved {saved} modified chunks on exit");
        }
        self.save_edit_overlay();
    }

    /// Write the LOD edit overlay if it changed since it was last written.
    ///
    /// Called wherever chunks are saved, never per edit: the whole overlay is
    /// rewritten each time, which is cheap at the rate chunks save and would
    /// not be at the rate blocks break. On failure it stays marked unsaved, so
    /// the next save retries.
    fn save_edit_overlay(&mut self) {
        if !self.edits_unsaved {
            return;
        }
        match self.store.save_edited_columns(&self.edited_columns) {
            Ok(()) => self.edits_unsaved = false,
            Err(e) => log::error!("failed to save the LOD edit overlay: {e}"),
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
    /// The inclusive chunk-Y span of terrain surface within a chunk column.
    ///
    /// Sampled at the column's four corners and its centre rather than one
    /// point: a 32-block-wide column can hold a cliff, and a single sample
    /// would report the top and miss the face.
    fn surface_span_chunks(&self, cx: i64, cz: i64) -> (i64, i64) {
        let (bx, bz) = (cx * CHUNK_SIZE_I, cz * CHUNK_SIZE_I);
        let e = CHUNK_SIZE_I - 1;
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        for (ox, oz) in [(0, 0), (e, 0), (0, e), (e, e), (e / 2, e / 2)] {
            let h = self.generator.surface_height(bx + ox, bz + oz);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        (lo.div_euclid(CHUNK_SIZE_I), hi.div_euclid(CHUNK_SIZE_I))
    }

    /// The chunk-Y range resident around a column's terrain surface,
    /// whatever the camera does. LOD coverage asks this: whether the ground
    /// under a node is drawn. Deferring to the streamer keeps one source of
    /// truth — anything judging it independently gets it wrong the moment the
    /// margins or the clamping change.
    fn surface_window_chunks(&self, cx: i64, cz: i64) -> (i64, i64) {
        self.streamer
            .surface_window(self.surface_span_chunks(cx, cz))
    }

    /// Every chunk layer resident for a column right now: the surface window
    /// plus the camera's own neighbourhood (M10 A3). A scan for "what is in
    /// this column" must cover both, or it misses what the player built
    /// above the terrain window.
    fn column_window_chunks(&self, cx: i64, cz: i64) -> vox_core::ColumnWindow {
        self.streamer.column_window(
            self.camera.block_pos().chunk(),
            self.surface_span_chunks(cx, cz),
        )
    }

    fn stream_tick(&mut self, camera_chunk: ChunkPos) {
        // 1. Ask the streamer what should change.
        //
        // The surface span is sampled at the chunk column's four corners and
        // its centre rather than one point: a 32-block-wide column can hold a
        // cliff, and a single sample would load its top and leave a hole down
        // the face. Cheap — the streamer memoizes one call per column.
        let generator = self.generator;
        let update = self.streamer.update(camera_chunk, |cx, cz| {
            let (bx, bz) = (cx * CHUNK_SIZE_I, cz * CHUNK_SIZE_I);
            let e = CHUNK_SIZE_I - 1;
            let mut lo = i64::MAX;
            let mut hi = i64::MIN;
            for (ox, oz) in [(0, 0), (e, 0), (0, e), (e, e), (e / 2, e / 2)] {
                let h = generator.surface_height(bx + ox, bz + oz);
                lo = lo.min(h);
                hi = hi.max(h);
            }
            (lo.div_euclid(CHUNK_SIZE_I), hi.div_euclid(CHUNK_SIZE_I))
        });

        // 2. Unloads: save the chunk if it was modified, then drop its data
        //    + GPU mesh; neighbors may now expose a border face, so they're
        //    marked dirty below.
        let mut saved_any = false;
        for pos in &update.to_unload {
            if let Some(chunk) = self.world.chunk(*pos) {
                if chunk.is_modified() {
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
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_chunk_mesh(*pos, &MeshData::default());
            }
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
            let generator = self.generator;
            let store = self.store.clone();
            let tx = self.gen_tx.clone();
            rayon::spawn(move || {
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
            });
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
                self.invalidate_lod_for_chunk(pos);
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
                let lit = relight_chunks_parallel(
                    &self.world,
                    &self.registry,
                    &self.column_heights,
                    &sub,
                );
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
            self.relight_ms_accum += start.elapsed().as_secs_f32() * 1000.0;
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
                let meshes = mesh_chunks_parallel(&self.world, &self.registry, &jobs);
                if let Some(renderer) = self.renderer.as_mut() {
                    let produced: HashSet<ChunkPos> = meshes.iter().map(|(p, _)| *p).collect();
                    // A batch chunk that produced no mesh (all air, or fully
                    // occluded) must have any stale GPU mesh cleared.
                    for p in &batch {
                        if !produced.contains(p) {
                            renderer.set_chunk_mesh(*p, &MeshData::default());
                        }
                    }
                    for (pos, mesh) in &meshes {
                        renderer.set_chunk_mesh(*pos, mesh);
                    }
                }
                if start.elapsed().as_secs_f32() * 1000.0 >= MESH_TIME_MS {
                    break;
                }
            }
            self.mesh_ms_accum += start.elapsed().as_secs_f32() * 1000.0;
        }
    }

    /// World-space origin chunk of a LOD node. Y comes from the node's own
    /// vertical index — columns stack to cover the world band (M09).
    fn lod_node_origin_chunk(ring: &LodRing, id: LodNodeId) -> ChunkPos {
        let (bx, by, bz) = ring.node_origin_blocks(id);
        ChunkPos::new(bx / CHUNK_SIZE_I, by / CHUNK_SIZE_I, bz / CHUNK_SIZE_I)
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
    fn ready_for_first_mesh(&self, p: ChunkPos, camera_chunk: ChunkPos) -> bool {
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
    fn neighbor_coming(&self, n: ChunkPos, camera_chunk: ChunkPos) -> bool {
        self.streamer
            .wants(n, camera_chunk, self.surface_span_chunks(n.x, n.z))
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
    fn sealed_faces(&self, p: ChunkPos, camera_chunk: ChunkPos) -> u8 {
        let mut sealed = 0u8;
        for (bit, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
            let n = ChunkPos::new(p.x + dx, p.y + dy, p.z + dz);
            if self.world.chunk(n).is_none() && !self.neighbor_coming(n, camera_chunk) {
                sealed |= 1 << bit;
            }
        }
        sealed
    }

    /// Outermost LOD radius in blocks, for sizing the camera's far plane and
    /// the automatic fog range.
    fn lod_far_blocks(&self) -> f32 {
        (self.lod_far_chunks * CHUNK_SIZE_I) as f32
    }

    /// Drop every LOD node, queued, in-flight or drawn. The next `lod_tick`
    /// re-requests the whole ring if LOD is enabled. One teardown for every
    /// caller, so none of them forgets a queue.
    fn reset_lod(&mut self) {
        self.lod_ring.clear();
        self.lod_in_flight.clear();
        self.lod_pending_set.clear();
        self.lod_retired.clear();
        self.lod_retire_age = 0;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_lod();
        }
    }

    /// How densely to sample a node of `level` (ADR-0008, M10 amendment).
    ///
    /// Exact where the level can meet full-resolution terrain — LOD underlaps
    /// it, and an overshoot there pokes through real ground. Sparse everywhere
    /// else, which is what keeps a node's cost flat at any stride.
    ///
    /// "Can meet" is the level's closest approach against the full-res
    /// region's reach (the unload radius — chunks linger until then), plus
    /// one coarsest stride: a node the ring has retired stays drawn until its
    /// replacement lands, by which time the camera may be one snap step
    /// nearer. Decided per level, never per node, so no node ever needs a
    /// rebuild because the camera moved.
    fn lod_sampling(&self, level: u32) -> LodSampling {
        let reach = self.streamer.unload_radius() + self.lod_ring.coarsest_stride();
        if self.lod_ring.closest_approach_chunks(level) <= reach {
            LodSampling::Exact
        } else {
            LodSampling::Sparse
        }
    }

    /// Recompute one column's surface height from the world.
    ///
    /// `update_heightmap_for` only ever RAISES a column (correct for
    /// streaming, where chunks arrive in any order and the max wins), so it
    /// cannot see a block being mined away. Without this the heightmap keeps
    /// the pre-edit surface, and anything derived from it — LOD nodes, skylight
    /// — rebuilds the terrain the player just removed: a "ghost block".
    fn recompute_column_height(&mut self, x: i64, z: i64) {
        // Scan only what can be resident for this column: its surface window
        // and the camera's neighbourhood, top down, skipping the gap between
        // them. Against the world band this would be a 20 000-block walk on
        // every block break; blocks outside the window are not loaded, so
        // scanning them would find nothing anyway. The camera part matters: a
        // block placed above the surface window is only visible to this scan
        // through it.
        let window =
            self.column_window_chunks(x.div_euclid(CHUNK_SIZE_I), z.div_euclid(CHUNK_SIZE_I));
        let mut top = i64::MIN;
        'scan: for cy in window.layers().rev() {
            for y in (cy * CHUNK_SIZE_I..(cy + 1) * CHUNK_SIZE_I).rev() {
                if !self.world.get_block(WorldPos::new(x, y, z)).is_air() {
                    top = y;
                    break 'scan;
                }
            }
        }
        self.column_heights.set(x, z, top);
        // Same value, but sparse, never pruned and saved with the world, so
        // every LOD level sees the edit, this session and every later one. Clamped into the LOD
        // band: a fully mined column reports the i64::MIN sentinel, which is a
        // "no terrain" marker, not a height.
        Arc::make_mut(&mut self.edited_columns).record(x, z, top.max(LOD_WORLD_Y_BLOCKS.0) as i32);
        self.edits_unsaved = true;
    }

    /// LOD nodes whose ground is fully covered by resident full-resolution
    /// chunks, and which therefore must NOT be drawn.
    ///
    /// LOD underlaps the full-res region by design (that is what guarantees no
    /// gaps). The cost is that any hole the player digs shows coarse terrain
    /// behind it — the neighbouring coarse cells' walls seen through the gap,
    /// which reads as a "ghost block" left where terrain was removed. A coarse
    /// node cannot represent a hole; the only correct answer is to stop drawing
    /// it wherever the real chunks already cover the ground.
    ///
    /// Cheap geometry pre-filter (is the whole footprint inside the streaming
    /// radius?) before the actual residency check, so only a handful of nodes
    /// pay for the lookups.
    fn covered_lod_nodes(&mut self, camera_chunk: ChunkPos) -> HashSet<(ChunkPos, u32)> {
        let mut out = HashSet::new();
        let r = self.settings.load_radius;
        // Snapshot the ids so the loop can borrow `self.world` freely.
        //
        // Retired nodes are still DRAWN but are no longer in `loaded()`, so
        // they must be considered here too — otherwise a node lingering over
        // ground the player has dug shows its coarse surface through the hole
        // for the few frames before it is flushed.
        let ids: Vec<LodNodeId> = self
            .lod_ring
            .loaded()
            .iter()
            .chain(self.lod_retired.iter())
            .copied()
            .collect();
        for id in &ids {
            let s = self.lod_ring.stride(id.level);
            let (ox, oz) = self.lod_ring.node_origin_chunk_xz(*id);
            // Pre-filter: every corner of the footprint inside the radius.
            let far_x = (ox - camera_chunk.x)
                .abs()
                .max((ox + s - 1 - camera_chunk.x).abs());
            let far_z = (oz - camera_chunk.z)
                .abs()
                .max((oz + s - 1 - camera_chunk.z).abs());
            if far_x * far_x + far_z * far_z > r * r {
                continue;
            }
            // Confirm every chunk in the footprint is actually DRAWN — resident
            // AND meshed at least once. Residency alone is not enough: a
            // chunk's first mesh is deliberately held back until its neighbors
            // arrive (so it never bakes dark), and behind a meshing queue. A
            // node suppressed in that window hides the coarse terrain while the
            // real chunk draws nothing, and the sky shows through — a brief
            // flash at the edge of the full-res region at radius 8, and a
            // band hundreds of blocks wide at radius 24, where the mesh
            // backlog runs to thousands of chunks.
            let mut missing = false;
            'cols: for cz in oz..oz + s {
                for cx in ox..ox + s {
                    // Only the layers that will ever be resident for THIS
                    // column. Walking the world band would be ~640 layers per
                    // column and would never succeed, because streaming does
                    // not load them.
                    let (band_lo, band_hi) = self.surface_window_chunks(cx, cz);
                    for cy in band_lo..=band_hi {
                        let c = ChunkPos::new(cx, cy, cz);
                        if self.world.chunk(c).is_none() || !self.meshed_once.contains(&c) {
                            missing = true;
                            break 'cols;
                        }
                    }
                }
            }
            if !missing {
                out.insert((Self::lod_node_origin_chunk(&self.lod_ring, *id), id.level));
            }
        }
        out
    }

    /// Invalidate the LOD nodes covering a whole chunk column.
    ///
    /// Needed when a chunk arrives carrying edits (loaded from disk). LOD
    /// streams immediately at startup, long before saved chunks finish
    /// loading, so those nodes are built from SEED heights and show the
    /// terrain as it was before the player ever touched it. Nothing later
    /// rebuilds them — no edit happens there in this session — so the stale
    /// surface persists in the same places every run.
    fn invalidate_lod_for_chunk(&mut self, pos: ChunkPos) {
        self.invalidate_lod_column(pos.x, pos.z);
    }

    /// Re-request every LOD node covering one chunk column, at every level —
    /// but ONLY nodes the ring already wanted.
    ///
    /// The guard is the point. Each level's annulus starts far from the camera
    /// (level 1 at 16 chunks, level 2 at 32), so the node covering an edit at
    /// the player's feet is normally one that NO level has asked for. Queueing
    /// it anyway built a coarse node from seed heights directly on top of the
    /// full-resolution terrain, where the coverage suppression can't reach it
    /// (its footprint is 4 or 8 chunks wide, so it fails the "whole footprint
    /// inside the load radius" pre-filter) and nothing unloads it until the
    /// camera crosses a snapped-centre boundary 256 blocks away. The result is
    /// coarse terrain appearing exactly where the player just mined, lasting
    /// until they walk far enough: the reported ghost blocks.
    ///
    /// A node the ring does not want needs no rebuild — when a level's annulus
    /// later reaches this column, `lod_tick` requests it and it is built fresh
    /// with the edits already folded in.
    fn invalidate_lod_column(&mut self, chunk_x: i64, chunk_z: i64) {
        for level in 0..self.lod_ring.level_count() as u32 {
            let id = self.lod_ring.node_containing(level, chunk_x, chunk_z);
            let loaded = self.lod_ring.loaded().contains(&id);
            let queued = self.lod_pending_set.contains(&id) || self.lod_in_flight.contains(&id);
            if !loaded && !queued {
                continue;
            }
            if loaded {
                self.lod_ring.mark_unloaded(id);
                let origin = Self::lod_node_origin_chunk(&self.lod_ring, id);
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.remove_lod_mesh(origin, id.level);
                }
            }
            // Re-request even if it was only pending: its heights are stale.
            self.lod_pending_set.insert(id);
            self.lod_in_flight.remove(&id);
        }
    }

    /// Invalidate the LOD nodes covering an edited block column, so they
    /// rebuild from the new terrain.
    ///
    /// Without this the coarse node keeps the pre-edit surface. LOD underlaps
    /// the full-resolution region, so the stale surface shows through the hole
    /// the player just dug — the block appears to still be there ("ghost
    /// block") even though the real chunk was re-meshed correctly.
    fn invalidate_lod_at(&mut self, pos: WorldPos) {
        // The heightmap must reflect the edit BEFORE the node regenerates from
        // it, or the rebuilt node restores what was just mined.
        self.recompute_column_height(pos.x, pos.z);
        self.invalidate_lod_column(
            pos.x.div_euclid(CHUNK_SIZE_I),
            pos.z.div_euclid(CHUNK_SIZE_I),
        );
    }

    /// Rebuild the streaming rings after an expensive settings change
    /// (M09 A3). Radius and LOD-distance edits change what the world should
    /// have loaded, so the streamer and LOD ring are recreated and their
    /// current sets dropped; the next tick re-requests everything.
    fn apply_view_settings(&mut self) {
        let s = self.settings;
        // Reconfigure, never replace: a fresh streamer would re-request every
        // resident chunk and overwrite unsaved edits with the disk copy.
        self.streamer.reconfigure(
            s.load_radius,
            s.load_radius + 2,
            LOAD_BELOW_CHUNKS,
            LOAD_ABOVE_CHUNKS,
            LOAD_AROUND_CAMERA_CHUNKS,
        );
        // Extend view distance by APPENDING coarser levels, each doubling both
        // stride and radius. That keeps per-level node count roughly constant,
        // so distance costs linearly in levels — where scaling the radii alone
        // is quadratic (x4 distance once produced 10 240 nodes and 4.3 GB).
        let mut levels: Vec<vox_core::LodLevel> = LOD_LEVELS.to_vec();
        for _ in 0..s.lod_extra_levels {
            let last = *levels.last().expect("at least one base level");
            levels.push(vox_core::LodLevel {
                stride: last.stride * 2,
                outer_chunks: last.outer_chunks * 2,
            });
        }
        // The ring requires every radius to be a multiple of the COARSEST
        // stride, which just grew. Round each outer edge up onto that grid and
        // keep the sequence strictly increasing — collapsing two levels onto
        // the same radius is what panicked ("outer 8 must exceed inner 8").
        let coarsest = levels.last().map(|l| l.stride).unwrap_or(8);
        let mut prev = LOD_INNER_CHUNKS;
        for l in levels.iter_mut() {
            let aligned = ((l.outer_chunks + coarsest - 1) / coarsest) * coarsest;
            l.outer_chunks = aligned.max(prev + coarsest);
            prev = l.outer_chunks;
        }
        self.lod_far_chunks = prev;

        // Actually install the new ring. Losing this line meant the ring kept
        // every node marked loaded with its centre already set, so update()
        // returned nothing and LOD never came back until the camera crossed a
        // node boundary — exactly the reported symptom.
        // The hysteresis margin has the SAME alignment requirement as the
        // radii: a multiple of the coarsest stride, which grows with extra
        // levels. Round it up rather than passing the base constant, or adding
        // a level trips the ring's assertion.
        let margin = ((LOD_UNLOAD_MARGIN_CHUNKS + coarsest - 1) / coarsest) * coarsest;
        self.lod_ring = LodRing::new(LOD_INNER_CHUNKS, &levels, margin, LOD_WORLD_Y_BLOCKS);

        self.lod_pending_set.clear();
        self.lod_in_flight.clear();
        // `clear_lod` drops the meshes retirees refer to; leaving ids behind
        // would make the next flush remove nodes that have since been rebuilt
        // under the same key.
        self.lod_retired.clear();
        self.lod_retire_age = 0;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_lod();
        }
        let exact = (0..levels.len() as u32)
            .filter(|&l| self.lod_sampling(l) == LodSampling::Exact)
            .count();
        log::info!(
            "settings applied: radius {}, {} LOD levels ({} exact, {} sparse), horizon {} blocks",
            s.load_radius,
            levels.len(),
            exact,
            levels.len() - exact,
            self.lod_far_blocks() as i64
        );
    }

    /// M08: stream coarse LOD nodes around the camera. Gen + mesh run on the
    /// rayon pool (both are pure CPU); the main thread only uploads. Mirrors the
    /// chunk streaming path (`stream_tick`) but for one coarse level.
    fn lod_tick(&mut self, camera_chunk: ChunkPos) {
        if !self.settings.lod_enabled {
            return;
        }
        let update = self.lod_ring.update(camera_chunk);

        // Unloads: RETIRE the node rather than deleting it. Forget it in the
        // ring and cancel any queued work, but leave its mesh on screen until
        // the replacement exists (flushed at the end of this tick).
        //
        // Deleting immediately is what produced the flash: a snapped-centre
        // jump retires ~150 nodes in one frame while their replacements take
        // many frames to generate and mesh, and the ground is simply missing in
        // between — you see fog through it, sweeping outward as the refill
        // works nearest-first.
        for n in &update.to_unload {
            self.lod_ring.mark_unloaded(*n);
            self.lod_in_flight.remove(n);
            // Cancel a queued node (leaves a stale deque entry, skipped on pop).
            self.lod_pending_set.remove(n);
            // Start the clock when the set goes from empty to non-empty, so
            // the cap measures how long THESE retirees have been held. Letting
            // it free-run meant it was already far past the cap by the time the
            // first retirement happened, and that batch was flushed one frame
            // later — the hold never actually held.
            if self.lod_retired.is_empty() {
                self.lod_retire_age = 0;
            }
            self.lod_retired.insert(*n);
        }

        // Enqueue newly-wanted nodes (the whole ring on the first update after
        // a node change; the budget below spreads generation over frames).
        for n in update.to_load {
            // Already on screen from a previous retirement: adopt it as-is.
            // Node geometry is independent of the camera, so the mesh is still
            // exactly right — rebuilding it would burn a budget slot to produce
            // identical bytes.
            if self.lod_retired.remove(&n) {
                self.lod_ring.mark_loaded(n);
                continue;
            }
            if self.lod_in_flight.contains(&n) || self.lod_pending_set.contains(&n) {
                continue;
            }
            self.lod_pending_set.insert(n);
        }

        // Drain finished meshes and upload (skipping any cancelled mid-flight).
        while let Ok((n, mesh)) = self.lod_rx.try_recv() {
            if !self.lod_in_flight.remove(&n) {
                continue;
            }
            // Span is per-level: a coarser node covers more world.
            let span = self.lod_ring.span_blocks(n.level) as f32;
            let origin = Self::lod_node_origin_chunk(&self.lod_ring, n);
            // Where this node's level hands over to the next coarser one, and
            // therefore where its geomorph must be complete (ADR-0009). The
            // coarsest level has nothing to morph toward, so it gets 0 and the
            // shader holds its morph factor at zero.
            let morph_end = self
                .lod_ring
                .morph_end_blocks(n.level)
                .map_or(0.0, |b| b as f32);
            if mesh.is_empty() {
                log::warn!("lod_tick: node {:?} produced an EMPTY mesh", n);
            }
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_lod_mesh(origin, n.level, span, morph_end, &mesh);
            }
            self.lod_ring.mark_loaded(n);
        }

        // Spawn up to the budget from the pending queue. Coarse gen + mesh run
        // on rayon (pure CPU); only the upload above is on the main thread.
        let grass = vox_core::registry::GRASS;
        let stone = vox_core::registry::STONE;
        let grass_layers: [u32; 6] = std::array::from_fn(|f| self.registry.face_layer(grass, f));
        let stone_layers: [u32; 6] = std::array::from_fn(|f| self.registry.face_layer(stone, f));

        let mut spawned = 0;
        if !self.lod_pending_set.is_empty() {
            // Nearest-first (M09 task 1): the ring is enqueued in scan order,
            // so a naive drain fills one edge before the nodes in front of the
            // player. Sorted by the node's distance in CHUNKS (comparable
            // across levels, which have different node sizes), then by level so
            // the finer, nearer detail lands first.
            let mut wanted: Vec<LodNodeId> = self.lod_pending_set.iter().copied().collect();
            let ring = &self.lod_ring;
            wanted.sort_unstable_by_key(|n| {
                let (ox, oz) = ring.node_origin_chunk_xz(*n);
                let s = ring.stride(n.level);
                // Centre of the node, in chunks.
                let dx = ox + s / 2 - camera_chunk.x;
                let dz = oz + s / 2 - camera_chunk.z;
                (dx * dx + dz * dz, n.level, n.x, n.z)
            });
            for n in wanted.into_iter().take(LOD_SPAWN_BUDGET) {
                self.lod_pending_set.remove(&n);
                if self.lod_in_flight.contains(&n) {
                    continue;
                }
                self.lod_in_flight.insert(n);
                let stride = self.lod_ring.stride(n.level);
                let (bx, by, bz) = self.lod_ring.node_origin_blocks(n);
                let origin = WorldPos::new(bx, by, bz);

                // Build the node as a HEIGHTFIELD (M09). The voxel path
                // quantized height to the cell size, which is what made distant
                // terrain read as stacked terraces of tall slabs; a heightfield
                // keeps vertical detail exact and only quantizes horizontally.
                //
                // Every level, level 0 included, is built from the seed with
                // the player's edits folded in. Level 0 used to gather real
                // heights from the heightmap on the main thread (~4 000 lookups
                // a node); at stride 2 the seed sampler already reads every
                // column, and the edit overlay is saved with the world, so the
                // gather added nothing but the cost (M10 A3).
                //
                // Copied into the rayon job (Generator is Copy, so this is free).
                let sampling = self.lod_sampling(n.level);
                let generator = self.generator;
                let tx = self.lod_tx.clone();
                let origin_y = origin.y as i32;
                let skirt = if self.lod_skirts {
                    LOD_SKIRT_DEPTH_CELLS * stride as i32
                } else {
                    0 // no border walls at all (see `lod_skirts`)
                };
                let edits = Arc::clone(&self.edited_columns);
                rayon::spawn(move || {
                    let mut heights =
                        generator.lod_heightfield(origin.x, origin.z, stride, sampling);
                    // Fold in player edits. The seed always reads higher than
                    // dug ground, so without this the coarse surface floats
                    // above the hole and shows through it (the "ghost block").
                    edits.apply_to_node(&mut heights, origin.x, origin.z, stride, origin_y);
                    let mesh = vox_mesh::mesh_lod_heightfield(
                        &heights,
                        stride as i32,
                        origin_y,
                        |b, face| {
                            if b == grass {
                                grass_layers[face]
                            } else {
                                stone_layers[face]
                            }
                        },
                        skirt,
                    );
                    let _ = tx.send((n, mesh));
                });
                spawned += 1;
            }
        }
        let _ = spawned;

        // Flush retirees once the work that replaces them has landed — or after
        // the cap, so a long flight can't grow this without bound. Checked
        // AFTER the drain and spawn above so meshes that arrived this tick
        // count toward "drained".
        if !self.lod_retired.is_empty() {
            self.lod_retire_age += 1;
            let replacements_landed =
                self.lod_pending_set.is_empty() && self.lod_in_flight.is_empty();
            if replacements_landed || self.lod_retire_age >= LOD_RETIRE_MAX_TICKS {
                for n in std::mem::take(&mut self.lod_retired) {
                    let origin = Self::lod_node_origin_chunk(&self.lod_ring, n);
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.remove_lod_mesh(origin, n.level);
                    }
                }
            }
        }
    }

    /// the edit persists). No-op when nothing is targeted.
    fn break_block(&mut self) {
        let Some(hit) = self.targeted else { return };
        let pos = hit.block_pos;
        if self.world.get_block(pos).is_air() {
            return;
        }
        self.world.set_block(pos, BlockId::AIR);
        self.recompute_height_column(pos.x, pos.z);
        self.mark_dirty_with_neighbors(pos.chunk());
        self.invalidate_lod_at(pos);
        log::info!("broke block at {:?}", (pos.x, pos.y, pos.z));
    }

    /// Place the selected block against the targeted face (right-click).
    /// Rejects placement into a non-empty cell or one overlapping the
    /// player (so you can't entomb the camera). Re-meshes + persists.
    fn place_block(&mut self) {
        let Some(hit) = self.targeted else { return };
        let Some(pos) = hit.place_pos else { return };
        if !self.world.get_block(pos).is_air() {
            return; // target cell occupied
        }
        // Reject if the cell would overlap the player's box.
        let (min, max) = self.player_aabb();
        if cell_overlaps_aabb(pos, min, max) {
            return;
        }
        self.world.set_block(pos, self.selected_block);
        self.recompute_height_column(pos.x, pos.z);
        self.mark_dirty_with_neighbors(pos.chunk());
        self.invalidate_lod_at(pos);
        log::info!("placed block at {:?}", (pos.x, pos.y, pos.z));
    }

    /// A small AABB approximating the player, centered on the camera. For
    /// now the camera is a free-flying point, so this is just a modest box
    /// around it — enough to stop placing a block that engulfs the view. A
    /// real player body (with physics) will replace this later.
    fn player_aabb(&self) -> ([f64; 3], [f64; 3]) {
        const HALF: f64 = 0.3;
        let p = self.camera.position;
        (
            [p.x - HALF, p.y - HALF, p.z - HALF],
            [p.x + HALF, p.y + HALF, p.z + HALF],
        )
    }

    /// Set the selected block to the Nth placeable block (1-based), from
    /// number keys. Out-of-range indices are ignored.
    fn select_block_slot(&mut self, slot: usize) {
        let placeable: Vec<BlockId> = self.registry.placeable().collect();
        if slot >= 1 && slot <= placeable.len() {
            self.selected_block = placeable[slot - 1];
            self.log_selection();
        }
    }

    /// Cycle the selected block forward (+1) or backward (-1), from scroll.
    fn cycle_selection(&mut self, dir: i32) {
        let placeable: Vec<BlockId> = self.registry.placeable().collect();
        if placeable.is_empty() {
            return;
        }
        let cur = placeable
            .iter()
            .position(|&b| b == self.selected_block)
            .unwrap_or(0) as i32;
        let n = placeable.len() as i32;
        let next = (cur + dir).rem_euclid(n) as usize;
        self.selected_block = placeable[next];
        self.log_selection();
    }

    fn log_selection(&self) {
        log::info!(
            "selected block: {}",
            self.registry.get(self.selected_block).name
        );
    }

    /// DEBUG: clear a sphere of blocks at the camera, marking touched chunks
    /// (and their neighbors) dirty so the streaming tick re-meshes them.
    /// Block-editing UI proper is Milestone 03; this exercises re-meshing.
    fn debug_punch_hole(&mut self) {
        const RADIUS: i64 = 6;
        let center = self.camera.block_pos();

        let mut touched: HashSet<ChunkPos> = HashSet::new();
        for dy in -RADIUS..=RADIUS {
            for dz in -RADIUS..=RADIUS {
                for dx in -RADIUS..=RADIUS {
                    if dx * dx + dy * dy + dz * dz > RADIUS * RADIUS {
                        continue;
                    }
                    let pos = WorldPos::new(center.x + dx, center.y + dy, center.z + dz);
                    if !self.world.get_block(pos).is_air() {
                        self.world.set_block(pos, BlockId::AIR);
                        touched.insert(pos.chunk());
                    }
                }
            }
        }
        for c in touched {
            self.mark_dirty_with_neighbors(c);
        }
    }

    fn set_cursor_captured(&mut self, captured: bool) {
        let Some(window) = &self.window else { return };
        if captured {
            // Locked is ideal (cursor frozen in place) but not supported
            // everywhere; Confined (trapped in window) is the fallback.
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = grabbed {
                log::warn!("cursor grab failed: {e}");
                return;
            }
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.cursor_captured = captured;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Voxterra"))
                .expect("failed to create window"),
        );

        let renderer = Renderer::new(window.clone());

        // The world is no longer pre-built: chunks stream in around the
        // camera each frame via stream_tick (M02). resumed() just stands up
        // the window/renderer; the first frames fill the initial sphere
        // progressively, bounded by GEN_SPAWN_BUDGET and the per-frame mesh time budget.
        log::info!(
            "streaming: load radius {} / unload {} chunks",
            LOAD_RADIUS,
            UNLOAD_RADIUS
        );

        self.ui = Some(SettingsUi::new(&window));
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.last_frame = Instant::now();
        self.set_cursor_captured(true);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // The settings menu sees events first. If egui consumed one (a click
        // on a slider, a keystroke in a text field), the game must ignore it —
        // otherwise dragging a slider would also swing the camera or break a
        // block behind the menu. Resize and close still fall through below.
        if !matches!(event, WindowEvent::CloseRequested | WindowEvent::Resized(_)) {
            let consumed = match (self.ui.as_mut(), self.window.as_ref()) {
                (Some(ui), Some(window)) => ui.on_window_event(window, &event),
                _ => false,
            };
            if consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_all_modified();
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if code == KeyCode::Escape {
                                // ESC toggles the settings menu (M09 A3). The
                                // cursor is released while it is open so the
                                // sliders can be used, and re-captured on
                                // close so mouse-look resumes.
                                let was_open = self.ui.as_ref().is_some_and(|u| u.open);
                                let now_closed = match self.ui.as_mut() {
                                    Some(ui) if was_open => ui.on_escape(),
                                    Some(ui) => {
                                        ui.open = true;
                                        false
                                    }
                                    None => true,
                                };
                                // Cursor is free while the menu is up.
                                self.set_cursor_captured(now_closed);
                            } else if code == KeyCode::KeyF {
                                // Toggle spectator (noclip free-fly) <-> survival.
                                self.camera.mode = match self.camera.mode {
                                    MoveMode::Spectator => MoveMode::Survival,
                                    MoveMode::Survival => MoveMode::Spectator,
                                };
                                self.camera.velocity = DVec3::ZERO;
                                self.camera.on_ground = false;
                                log::info!(
                                    "move mode: {}",
                                    match self.camera.mode {
                                        MoveMode::Spectator => "spectator (fly)",
                                        MoveMode::Survival => "survival",
                                    }
                                );
                            } else if code == KeyCode::KeyG {
                                // Debug: punch a hole to exercise re-meshing.
                                self.debug_punch_hole();
                            } else if code == KeyCode::KeyT {
                                // Debug: freeze/unfreeze the day/night cycle.
                                self.time_paused = !self.time_paused;
                                log::info!(
                                    "time {} @ {:.2}h",
                                    if self.time_paused {
                                        "paused"
                                    } else {
                                        "running"
                                    },
                                    self.world_time.time_of_day() * 24.0
                                );
                            } else if code == KeyCode::Backslash {
                                // Debug: 60x fast-forward (~24s per full day).
                                self.time_fast = !self.time_fast;
                                log::info!(
                                    "time fast-forward {}",
                                    if self.time_fast { "ON (60x)" } else { "off" }
                                );
                            } else if code == KeyCode::BracketLeft || code == KeyCode::BracketRight
                            {
                                // Debug: jump time by ±1 game-hour.
                                let hour = vox_core::TICKS_PER_DAY as f64 / 24.0;
                                let delta = if code == KeyCode::BracketRight {
                                    hour
                                } else {
                                    -hour
                                };
                                self.time_accum = (self.time_accum + delta).max(0.0);
                                self.world_time =
                                    vox_core::WorldTime::from_ticks(self.time_accum as u64);
                                log::info!(
                                    "time -> {:.2}h (sky_scale {:.3})",
                                    self.world_time.time_of_day() * 24.0,
                                    self.world_time.sky_scale()
                                );
                            } else if code == KeyCode::KeyM {
                                // Debug: advance one game DAY to step the moon
                                // phase (~1/8 of a lunation) for testing new vs
                                // full moon nights without waiting.
                                self.time_accum += vox_core::TICKS_PER_DAY as f64;
                                self.world_time =
                                    vox_core::WorldTime::from_ticks(self.time_accum as u64);
                                log::info!(
                                    "moon phase -> {:.2} (illum {:.2})",
                                    self.world_time.moon_phase(),
                                    self.world_time.moon_illumination()
                                );
                            } else if code == KeyCode::KeyL {
                                // Debug: toggle coarse LOD terrain (M08).
                                // Single source of truth: the L key edits the
                                // SAME flag the settings menu does. Two flags
                                // ANDed together meant toggling off with one
                                // and on with the other left LOD permanently
                                // dead until the camera crossed a node border.
                                self.settings.lod_enabled = !self.settings.lod_enabled;
                                if !self.settings.lod_enabled {
                                    self.reset_lod();
                                }
                                log::info!(
                                    "LOD {}",
                                    if self.settings.lod_enabled {
                                        "ON"
                                    } else {
                                        "off"
                                    }
                                );
                            } else if code == KeyCode::KeyK {
                                // Debug: rebuild LOD with or without skirts, to
                                // diagnose the grid lines (see `lod_skirts`).
                                self.lod_skirts = !self.lod_skirts;
                                self.reset_lod();
                                log::info!(
                                    "LOD skirts {} (rebuilding LOD)",
                                    if self.lod_skirts { "ON" } else { "off" }
                                );
                            } else if let Some(slot) = digit_slot(code) {
                                self.select_block_slot(slot);
                            } else {
                                self.keys.insert(code);
                            }
                        }
                        ElementState::Released => {
                            self.keys.remove(&code);
                        }
                    }
                }
            }

            // Mouse buttons: left = break (or recapture cursor after Esc),
            // right = place. Only act on edits while the cursor is captured.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => {
                    if self.cursor_captured {
                        self.break_block();
                    } else {
                        self.set_cursor_captured(true);
                    }
                }
                MouseButton::Right if self.cursor_captured => {
                    self.place_block();
                }
                _ => {}
            },

            // Scroll wheel cycles the selected block.
            WindowEvent::MouseWheel { delta, .. } => {
                if self.cursor_captured {
                    let dir = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => -y.signum() as i32,
                        winit::event::MouseScrollDelta::PixelDelta(p) => -(p.y.signum() as i32),
                    };
                    if dir != 0 {
                        self.cycle_selection(dir);
                    }
                }
            }

            // Window lost focus (alt-tab): release cursor, drop held keys.
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.set_cursor_captured(false);
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let raw_dt = (now - self.last_frame).as_secs_f32();
                // Physics/streaming use a clamped dt so a long stall can't
                // teleport the player; telemetry uses the RAW value, or a real
                // 300 ms hitch would be reported as the 100 ms clamp.
                let dt = raw_dt.min(0.1);
                self.last_frame = now;
                self.worst_frame_ms = self.worst_frame_ms.max(raw_dt * 1000.0);

                // Advance world time (M07 task 3). Accumulate in fractional
                // game-ticks so slow frames don't drift, then snapshot to the
                // integer WorldTime the shader/sky read.
                if !self.time_paused && !self.settings.time_paused {
                    let mut rate =
                        vox_core::game_ticks_per_second(self.settings.day_length_secs as f64);
                    if self.time_fast {
                        rate *= 60.0;
                    }
                    self.time_accum += dt as f64 * rate;
                    self.world_time = vox_core::WorldTime::from_ticks(self.time_accum as u64);
                }

                // Motion integrates in f64: an f32 frame time is exact enough,
                // an f32 POSITION is what loses the step far from the origin.
                match self.camera.mode {
                    MoveMode::Spectator => self.camera.update_spectator(&self.keys, f64::from(dt)),
                    MoveMode::Survival => self.physics_update(f64::from(dt)),
                }

                // Camera's current chunk drives both streaming and the
                // floating-origin render origin.
                let origin_chunk = self.camera.block_pos().chunk();

                // Stream chunks in/out around the camera (borrows all of
                // self), before the render borrow below.
                self.stream_tick(origin_chunk);
                // Stream coarse LOD nodes for the distant horizon (M08).
                self.lod_tick(origin_chunk);

                // Raycast from the camera to find the targeted block (M03
                // task 3). Uses loaded world data; unloaded cells read as air
                // (get_block returns AIR for missing chunks), so you can only
                // target visible blocks. Computed before the renderer borrow.
                let hit = {
                    let world = &self.world;
                    let registry = &self.registry;
                    let eye = self.camera.position.to_array();
                    let dir = self.camera.forward().to_array();
                    vox_core::raycast_voxels(eye, dir, REACH, |p| {
                        registry.is_solid(world.get_block(p))
                    })
                };
                self.targeted = hit;

                let loaded = self.streamer.loaded_count();
                let in_flight = self.gen_in_flight.len();
                let dirty = self.dirty.len();
                let relight = self.relight.len();

                // --- Settings menu (M09 amendment A3) ---
                // Run the UI before borrowing the renderer, then hand its
                // tessellated output to render(). Costs nothing while closed.
                // Mirror the running clock into the slider so it reads the
                // current time when the menu opens (and tracks while open).
                self.settings.time_of_day = self.world_time.time_of_day();
                self.settings_applied.time_of_day = self.settings.time_of_day;
                if let Some(ui) = self.ui.as_mut() {
                    if ui.open {
                        ui.status = format!(
                            "loaded {loaded}  dirty {dirty}  relight {relight}  lod {}",
                            self.lod_ring.loaded().len()
                        );
                    }
                }
                let ui_output = match (self.ui.as_mut(), self.window.as_ref()) {
                    (Some(ui), Some(window)) => ui.run(window, &mut self.settings),
                    _ => None,
                };
                // "Save and Quit" from the pause screen.
                if self.ui.as_ref().is_some_and(|u| u.quit_requested) {
                    self.save_all_modified();
                    event_loop.exit();
                    return;
                }
                // The menu can close itself (Back to Game); recapture the
                // cursor so mouse-look resumes without needing ESC.
                let menu_open = self.ui.as_ref().is_some_and(|u| u.open);
                if !menu_open && !self.cursor_captured {
                    self.set_cursor_captured(true);
                }
                // Expensive edits (radius, LOD distance) re-stream the world,
                // so they are applied deliberately here rather than on every
                // slider pixel.
                if self.settings.rebuild_needed(&self.settings_applied) {
                    self.apply_view_settings();
                }
                // Scrubbing time of day: only act when the SLIDER moved, not
                // every frame, or writing the clock back would fight the clock
                // advancing and time would freeze.
                if (self.settings.time_of_day - self.settings_applied.time_of_day).abs() > 1e-6 {
                    let day = vox_core::TICKS_PER_DAY as f64;
                    let day_index = (self.time_accum / day).floor();
                    self.time_accum = (day_index + self.settings.time_of_day as f64) * day;
                    self.world_time = vox_core::WorldTime::from_ticks(self.time_accum as u64);
                }
                self.settings_applied = self.settings;

                // Far plane must reach past the outermost LOD ring (plus
                // headroom for looking across it from altitude). Computed
                // before borrowing the renderer: it reads &self.
                let far = (self.lod_far_blocks() * 1.6).max(1000.0);
                // Nodes whose ground full-res already covers must not draw, or
                // every dug hole shows coarse terrain behind it.
                let covered = self.covered_lod_nodes(origin_chunk);

                if let Some(renderer) = self.renderer.as_mut() {
                    // Floating origin (ADR-0002): keep the render origin at
                    // the camera's current chunk so vertex math stays precise
                    // arbitrarily far from world zero. set_render_origin is a
                    // no-op when unchanged, so this is free while standing
                    // still and cheap (one uniform rewrite per chunk) when
                    // crossing a boundary.
                    renderer.set_render_origin(origin_chunk);

                    // Highlight the targeted block (offset uses the current
                    // render origin, set just above).
                    renderer.set_highlight(self.targeted.map(|h| h.block_pos));

                    let render_origin = origin_chunk.origin();
                    let view_proj = self.camera.view_proj(
                        renderer.aspect(),
                        render_origin,
                        self.settings.fov_degrees,
                        far,
                    );

                    // Day/night (M07 task 3): push sky_scale + sun/moon to the
                    // shaders. The sky pass needs the inverse view-projection to
                    // turn each pixel back into a world ray; direction is
                    // origin-independent, so floating origin doesn't matter here.
                    // Fog colour tracks the sky it fades into: the daytime
                    // horizon tint, dimmed by sky_scale so distant terrain goes
                    // dark with the night rather than glowing grey.
                    let sky_scale = self.world_time.sky_scale();
                    let (fog_start, fog_end) = self.settings.fog_range(far / 1.6);
                    let fog = vox_render::FogParams {
                        color: [
                            (0.68 * sky_scale).max(0.012),
                            (0.82 * sky_scale).max(0.018),
                            (0.95 * sky_scale).max(0.038),
                        ],
                        strength: self.settings.effective_fog_strength(),
                        // Scaled to the current LOD horizon in auto mode, so
                        // changing LOD levels doesn't require re-tuning fog.
                        start: fog_start,
                        end: fog_end,
                    };
                    let cam_rel = self.camera.render_relative(render_origin);
                    let morph = vox_render::MorphParams {
                        band: self.settings.lod_morph_band,
                    };
                    renderer.set_sky(
                        self.world_time,
                        view_proj.inverse().to_cols_array_2d(),
                        [cam_rel.x, cam_rel.y, cam_rel.z],
                        fog,
                        self.settings.star_intensity,
                        morph,
                    );

                    let ui_frame = ui_output.as_ref().map(|o| vox_render::UiFrame {
                        primitives: &o.primitives,
                        textures_delta_set: &o.textures_set,
                        textures_delta_free: o.textures_free.clone(),
                        pixels_per_point: o.pixels_per_point,
                    });
                    renderer.set_suppressed_lod(covered);
                    renderer.render(view_proj.to_cols_array_2d(), ui_frame);

                    // Telemetry once per second: FPS, frustum-culling ratio,
                    // and streaming state (resident chunks, gen queue, dirty).
                    self.telemetry_accum += dt;
                    self.telemetry_frames += 1;
                    if self.telemetry_accum >= 1.0 {
                        let fps = self.telemetry_frames as f32 / self.telemetry_accum;
                        let lod_backlog = self.lod_pending_set.len() + self.lod_in_flight.len();
                        log::info!(
                            "{:.0} fps (worst {:.1}ms) | drawn {}/{} | {:.2}M tris | {:.0}MB gpu | lod {}+{} | loaded {} | hmap {}k | gen {} | dirty {} | relight {} | lt {:.0}ms msh {:.0}ms",
                            fps,
                            self.worst_frame_ms,
                            renderer.drawn_last_frame(),
                            renderer.mesh_count(),
                            renderer.tris_last_frame() as f32 / 1.0e6,
                            renderer.buffer_bytes() as f32 / 1.048576e6,
                            renderer.lod_count(),
                            lod_backlog,
                            loaded,
                            // Heightmap columns, thousands. Bounded by the
                            // resident chunk columns (1 024 each); it used to
                            // grow with every column ever visited.
                            self.column_heights.len() / 1000,
                            in_flight,
                            dirty,
                            relight,
                            self.relight_ms_accum,
                            self.mesh_ms_accum,
                        );
                        self.telemetry_accum = 0.0;
                        self.telemetry_frames = 0;
                        self.relight_ms_accum = 0.0;
                        self.mesh_ms_accum = 0.0;
                        self.worst_frame_ms = 0.0;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        // Raw mouse motion: unaffected by cursor position/acceleration —
        // the right input for camera look.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.cursor_captured {
                self.camera.mouse_look(dx, dy);
            }
        }
    }
}

fn main() {
    // Default to vox_app=info so telemetry always prints; RUST_LOG still
    // overrides (e.g. RUST_LOG=vox_app=debug or =error to quiet it).
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("vox_app=info"))
        .init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
