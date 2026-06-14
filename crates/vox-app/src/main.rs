//! vox-app: the game binary. Window, event loop, fly camera, and the
//! Milestone 00 test scene: one sine-wave chunk.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use vox_core::{BlockId, Chunk, ChunkPos, Streamer, World, WorldPos, WorldStore};
use vox_mesh::{mesh_chunk, ChunkNeighbors, MeshData};
use vox_render::Renderer;
use vox_worldgen::Generator;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

// ---------------------------------------------------------------------------
// Block appearance (vox-app owns block→color policy; IDs mirror vox-worldgen)
// ---------------------------------------------------------------------------

const STONE: BlockId = BlockId(1);
const DIRT: BlockId = BlockId(2);
const GRASS: BlockId = BlockId(3);

fn block_color(block: BlockId) -> [f32; 3] {
    match block {
        STONE => [0.55, 0.55, 0.58],
        DIRT => [0.55, 0.40, 0.27],
        GRASS => [0.33, 0.62, 0.28],
        _ => [1.0, 0.0, 1.0], // magenta = "you forgot a block type"
    }
}

/// Side length (in chunks) of the cube of world generated for this
/// milestone's acceptance scene. 16 → 4,096 chunks → a 512³-block world.
/// The six face-neighbor offsets, shared by streaming and edit re-meshing.
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
/// World immutably; see the streaming-tick comment). This caps how many
/// dirty chunks are meshed+uploaded per frame.
const MESH_BUDGET: usize = 48;

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
fn mesh_chunks_parallel(world: &World, positions: &[ChunkPos]) -> Vec<(ChunkPos, MeshData)> {
    positions
        .par_iter()
        .filter_map(|&pos| {
            let chunk = world.chunk(pos)?;
            let neighbors = ChunkNeighbors::of(world, pos);
            let mesh = mesh_chunk(chunk, &neighbors, block_color);
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

struct FlyCamera {
    position: Vec3,
    /// Radians. 0 looks along +X; positive turns toward +Z.
    yaw: f32,
    /// Radians, clamped to ±~89° so the view never flips.
    pitch: f32,
}

impl FlyCamera {
    const SPEED: f32 = 30.0; // m/s
    const SPRINT_MULTIPLIER: f32 = 4.0; // hold Ctrl
    const SENSITIVITY: f32 = 0.0022; // radians per mouse count
    const PITCH_LIMIT: f32 = 1.55; // just under PI/2

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

    fn update(&mut self, keys: &HashSet<KeyCode>, dt: f32) {
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
    /// On-disk world: generate-or-load on chunk-in, save modified on
    /// chunk-out (M02 task 5).
    store: WorldStore,
    /// Chunks whose generation has been spawned but whose result hasn't been
    /// received yet — prevents re-spawning the same chunk every frame.
    gen_in_flight: HashSet<ChunkPos>,
    /// Chunks needing a (re)mesh: newly generated chunks and any loaded
    /// neighbor whose border faces may have changed.
    dirty: HashSet<ChunkPos>,
    /// Async generation results arrive here from the rayon pool.
    gen_tx: Sender<(ChunkPos, Chunk)>,
    gen_rx: Receiver<(ChunkPos, Chunk)>,

    /// Telemetry: accumulate frames over ~1s to log FPS + drawn/total.
    telemetry_accum: f32,
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
            },
            keys: HashSet::new(),
            cursor_captured: false,
            last_frame: Instant::now(),

            generator: Generator::new(seed),
            streamer: Streamer::new(LOAD_RADIUS, UNLOAD_RADIUS),
            store,
            gen_in_flight: HashSet::new(),
            dirty: HashSet::new(),
            gen_tx,
            gen_rx,

            telemetry_accum: 0.0,
            telemetry_frames: 0,
        }
    }
}

impl App {
    /// Mark a chunk and its six face-neighbors dirty (needing a re-mesh).
    /// A chunk's border faces depend on its neighbors, so any edit or
    /// load/unload of a chunk can change its neighbors' meshes.
    fn mark_dirty_with_neighbors(&mut self, c: ChunkPos) {
        self.dirty.insert(c);
        for (dx, dy, dz) in NEIGHBOR_OFFSETS {
            self.dirty
                .insert(ChunkPos::new(c.x + dx, c.y + dy, c.z + dz));
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
    /// via `mesh_chunks_parallel` (all cores), capped at `MESH_BUDGET`
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
            self.mark_dirty_with_neighbors(pos);
        }

        // 5. Mesh a bounded batch of dirty chunks and upload. Only mesh
        //    chunks that are actually resident; skip (defer) others.
        if !self.dirty.is_empty() {
            let batch: Vec<ChunkPos> = self
                .dirty
                .iter()
                .copied()
                .filter(|p| self.world.chunk(*p).is_some())
                .take(MESH_BUDGET)
                .collect();
            for p in &batch {
                self.dirty.remove(p);
            }

            let meshes = mesh_chunks_parallel(&self.world, &batch);
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
        }
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
        // progressively, bounded by GEN_SPAWN_BUDGET / MESH_BUDGET.
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
                            } else if code == KeyCode::KeyG {
                                // Debug: punch a hole to exercise re-meshing.
                                self.debug_punch_hole();
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

            // Click to recapture the mouse after Esc.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.cursor_captured {
                    self.set_cursor_captured(true);
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

                self.camera.update(&self.keys, dt);

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

                let loaded = self.streamer.loaded_count();
                let in_flight = self.gen_in_flight.len();
                let dirty = self.dirty.len();

                if let Some(renderer) = self.renderer.as_mut() {
                    // Floating origin (ADR-0002): keep the render origin at
                    // the camera's current chunk so vertex math stays precise
                    // arbitrarily far from world zero. set_render_origin is a
                    // no-op when unchanged, so this is free while standing
                    // still and cheap (one uniform rewrite per chunk) when
                    // crossing a boundary.
                    renderer.set_render_origin(origin_chunk);

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
                            "{:.0} fps | drawn {}/{} | loaded {} | gen {} | dirty {}",
                            fps,
                            renderer.drawn_last_frame(),
                            renderer.mesh_count(),
                            loaded,
                            in_flight,
                            dirty,
                        );
                        self.telemetry_accum = 0.0;
                        self.telemetry_frames = 0;
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
