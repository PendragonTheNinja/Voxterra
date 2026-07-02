//! vox-app: the game binary. Window, event loop, fly camera, and the
//! Milestone 00 test scene: one sine-wave chunk.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use vox_core::{
    cell_overlaps_aabb, BlockId, BlockRegistry, Chunk, ChunkPos, LocalPos, RayHit, Streamer, World,
    WorldPos, WorldStore,
};
use vox_mesh::{mesh_chunk, ChunkNeighbors, MeshData};
use vox_render::Renderer;
use vox_worldgen::Generator;
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
fn top_sky_from_heightmap(heights: &HashMap<(i64, i64), i64>, pos: ChunkPos) -> Vec<u8> {
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
            let known_h = heights.get(&(origin.x + lx, origin.z + lz)).copied();

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
    heights: &HashMap<(i64, i64), i64>,
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

fn mesh_chunks_parallel(
    world: &World,
    registry: &BlockRegistry,
    positions: &[ChunkPos],
) -> Vec<(ChunkPos, MeshData)> {
    positions
        .par_iter()
        .filter_map(|&pos| {
            let chunk = world.chunk(pos)?;
            let neighbors = ChunkNeighbors::of(world, pos);
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

struct FlyCamera {
    /// Eye position in world blocks. In survival the player AABB hangs below
    /// this by `EYE_HEIGHT`.
    position: Vec3,
    /// Radians. 0 looks along +X; positive turns toward +Z.
    yaw: f32,
    /// Radians, clamped to ±~89° so the view never flips.
    pitch: f32,
    /// World-space velocity (m/s). Used by survival physics; spectator ignores
    /// it (moves position directly).
    velocity: Vec3,
    /// Whether the player is standing on solid ground (survival).
    on_ground: bool,
    mode: MoveMode,
}

impl FlyCamera {
    const SPEED: f32 = 30.0; // m/s (spectator)
    const SPRINT_MULTIPLIER: f32 = 4.0; // hold Ctrl (spectator)
    const SENSITIVITY: f32 = 0.0022; // radians per mouse count
    const PITCH_LIMIT: f32 = 1.55; // just under PI/2

    // --- Survival tuning (Minecraft-like) ---
    /// Player collision box: 0.6 × 1.8 × 0.6 blocks.
    const HALF_WIDTH: f32 = 0.3;
    const HEIGHT: f32 = 1.8;
    /// Eye sits near the top of the box (MC eye height ~1.62).
    const EYE_HEIGHT: f32 = 1.62;
    const WALK_SPEED: f32 = 4.317; // m/s, MC walking
    const SPRINT_SPEED: f32 = 5.612; // m/s, MC sprinting
    const GRAVITY: f32 = 28.0; // m/s² (tuned for snappy MC-ish fall)
    const JUMP_SPEED: f32 = 9.0; // m/s initial (clears ~1.25 blocks)
    const STEP_HEIGHT: f32 = 0.6; // auto-step over single blocks
    const TERMINAL_FALL: f32 = 78.0; // m/s clamp

    fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
    }

    fn mouse_look(&mut self, dx: f64, dy: f64) {
        self.yaw += dx as f32 * Self::SENSITIVITY;
        self.pitch = (self.pitch - dy as f32 * Self::SENSITIVITY)
            .clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }

    /// Spectator free-fly: move the eye directly, no collision.
    fn update_spectator(&mut self, keys: &HashSet<KeyCode>, dt: f32) {
        // Horizontal movement follows yaw only (classic fly-cam feel);
        // Space/Shift move straight up/down in world space.
        let flat_forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let right = Vec3::new(-self.yaw.sin(), 0.0, self.yaw.cos());

        let mut dir = Vec3::ZERO;
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
            dir += Vec3::Y;
        }
        if keys.contains(&KeyCode::ShiftLeft) {
            dir -= Vec3::Y;
        }

        if dir != Vec3::ZERO {
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
            (self.position.x - Self::HALF_WIDTH) as f64,
            feet_y as f64,
            (self.position.z - Self::HALF_WIDTH) as f64,
        ];
        let max = [
            (self.position.x + Self::HALF_WIDTH) as f64,
            (feet_y + Self::HEIGHT) as f64,
            (self.position.z + Self::HALF_WIDTH) as f64,
        ];
        (min, max)
    }

    /// View-projection matrix built with the camera positioned **relative to
    /// the render origin** (ADR-0002). `render_origin_blocks` is the world
    /// position of the render origin; subtracting it keeps the numbers fed
    /// to the matrix small regardless of absolute distance.
    fn view_proj(&self, aspect: f32, render_origin_blocks: Vec3) -> Mat4 {
        let proj = Mat4::perspective_rh(70f32.to_radians(), aspect, 0.1, 1000.0);
        let rel_pos = self.position - render_origin_blocks;
        let view = Mat4::look_to_rh(rel_pos, self.forward(), Vec3::Y);
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
    /// Per-column heightmap: world `(x, z)` → highest solid block world-Y
    /// among loaded chunks. Drives the skylight top boundary directly, so each
    /// chunk computes its daylight in one pass without waiting on its vertical
    /// neighbors to be relit (avoids the skylight cascade; ADR-0005). Absent
    /// key = no solid known in that column = fully open to sky.
    column_heights: HashMap<(i64, i64), i64>,
    /// Block currently under the crosshair (raycast result), or none.
    targeted: Option<RayHit>,
    /// Block type placed on right-click (M03 task 4). Cycled with number
    /// keys / scroll among the registry's placeable blocks.
    selected_block: BlockId,

    /// Async generation results arrive here from the rayon pool.
    gen_tx: Sender<(ChunkPos, Chunk)>,
    gen_rx: Receiver<(ChunkPos, Chunk)>,

    /// Telemetry: accumulate frames over ~1s to log FPS + drawn/total.
    telemetry_accum: f32,
    /// Milliseconds spent in the relight / mesh streaming passes since the
    /// last telemetry line — shows where frame time goes during bursts.
    relight_ms_accum: f32,
    mesh_ms_accum: f32,
    telemetry_frames: u32,
}

impl Default for App {
    fn default() -> Self {
        let (gen_tx, gen_rx) = std::sync::mpsc::channel();

        // Open (or create) the world directory. The store's seed is
        // authoritative: a new world uses this default seed, an existing one
        // reuses its saved seed so terrain regenerates identically.
        const DEFAULT_SEED: u64 = 0x0007_E22A_C0DE;
        let store =
            WorldStore::open("world", DEFAULT_SEED).expect("failed to open world directory");
        let seed = store.seed();
        log::info!("world at {:?}, seed {:#x}", store.root(), seed);

        Self {
            window: None,
            renderer: None,
            world: World::new(),
            // Up high, looking down into the terrain that streams in below.
            camera: FlyCamera {
                position: Vec3::new(0.0, 60.0, 0.0),
                yaw: std::f32::consts::FRAC_PI_4,
                pitch: -0.45,
                velocity: Vec3::ZERO,
                on_ground: false,
                mode: MoveMode::Spectator,
            },
            keys: HashSet::new(),
            cursor_captured: false,
            last_frame: Instant::now(),

            generator: Generator::new(seed),
            streamer: Streamer::new(LOAD_RADIUS, UNLOAD_RADIUS),
            registry: BlockRegistry::default_set(),
            store,
            gen_in_flight: HashSet::new(),
            dirty: HashSet::new(),
            relight: HashSet::new(),
            column_heights: HashMap::new(),
            targeted: None,
            selected_block: vox_core::registry::STONE,
            gen_tx,
            gen_rx,

            telemetry_accum: 0.0,
            relight_ms_accum: 0.0,
            mesh_ms_accum: 0.0,
            telemetry_frames: 0,
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
    fn move_axis(&self, cam: &mut FlyCamera, axis: usize, delta: f32) -> bool {
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
    fn physics_update(&mut self, dt: f32) {
        let mut cam = FlyCamera {
            position: self.camera.position,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
            velocity: self.camera.velocity,
            on_ground: self.camera.on_ground,
            mode: self.camera.mode,
        };

        // Horizontal velocity from input (yaw-relative).
        let flat_forward = Vec3::new(cam.yaw.cos(), 0.0, cam.yaw.sin());
        let right = Vec3::new(-cam.yaw.sin(), 0.0, cam.yaw.cos());
        let mut wish = Vec3::ZERO;
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
        let horiz = if wish != Vec3::ZERO {
            wish.normalize() * speed
        } else {
            Vec3::ZERO
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
    fn move_horizontal_with_step(&self, cam: &mut FlyCamera, dx: f32, dz: f32) {
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
                let key = (origin.x + lx, origin.z + lz);
                let e = self.column_heights.entry(key).or_insert(i64::MIN);
                if world_y > *e {
                    *e = world_y;
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
        let key = (wx, wz);
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
            let e = self.column_heights.entry(key).or_insert(i64::MIN);
            if h > *e {
                *e = h;
            }
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

    /// Save every loaded chunk that's been modified — called on exit so
    /// edits to chunks still resident (not yet unloaded) aren't lost.
    fn save_all_modified(&self) {
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
    fn stream_tick(&mut self, camera_chunk: ChunkPos) {
        // 1. Ask the streamer what should change.
        let update = self.streamer.update(camera_chunk);

        // 2. Unloads: save the chunk if it was modified, then drop its data
        //    + GPU mesh; neighbors may now expose a border face, so they're
        //    marked dirty below.
        for pos in &update.to_unload {
            if let Some(chunk) = self.world.chunk(*pos) {
                if chunk.is_modified() {
                    if let Err(e) = self.store.save_chunk(*pos, chunk) {
                        log::error!("failed to save chunk {:?}: {e}", (pos.x, pos.y, pos.z));
                    }
                }
            }
            self.world.remove_chunk(*pos);
            self.streamer.mark_unloaded(*pos);
            self.dirty.remove(pos);
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_chunk_mesh(*pos, &MeshData::default());
            }
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
            self.world.insert_chunk(pos, chunk);
            self.streamer.mark_loaded(pos);
            newly_generated.push(pos);
        }
        for pos in newly_generated {
            // Fold the new chunk into the column heightmap BEFORE marking it
            // dirty, so its first relight uses a correct sky top boundary.
            self.update_heightmap_for(pos);
            self.mark_dirty_with_neighbors(pos);
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
        }

        // --- Relight pass (time-budgeted, parallel). ---
        // Recompute light in parallel sub-batches until a per-frame time cap.
        // top_sky is built per-chunk inside the workers (parallel), not here.
        // Apply only where light actually changed (→ re-mesh via `dirty`); a
        // changed border just queues a neighbor relight check (no re-mesh).
        if !self.relight.is_empty() {
            let start = Instant::now();
            loop {
                let sub: Vec<ChunkPos> = self
                    .relight
                    .iter()
                    .copied()
                    .take(RELIGHT_SUBBATCH)
                    .collect();
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
                            let n = ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                            if self.world.chunk(n).is_some() {
                                self.relight.insert(n);
                                self.dirty.insert(n);
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
            loop {
                // Defer any dirty chunk that is still queued for relight:
                // meshing it now would bake stale/zero light (the dark
                // chunk-checkerboard during streaming) and the relight would
                // immediately re-dirty it — every streamed chunk meshed twice.
                // Lighting first means each chunk is meshed once, already lit.
                // Deferred chunks stay in `dirty` and are picked up on a later
                // frame once their relight has drained.
                let batch: Vec<ChunkPos> = self
                    .dirty
                    .iter()
                    .copied()
                    .filter(|p| !self.relight.contains(p))
                    .take(MESH_SUBBATCH)
                    .collect();
                if batch.is_empty() {
                    break;
                }
                for p in &batch {
                    self.dirty.remove(p);
                }

                let meshes = mesh_chunks_parallel(&self.world, &self.registry, &batch);
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
            [p.x as f64 - HALF, p.y as f64 - HALF, p.z as f64 - HALF],
            [p.x as f64 + HALF, p.y as f64 + HALF, p.z as f64 + HALF],
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
        let center = WorldPos::new(
            self.camera.position.x.floor() as i64,
            self.camera.position.y.floor() as i64,
            self.camera.position.z.floor() as i64,
        );

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

        self.renderer = Some(renderer);
        self.window = Some(window);
        self.last_frame = Instant::now();
        self.set_cursor_captured(true);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
                                self.set_cursor_captured(false);
                            } else if code == KeyCode::KeyF {
                                // Toggle spectator (noclip free-fly) <-> survival.
                                self.camera.mode = match self.camera.mode {
                                    MoveMode::Spectator => MoveMode::Survival,
                                    MoveMode::Survival => MoveMode::Spectator,
                                };
                                self.camera.velocity = Vec3::ZERO;
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
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;

                match self.camera.mode {
                    MoveMode::Spectator => self.camera.update_spectator(&self.keys, dt),
                    MoveMode::Survival => self.physics_update(dt),
                }

                // Camera's current chunk drives both streaming and the
                // floating-origin render origin.
                let cam = WorldPos::new(
                    self.camera.position.x.floor() as i64,
                    self.camera.position.y.floor() as i64,
                    self.camera.position.z.floor() as i64,
                );
                let origin_chunk = cam.chunk();

                // Stream chunks in/out around the camera (borrows all of
                // self), before the render borrow below.
                self.stream_tick(origin_chunk);

                // Raycast from the camera to find the targeted block (M03
                // task 3). Uses loaded world data; unloaded cells read as air
                // (get_block returns AIR for missing chunks), so you can only
                // target visible blocks. Computed before the renderer borrow.
                let hit = {
                    let world = &self.world;
                    let registry = &self.registry;
                    let eye = [
                        self.camera.position.x as f64,
                        self.camera.position.y as f64,
                        self.camera.position.z as f64,
                    ];
                    let fwd = self.camera.forward();
                    let dir = [fwd.x as f64, fwd.y as f64, fwd.z as f64];
                    vox_core::raycast_voxels(eye, dir, REACH, |p| {
                        registry.is_solid(world.get_block(p))
                    })
                };
                self.targeted = hit;

                let loaded = self.streamer.loaded_count();
                let in_flight = self.gen_in_flight.len();
                let dirty = self.dirty.len();
                let relight = self.relight.len();

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

                    let origin_blocks = origin_chunk.origin();
                    let render_origin_blocks = Vec3::new(
                        origin_blocks.x as f32,
                        origin_blocks.y as f32,
                        origin_blocks.z as f32,
                    );
                    let view_proj = self
                        .camera
                        .view_proj(renderer.aspect(), render_origin_blocks);
                    renderer.render(view_proj.to_cols_array_2d());

                    // Telemetry once per second: FPS, frustum-culling ratio,
                    // and streaming state (resident chunks, gen queue, dirty).
                    self.telemetry_accum += dt;
                    self.telemetry_frames += 1;
                    if self.telemetry_accum >= 1.0 {
                        let fps = self.telemetry_frames as f32 / self.telemetry_accum;
                        log::info!(
                            "{:.0} fps | drawn {}/{} | loaded {} | gen {} | dirty {} | relight {} | lt {:.0}ms msh {:.0}ms",
                            fps,
                            renderer.drawn_last_frame(),
                            renderer.mesh_count(),
                            loaded,
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
    env_logger::init();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
