//! vox-app: the game binary. Window, event loop, fly camera, and the
//! Milestone 00 test scene: one sine-wave chunk.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use vox_core::{BlockId, ChunkPos, World};
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
const WORLD_CHUNKS: i64 = 16;

/// Translate every vertex of a locally-meshed chunk into world space by
/// adding the chunk's origin. The mesher emits positions in 0..32 local
/// space; this is where the chunk's world offset is baked in (see the
/// Milestone 01 spec's note on f32 precision far from origin — fine at
/// this scale, revisited via a future ADR before continent-scale worlds).
fn offset_mesh(mesh: &mut MeshData, chunk_pos: ChunkPos) {
    let origin = chunk_pos.origin();
    let (ox, oy, oz) = (origin.x as f32, origin.y as f32, origin.z as f32);
    for v in &mut mesh.vertices {
        v.position[0] += ox;
        v.position[1] += oy;
        v.position[2] += oz;
    }
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

    fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective_rh(70f32.to_radians(), aspect, 0.1, 1000.0);
        let view = Mat4::look_to_rh(self.position, self.forward(), Vec3::Y);
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
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            renderer: None,
            world: World::new(),
            // Up high and back, looking down into the generated terrain.
            camera: FlyCamera {
                position: Vec3::new(-20.0, 60.0, -20.0),
                yaw: std::f32::consts::FRAC_PI_4, // 45°: toward world center
                pitch: -0.45,
            },
            keys: HashSet::new(),
            cursor_captured: false,
            last_frame: Instant::now(),
        }
    }
}

impl App {
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

        let mut renderer = Renderer::new(window.clone());

        // --- Build the Milestone 01 acceptance scene: a WORLD_CHUNKS³ cube
        // of generated terrain, meshed per chunk with cross-chunk culling,
        // uploaded once. Single-threaded (rayon is task 6). ---
        let gen_start = Instant::now();
        let generator = Generator::new(0x0007_E22A_C0DE);
        let mut world = World::new();
        for cy in 0..WORLD_CHUNKS {
            for cz in 0..WORLD_CHUNKS {
                for cx in 0..WORLD_CHUNKS {
                    let pos = ChunkPos::new(cx, cy, cz);
                    world.insert_chunk(pos, generator.generate_chunk(pos));
                }
            }
        }
        let gen_ms = gen_start.elapsed().as_secs_f32() * 1000.0;

        let mesh_start = Instant::now();
        let mut total_quads = 0usize;
        let mut nonempty = 0usize;
        for (pos, chunk) in world.chunks() {
            let neighbors = ChunkNeighbors::of(&world, pos);
            let mut mesh = mesh_chunk(chunk, &neighbors, block_color);
            if !mesh.is_empty() {
                offset_mesh(&mut mesh, pos);
                total_quads += mesh.quad_count();
                nonempty += 1;
                renderer.set_chunk_mesh(pos, &mesh);
            }
        }
        let mesh_ms = mesh_start.elapsed().as_secs_f32() * 1000.0;

        log::info!(
            "world: {} chunks ({} non-empty meshes), {} quads | gen {:.0}ms, mesh {:.0}ms",
            world.chunk_count(),
            nonempty,
            total_quads,
            gen_ms,
            mesh_ms,
        );

        self.world = world;
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.last_frame = Instant::now();
        self.set_cursor_captured(true);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

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

                if let Some(renderer) = self.renderer.as_mut() {
                    let view_proj = self.camera.view_proj(renderer.aspect());
                    renderer.render(view_proj.to_cols_array_2d());
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
