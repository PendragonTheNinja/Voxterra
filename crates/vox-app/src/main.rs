//! vox-app: the game binary. Window, event loop, fly camera, and the
//! Milestone 00 test scene: one sine-wave chunk.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec3, Mat4, Vec3};
use vox_core::{
    cell_overlaps_aabb, BlockId, ChunkPos, EditedColumns, LodNodeId, LodRing, RayHit, WorldMeta,
    WorldPos, WorldShape, WorldStore,
};
use vox_mesh::LodMeshData;

mod bench;
mod session;
mod settings_ui;

// The benchmark flies at spectator sprint speed; keep the two one number.
const _: () = assert!(FlyCamera::SPEED * FlyCamera::SPRINT_MULTIPLIER == bench::SPRINT_SPEED);
use session::{background_pool, Session, Spawner, CHUNK_SIZE_I};
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

// --- LOD (M09). LOD is configured as a list of levels (finest to coarsest). Radii are in
// CHUNKS, each level's around its own snapped centre (ADR-0014), and must lie on
// the grids the ring asserts (`LodLevel::extended` puts them there). LOD starts at 0 — it underlaps the full-res region, which
// simply draws on top; see the vox-core::lod docs on why a flush inner edge
// would open a gap. Multi-level tuning lands with the octree wiring; today this
// is the single level M08 shipped.
const LOD_INNER_CHUNKS: i64 = 0;
// Finest to coarsest. Each radius is CHUNKS from that level's snapped centre and
// must be a multiple of the next level's stride (the ring asserts it).
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
const LOD_UNLOAD_MARGIN_CHUNKS: i64 = 8; // each level uses at least its snap step (ADR-0014)

// Fog (M09 amendment A2) is tuned live from the settings menu; its defaults
// live in `vox_core::Settings`.
const LOD_SKIRT_DEPTH_CELLS: i32 = 3; // multiplied by stride for block depth
                                      // Coarse nodes generated+meshed per frame. Sized so a full ring (up to ~2400
                                      // nodes with extra levels) fills in well under a second instead of appearing in
                                      // visibly staggered rings; the work is on the rayon pool, not the frame thread.
const LOD_SPAWN_BUDGET: usize = 48;

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

/// The player's world folder, beside the executable's working directory.
const WORLD_DIR: &str = "world";

/// The benchmark's world folder, suffixed with the generator version.
///
/// `--bench` used to open the player's own world. So it measured whatever
/// that world had become — every edit ever made in it is a saved chunk to
/// load and a column for the LOD overlay — and it refused to run at all
/// when that world was made by an older generator, which is how the M14
/// re-bench stopped the owner on 2026-11-04. A benchmark needs the same
/// world every time: this one is created from the default seed on first
/// use, never edited (the benchmark flies; it does not dig), and a new
/// generator version simply gets a new folder. It sits under `bench/`,
/// which git ignores, with the reports.
const BENCH_WORLD_DIR: &str = "bench/world";

/// Seconds between autosaves of edited chunks ([`App::save_unsaved`]).
///
/// Edits used to reach the disk only when their chunk unloaded or the game
/// closed cleanly, so a crash — a panic, a lost GPU device, the power — took
/// every block placed or broken near the player since they arrived. A
/// minute is short enough to lose little and long enough that the write
/// (only chunks changed since the last one) is never felt.
const AUTOSAVE_SECS: f32 = 60.0;

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

/// The world's climate map as texels for `Renderer::set_climate` (M13,
/// ADR-0018): a cell's sea-level temperature and precipitation, row by row.
fn climate_texels(generator: &Generator) -> (Vec<u8>, (u32, u32)) {
    let climate = generator.climate();
    let (nx, nz) = climate.cells();
    let mut texels = Vec::with_capacity(nx * nz * 4);
    for j in 0..nz {
        for i in 0..nx {
            let (sea_level_c, precip, _, _) = climate.cell(i, j);
            texels.extend_from_slice(&vox_render::encode_climate(sea_level_c, precip));
        }
    }
    (texels, (nx as u32, nz as u32))
}

/// Where the benchmark's water phases start (M14), from the drainage
/// network alone, so a given world always benchmarks the same water:
/// the rivers phase where the sprint path, along +X, crosses the most river
/// and lake cells within ~15 km of spawn; the waterfall phase downstream of
/// the largest waterfall there that falls toward −X, so the camera, looking
/// +X, faces its sheet. Positions in the spawn's frame (no wrapping).
fn bench_water_starts(
    generator: &Generator,
    sx: i64,
    sz: i64,
    path: i64,
    above: f64,
) -> (Option<[f64; 3]>, Option<[f64; 3]>) {
    let d = generator.drainage();
    let (nx, nz) = d.cells();
    let (cw, ch) = d.cell_blocks();
    let (si, sj) = d.cell_of(sx, sz);
    let wrap = |i: i64, j: i64| {
        (
            i.rem_euclid(nx as i64) as usize,
            j.rem_euclid(nz as i64) as usize,
        )
    };
    // Unwrapped cell (si + di, sj + dj) to a world position near spawn.
    let at = |di: i64, dj: i64| {
        (
            sx - sx.rem_euclid(cw as i64) + di * cw as i64 + (cw / 2.0) as i64,
            sz - sz.rem_euclid(ch as i64) + dj * ch as i64 + (ch / 2.0) as i64,
        )
    };
    const REACH: i64 = 120;
    let path_cells = (path as f64 / cw).ceil() as i64;
    let mut best_rivers: Option<(usize, i64, i64, i64)> = None; // (score, -dist, di, dj)
    let mut best_fall: Option<(f32, i64, i64)> = None;
    for dj in -REACH..=REACH {
        for di in -REACH..=REACH {
            let (i, j) = wrap(si as i64 + di, sj as i64 + dj);
            if di % 3 == 0 && dj % 3 == 0 && !d.is_sea(i, j) {
                let score = (0..path_cells)
                    .filter(|&k| {
                        let (ci, cj) = wrap(si as i64 + di + k, sj as i64 + dj);
                        d.river_kind_at(ci, cj).is_some() || d.is_lake(ci, cj)
                    })
                    .count();
                let key = (score, -(di * di + dj * dj), di, dj);
                if best_rivers.is_none_or(|b| key > b) {
                    best_rivers = Some(key);
                }
            }
            if d.waterfall_at(i, j) {
                let (_, _, q, (ri, _)) = d.cell(i, j);
                let west = (ri as i64 - i as i64).rem_euclid(nx as i64) == nx as i64 - 1;
                if west && best_fall.is_none_or(|b| q > b.0) {
                    best_fall = Some((q, di, dj));
                }
            }
        }
    }
    let rivers = best_rivers.filter(|b| b.0 > 0).map(|(_, _, di, dj)| {
        let (x, z) = at(di, dj);
        let highest = (0..=path)
            .step_by(64)
            .map(|k| generator.surface_height(x + k, z))
            .max()
            .unwrap_or(0);
        [x as f64 + 0.5, highest as f64 + above, z as f64 + 0.5]
    });
    let waterfall = best_fall.map(|(_, di, dj)| {
        let (x, z) = at(di, dj);
        let (i, j) = wrap(si as i64 + di, sj as i64 + dj);
        let (_, upper, _, _) = d.cell(i, j);
        // The lip is where the ground falls away, that far of the way to
        // the receiver a cell west; stand ~100 blocks downstream of it, a
        // little above the pool's level.
        let lip = f64::from(d.waterfall_lip(i, j).unwrap_or(0.5));
        let lip_x = x - (cw * lip) as i64;
        // Never inside the hillside: the ground there can stand above the
        // pool.
        let ground = generator.surface_height(lip_x - 100, z);
        [
            (lip_x - 100) as f64 + 0.5,
            f64::from(upper).max(ground as f64) + 12.0,
            z as f64 + 0.5,
        ]
    });
    (rivers, waterfall)
}

/// Whether flowing water is switched off (`VOX_NO_FLOW=1`): all water still,
/// for measuring what the flow shader costs.
fn no_flow() -> bool {
    std::env::var("VOX_NO_FLOW").is_ok_and(|v| v == "1")
}

/// The drainage network's flow as texels for `Renderer::set_flow` (M14):
/// each cell's water velocity, row by row.
fn flow_texels(generator: &Generator) -> (Vec<u8>, (u32, u32)) {
    let drainage = generator.drainage();
    let (nx, nz) = drainage.cells();
    let mut texels = Vec::with_capacity(nx * nz * 2);
    for j in 0..nz {
        for i in 0..nx {
            let (vx, vz) = drainage.flow(i, j);
            texels.extend_from_slice(&vox_render::encode_flow(vx, vz));
        }
    }
    (texels, (nx as u32, nz as u32))
}

// ---------------------------------------------------------------------------
// Fly camera
// ---------------------------------------------------------------------------

/// How the player moves through the world.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MoveMode {
    /// Free-fly noclip (the original behavior): WASD relative to yaw,
    /// Space/Shift for vertical, Ctrl to sprint, the scroll wheel sets the
    /// speed. Passes through blocks.
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
    /// Spectator speed, in quarter-octave steps from [`Self::SPEED`]: the
    /// scroll wheel moves it, sprint multiplies whatever it is. Survival
    /// ignores it (the wheel picks blocks there), and the bench sets the
    /// camera's position itself, so neither depends on it.
    fly_step: i32,
}

impl FlyCamera {
    const SPEED: f64 = 30.0; // m/s (spectator)
    const SPRINT_MULTIPLIER: f64 = 4.0; // hold Ctrl (spectator)
    /// Scroll-wheel range: 2^(±12/4) = ⅛× to 8× — 3.75 to 240 m/s, sprint
    /// 15 to 960. Each notch is ×1.19, four notches double it.
    const FLY_STEPS: std::ops::RangeInclusive<i32> = -12..=12;
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
                self.fly_speed() * Self::SPRINT_MULTIPLIER
            } else {
                self.fly_speed()
            };
            self.position += dir.normalize() * speed * dt;
        }
    }

    /// Spectator speed before sprint, m/s, at the wheel's setting.
    fn fly_speed(&self) -> f64 {
        Self::SPEED * 2f64.powf(f64::from(self.fly_step) / 4.0)
    }

    /// Move the wheel's speed setting `notches` up (positive) or down,
    /// within [`Self::FLY_STEPS`].
    fn adjust_fly_speed(&mut self, notches: i32) {
        self.fly_step =
            (self.fly_step + notches).clamp(*Self::FLY_STEPS.start(), *Self::FLY_STEPS.end());
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
    /// The world and the pipeline that streams, lights and meshes it
    /// (`session.rs`): everything about the world that needs no window.
    session: Session,
    camera: FlyCamera,
    keys: HashSet<KeyCode>,
    cursor_captured: bool,
    last_frame: Instant,
    /// Seconds since edited chunks were last written ([`AUTOSAVE_SECS`]).
    autosave_accum: f32,

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
    /// Block currently under the crosshair (raycast result), or none.
    targeted: Option<RayHit>,
    /// Block type placed on right-click (M03 task 4). Cycled with number
    /// keys / scroll among the registry's placeable blocks.
    selected_block: BlockId,

    /// The thread pool for work nobody waits on: chunk generation and LOD
    /// node building ([`background_pool`]). Relighting and meshing, which
    /// the frame DOES wait on, stay on rayon's global pool.
    background: Arc<rayon::ThreadPool>,

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
    /// Debug (`J`): build LOD with flat-cell merging (the default) or one top
    /// quad per cell, to tell whether an artifact comes from merging.
    lod_merge: bool,
    /// Debug (`H`): sky painted magenta so holes in the terrain stand out.
    debug_holes: bool,
    /// Nodes wanted but not yet spawned (the ring is requested all at once but
    /// generated a few per frame). Drained nearest-camera-first, so this is a
    /// set rather than a queue — insertion order carries no meaning.
    lod_pending_set: HashSet<LodNodeId>,

    /// Telemetry: accumulate frames over ~1s to log FPS + drawn/total.
    telemetry_accum: f32,
    /// `--bench` was given: run the benchmark script once the renderer is up.
    bench_requested: bool,
    bench: Option<bench::Bench>,
    bench_started: Option<Instant>,
    /// P was pressed: run `Renderer::hole_census` after this frame.
    census_requested: bool,
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

impl App {
    /// Open the world and stand the player in it. `bench_requested` is
    /// `--bench`: the benchmark runs in a world folder of its own
    /// ([`BENCH_WORLD_DIR`]).
    fn new(bench_requested: bool) -> Self {
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
        let world_dir = if bench_requested {
            format!("{BENCH_WORLD_DIR}-v{}", vox_worldgen::GENERATOR_VERSION)
        } else {
            WORLD_DIR.to_string()
        };
        let store = WorldStore::open(&world_dir, new_world)
            .unwrap_or_else(|e| panic!("cannot open the world at \"{world_dir}\": {e}"));
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
        // Stage-1 climate (ADR-0017), built here so world load pays for it
        // rather than whichever chunk happens to be generated first.
        let started = Instant::now();
        let climate = generator.climate();
        log::info!(
            "climate map: {:?} built in {:.0} ms",
            climate,
            started.elapsed().as_secs_f64() * 1000.0
        );
        // Stage-1 drainage and the erosion stage (M14, ADR-0019): every
        // surface query reads them, so they too are made ready here, before
        // the first chunk — from the world folder's cache when it holds this
        // world's network, else built and cached for next time.
        let started = Instant::now();
        let (drainage, source) = generator.drainage_cached(store.root());
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        match source {
            vox_worldgen::DrainageSource::BuiltUnsaved(e) => {
                log::warn!(
                    "drainage network: {drainage:?} built in {ms:.0} ms; cache not written: {e}"
                )
            }
            source => log::info!("drainage network: {drainage:?} ({source:?}) in {ms:.0} ms"),
        }
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

        let background = Arc::new(background_pool());
        let session = Session::new(
            store,
            generator,
            edited_columns,
            vox_core::Settings::default().load_radius,
            Spawner::Pool(Arc::clone(&background)),
        );

        Self {
            window: None,
            renderer: None,
            session,
            // On the ground at the resolved spawn, looking out across it.
            camera: FlyCamera {
                position: DVec3::new(spawn_x as f64, spawn_y, spawn_z as f64),
                yaw: std::f64::consts::FRAC_PI_4,
                pitch: -0.45,
                velocity: DVec3::ZERO,
                on_ground: false,
                mode: MoveMode::Spectator,
                fly_step: 0,
            },
            keys: HashSet::new(),
            cursor_captured: false,
            last_frame: Instant::now(),
            autosave_accum: 0.0,

            lod_ring: LodRing::new(
                LOD_INNER_CHUNKS,
                &LOD_LEVELS,
                LOD_UNLOAD_MARGIN_CHUNKS,
                (vox_core::WORLD_Y_MIN_BLOCKS, vox_core::WORLD_Y_MAX_BLOCKS),
            ),
            settings: vox_core::Settings::default(),
            settings_applied: vox_core::Settings::default(),
            ui: None,
            lod_far_chunks: LOD_LEVELS[LOD_LEVELS.len() - 1].outer_chunks,
            targeted: None,
            selected_block: vox_core::registry::STONE,
            background,
            lod_tx,
            lod_rx,
            lod_in_flight: HashSet::new(),
            lod_retired: HashSet::new(),
            lod_retire_age: 0,
            lod_skirts: true,
            lod_merge: true,
            debug_holes: false,
            lod_pending_set: HashSet::new(),

            telemetry_accum: 0.0,
            bench_requested,
            bench: None,
            bench_started: None,
            census_requested: false,
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
                    // PHYSICAL (ADR-0011): only solid blocks stop the player.
                    if self
                        .session
                        .registry
                        .is_solid(self.session.world.get_block(wp))
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
            fly_step: self.camera.fly_step,
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

    /// World-space origin chunk of a LOD node. Y comes from the node's own
    /// vertical index — columns stack to cover the world band (M09).
    fn lod_node_origin_chunk(ring: &LodRing, id: LodNodeId) -> ChunkPos {
        let (bx, by, bz) = ring.node_origin_blocks(id);
        ChunkPos::new(bx / CHUNK_SIZE_I, by / CHUNK_SIZE_I, bz / CHUNK_SIZE_I)
    }

    /// Outermost LOD radius in blocks, for sizing the camera's far plane and
    /// the automatic fog range.
    /// Begin the benchmark (`--bench`, bench.rs): fix the settings and the
    /// light, and find where its land and ocean phases start.
    ///
    /// Both starts are derived from the world alone, so a given world always
    /// benchmarks the same ground: land at the spawn, ocean at the nearest
    /// point around it with deep water under the whole sprint path. The
    /// camera flies 40 blocks over the highest ground (or the sea) along that
    /// path, so a sprint never passes through a hill.
    fn start_bench(&mut self) {
        let generator = self.session.generator.clone();
        let shape = self.session.store.shape();
        let (sx, sz) = (
            self.camera.position.x.floor() as i64,
            self.camera.position.z.floor() as i64,
        );
        let path = (bench::SPRINT_SPEED * bench::SPRINT_SECS) as i64;
        const ABOVE: f64 = 40.0;
        let highest = (0..=path)
            .step_by(64)
            .map(|d| generator.surface_height(sx + d, sz))
            .max()
            .unwrap_or(0);
        let land = [sx as f64 + 0.5, highest as f64 + ABOVE, sz as f64 + 0.5];
        let sea = vox_core::SEA_LEVEL_BLOCKS;
        let ocean_xz = vox_core::find_spawn(
            shape,
            sx,
            sz,
            SPAWN_SEARCH_STRIDE,
            SPAWN_SEARCH_RINGS,
            |x, z| {
                (0..=path)
                    .step_by(300)
                    .all(|d| generator.surface_height(x + d, z) < sea - 30)
            },
        );
        let (gpu, (w, h)) = self
            .renderer
            .as_ref()
            .map(|r| (r.gpu_name().to_string(), r.surface_size()))
            .unwrap_or_default();
        let mut header = vec![
            format!("GPU: {gpu}, {w}×{h}"),
            format!(
                "World: seed {:#x}, {} × {} km; start ({sx}, {sz})",
                self.session.store.seed(),
                shape.size_x() / 1000,
                shape.size_z() / 1000
            ),
            "Settings: defaults (full-res radius 8, curvature on, air density 1), \
             spectator, noon, time frozen"
                .to_string(),
            format!(
                "Flowing water: {}",
                if no_flow() {
                    "off (VOX_NO_FLOW=1)"
                } else {
                    "on"
                }
            ),
        ];
        let ocean = match ocean_xz {
            Some((x, z)) => [x as f64 + 0.5, (sea + 1) as f64 + ABOVE, z as f64 + 0.5],
            None => {
                header.push("No deep ocean found near spawn: ocean phases ran on land".into());
                land
            }
        };
        let (rivers, waterfall) = bench_water_starts(&generator, sx, sz, path, ABOVE);
        let rivers = rivers.unwrap_or_else(|| {
            header.push("No rivers found near spawn: river phases ran on land".into());
            land
        });
        let waterfall = waterfall.unwrap_or_else(|| {
            header.push("No waterfall facing the camera near spawn: its phase ran on land".into());
            land
        });
        self.settings = vox_core::Settings::default();
        self.camera.mode = MoveMode::Spectator;
        self.time_paused = true;
        self.time_accum = 0.5 * vox_core::TICKS_PER_DAY as f64;
        self.world_time = vox_core::WorldTime::from_ticks(self.time_accum as u64);
        log::info!("bench: starting — land at {land:?}, ocean at {ocean:?}");
        log::info!("bench: rivers at {rivers:?}, waterfall at {waterfall:?}");
        self.bench = Some(bench::Bench::new(
            bench::Starts {
                land,
                ocean,
                rivers,
                waterfall,
            },
            header,
        ));
        self.bench_started = Some(Instant::now());
    }

    /// What the `dirty` set is waiting on, judged as the mesh pass judges
    /// it (for the benchmark: `bench::DirtySplit`).
    fn dirty_split(&self) -> bench::DirtySplit {
        let camera_chunk = self.camera.block_pos().chunk();
        let mut split = bench::DirtySplit::default();
        for &p in &self.session.dirty {
            if self.session.relight.contains(&p) {
                split.relight += 1;
            } else if self.session.ready_for_first_mesh(p, camera_chunk) {
                split.ready += 1;
            } else {
                split.neighbours += 1;
            }
        }
        split
    }

    /// The column under the camera, as the generator classifies it (M13):
    /// biome, mean annual temperature at the ground, annual precipitation
    /// and latitude — `docs/biomes.md`'s terms, for play-testing climate.
    /// The seed's column, not edits: a dug pit reads as the ground it was.
    fn place_readout(&self) -> String {
        let (x, z) = (
            self.camera.position.x.floor() as i64,
            self.camera.position.z.floor() as i64,
        );
        let c = self.session.generator.column(x, z);
        format!(
            "{} {:.0}C {:.0}mm lat {:.1}",
            c.biome.name(),
            c.temperature,
            c.precipitation,
            self.session.store.shape().latitude_degrees(z)
        )
    }

    /// One frame of the benchmark: measure, then do what the script says.
    /// Returns true when the run is over and the program should exit.
    fn bench_tick(&mut self, raw_dt: f32) -> bool {
        let now = self
            .bench_started
            .map_or(0.0, |t| t.elapsed().as_secs_f64());
        let (lod_nodes, gpu_mb, tris, tris_drawn, draws, encode_ms) =
            self.renderer.as_ref().map_or((0, 0.0, 0, 0, 0, 0.0), |r| {
                (
                    r.lod_count(),
                    r.buffer_bytes() as f32 / 1.048576e6,
                    r.tris_last_frame(),
                    r.area_tris_last_frame(),
                    r.draws_last_frame(),
                    r.encode_ms_last_frame(),
                )
            });
        let sample = bench::Sample {
            frame_ms: raw_dt * 1000.0,
            dirty: self.session.dirty.len(),
            relight: self.session.relight.len(),
            gen_in_flight: self.session.gen_in_flight.len(),
            lod_backlog: self.lod_pending_set.len() + self.lod_in_flight.len(),
            lod_nodes,
            gpu_mb,
            tris,
            tris_drawn,
            draws,
            encode_ms,
            loaded: self.session.streamer.loaded_count(),
            relight_ms_total: self.session.relight_ms_total,
            mesh_ms_total: self.session.mesh_ms_total,
            meshed_total: self.session.meshed_total,
            mesh_cpu_ms_total: self.session.mesh_cpu_ms_total,
            relit_total: self.session.relit_total,
            relight_cpu_ms_total: self.session.relight_cpu_ms_total,
            dirty_split: self
                .bench
                .as_ref()
                .is_some_and(|b| b.ends_this_frame(now))
                .then(|| self.dirty_split()),
        };
        let Some(b) = self.bench.as_mut() else {
            return false;
        };
        let tick = b.tick(now, sample);
        if let Some(levels) = tick.apply_levels {
            // Applied this frame by the usual settings check, which rebuilds
            // the LOD exactly once.
            self.settings.lod_extra_levels = levels;
        }
        if let Some(eye) = tick.camera {
            self.camera.position = DVec3::from_array(eye);
            self.camera.velocity = DVec3::ZERO;
            self.camera.yaw = 0.0; // along +X, the sprint direction
            self.camera.pitch = bench::PITCH;
        }
        let Some(report) = tick.done else {
            return false;
        };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = format!("bench/bench-{secs}.md");
        match std::fs::create_dir_all("bench").and_then(|()| std::fs::write(&path, &report)) {
            Ok(()) => log::info!("bench: results written to {path}"),
            Err(e) => log::error!("bench: could not write {path}: {e}"),
        }
        for line in report.lines() {
            log::info!("{line}");
        }
        true
    }

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
    /// one step of the level's inner edge: a node the ring has retired stays
    /// drawn until its replacement lands, by which time the camera may be one
    /// snap step nearer. That step is the level's own stride since each level
    /// snaps separately (ADR-0014); under the shared centre it was the
    /// coarsest stride, which at eight levels would have made every level
    /// exact. Decided per level, never per node, so no node ever needs a
    /// rebuild because the camera moved.
    fn lod_sampling(&self, level: u32) -> LodSampling {
        let reach = self.session.streamer.unload_radius() + self.lod_ring.inner_snap_chunks(level);
        if self.lod_ring.closest_approach_chunks(level) <= reach {
            LodSampling::Exact
        } else {
            LodSampling::Sparse
        }
    }

    /// LOD nodes every column of which full resolution draws, and which
    /// therefore need not be drawn at all.
    ///
    /// LOD underlaps the full-res region by design (that is what guarantees no
    /// gaps). A coarse node cannot represent a hole the player dug, and under
    /// translucent water it cannot hide behind the real terrain at all — so it
    /// must not be drawn wherever the real chunks already are. Whole nodes are
    /// skipped here; the LOD shader discards per column for the rest.
    ///
    /// Retired nodes are still DRAWN but are no longer in `loaded()`, so they
    /// are considered too — otherwise a node lingering over ground the player
    /// has dug shows its coarse surface through the hole for the few frames
    /// before it is flushed.
    fn covered_lod_nodes(&self, fullres: &HashSet<(i64, i64)>) -> HashSet<(ChunkPos, u32)> {
        let mut out = HashSet::new();
        for id in self.lod_ring.loaded().iter().chain(self.lod_retired.iter()) {
            let s = self.lod_ring.stride(id.level);
            let (ox, oz) = self.lod_ring.node_origin_chunk_xz(*id);
            let covered = (oz..oz + s).all(|cz| (ox..ox + s).all(|cx| fullres.contains(&(cx, cz))));
            if covered {
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
        // The edit overlay must reflect the edit BEFORE the node regenerates
        // from it, or the rebuilt node restores what was just mined:
        // `Session::set_block` has already recorded the column.
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
        self.session
            .streamer
            .reconfigure(Session::stream_config(s.load_radius));
        // Extend view distance by APPENDING coarser levels (see
        // `LodLevel::extended`, which also puts every radius on the grid the
        // ring requires — one rule for the menu and the tests).
        // A small world clamps the horizon below half its width, or the same
        // terrain would be visible twice around it (ADR-0012 rule 3). The
        // clamped value is written back so the menu shows what is in effect.
        let wanted = s.lod_extra_levels.max(0) as u32;
        let extra = vox_core::LodLevel::extra_levels_within(
            &LOD_LEVELS,
            wanted,
            LOD_INNER_CHUNKS,
            self.session.store.shape(),
        );
        if extra < wanted {
            log::info!(
                "extra LOD levels clamped {wanted} -> {extra}: the horizon must stay under half this world's width"
            );
            self.settings.lod_extra_levels = extra as i32;
        }
        let levels = vox_core::LodLevel::extended(&LOD_LEVELS, extra, LOD_INNER_CHUNKS);
        self.lod_far_chunks = levels.last().expect("at least one level").outer_chunks;

        // Actually install the new ring. Losing this line meant the ring kept
        // every node marked loaded with its centre already set, so update()
        // returned nothing and LOD never came back until the camera crossed a
        // node boundary — exactly the reported symptom.
        // The whole world's height: one node spans it at every level (cells
        // are `stride` wide but band/32 tall), affordable because streaming
        // follows the surface and nothing loads a full column.
        let band = (vox_core::WORLD_Y_MIN_BLOCKS, vox_core::WORLD_Y_MAX_BLOCKS);
        self.lod_ring = LodRing::new(LOD_INNER_CHUNKS, &levels, LOD_UNLOAD_MARGIN_CHUNKS, band);

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
        // Every block's face layers, by id, for the LOD workers: distant
        // terrain wears each biome's own top block (M13), and the sea surface
        // is the same translucent water as up close (ADR-0011 decision 1c).
        let layers: Arc<Vec<[u32; 6]>> = Arc::new(
            (0..self.session.registry.len())
                .map(|id| {
                    std::array::from_fn(|f| self.session.registry.face_layer(BlockId(id as u16), f))
                })
                .collect(),
        );

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
                // Copied into the rayon job (a cheap clone: the climate map is shared behind an Arc).
                let sampling = self.lod_sampling(n.level);
                let generator = self.session.generator.clone();
                let tx = self.lod_tx.clone();
                let origin_y = origin.y as i32;
                let skirt = if self.lod_skirts {
                    LOD_SKIRT_DEPTH_CELLS * stride as i32
                } else {
                    0 // no border walls at all (see `lod_skirts`)
                };
                let edits = Arc::clone(&self.session.edited_columns);
                let merge = self.lod_merge;
                let layers = Arc::clone(&layers);
                self.background.spawn(move || {
                    let (mut heights, tops, mut water) =
                        generator.lod_surface(origin.x, origin.z, stride, sampling);
                    // Fold in player edits. The seed always reads higher than
                    // dug ground, so without this the coarse surface floats
                    // above the hole and shows through it (the "ghost block").
                    edits.apply_to_node(&mut heights, origin.x, origin.z, stride, origin_y);
                    // Ground dug below sea level shows the sea, as before M14.
                    for (w, sea) in water.iter_mut().zip(vox_mesh::lod_sea(&heights)) {
                        if *w == vox_mesh::LOD_DRY {
                            *w = sea;
                        }
                    }
                    let mesh = vox_mesh::mesh_lod_cells(
                        vox_mesh::LodCells {
                            heights: &heights,
                            tops: &tops,
                            water: &water,
                        },
                        stride as i32,
                        origin_y,
                        |b, face| layers.get(b.0 as usize).map_or(0, |l| l[face]),
                        skirt,
                        merge,
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
        // PHYSICAL (ADR-0011): only solid blocks can be broken.
        if !self
            .session
            .registry
            .is_solid(self.session.world.get_block(pos))
        {
            return;
        }
        self.session.set_block(pos, BlockId::AIR);
        self.invalidate_lod_at(pos);
        log::info!("broke block at {:?}", (pos.x, pos.y, pos.z));
    }

    /// Place the selected block against the targeted face (right-click).
    /// Rejects placement into a non-empty cell or one overlapping the
    /// player (so you can't entomb the camera). Re-meshes + persists.
    fn place_block(&mut self) {
        let Some(hit) = self.targeted else { return };
        let Some(pos) = hit.place_pos else { return };
        // PHYSICAL (ADR-0011): a cell is occupied only by a solid block, so
        // placing into water replaces it.
        if self
            .session
            .registry
            .is_solid(self.session.world.get_block(pos))
        {
            return; // target cell occupied
        }
        // Reject if the cell would overlap the player's box.
        let (min, max) = self.player_aabb();
        if cell_overlaps_aabb(pos, min, max) {
            return;
        }
        // Only into a chunk that is resident. `World::set_block` creates a
        // missing chunk, and one created here is known to neither the
        // streamer nor the heightmap: when the real chunk arrives it replaces
        // this one — the block silently vanishes — and the column's resident
        // count is left one short (2026-11 audit). Reach is 6 blocks, so this
        // only ever refuses a cell in a chunk still streaming in.
        if self.session.world.chunk(pos.chunk()).is_none() {
            return;
        }
        self.session.set_block(pos, self.selected_block);
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
        let placeable: Vec<BlockId> = self.session.registry.placeable().collect();
        if slot >= 1 && slot <= placeable.len() {
            self.selected_block = placeable[slot - 1];
            self.log_selection();
        }
    }

    /// Cycle the selected block forward (+1) or backward (-1), from scroll.
    fn cycle_selection(&mut self, dir: i32) {
        let placeable: Vec<BlockId> = self.session.registry.placeable().collect();
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
            self.session.registry.get(self.selected_block).name
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
                    // Deliberately `is_air`, not a registry property: the
                    // debug hole clears everything, water included.
                    if !self.session.world.get_block(pos).is_air() {
                        self.session.world.set_block(pos, BlockId::AIR);
                        touched.insert(pos.chunk());
                    }
                }
            }
        }
        for c in touched {
            self.session.mark_dirty_with_neighbors(c);
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

        let mut renderer = Renderer::new(window.clone());
        // Biome colour (M13, ADR-0018): the world's climate map, for the
        // terrain shaders to tint grass by, near and far alike.
        let (texels, cells) = climate_texels(&self.session.generator);
        let shape = self.session.store.shape();
        renderer.set_climate(
            &texels,
            cells,
            (shape.size_x(), shape.size_z()),
            vox_worldgen::climate::LAPSE_PER_BLOCK,
        );
        // Rivers drift with their flow (M14), placed as the climate map is.
        // VOX_NO_FLOW=1 leaves all water still: an A/B switch for measuring
        // what flowing water costs (`--bench` notes it in its header).
        if !no_flow() {
            let (flow, cells) = flow_texels(&self.session.generator);
            renderer.set_flow(&flow, cells);
        }

        // The world is no longer pre-built: chunks stream in around the
        // camera each frame via stream_tick (M02). resumed() just stands up
        // the window/renderer; the first frames fill the initial sphere
        // progressively, bounded by GEN_SPAWN_BUDGET and the per-frame mesh time budget.
        log::info!(
            "streaming: load radius {} / unload {} chunks",
            self.session.streamer.load_radius(),
            self.session.streamer.unload_radius()
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
                self.session.save_unsaved("exit");
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
                            } else if code == KeyCode::KeyH {
                                // Debug: magenta sky, so cracks and missing
                                // faces show as unmistakable magenta pixels.
                                self.debug_holes = !self.debug_holes;
                                if let Some(r) = self.renderer.as_mut() {
                                    r.set_debug_holes(self.debug_holes);
                                }
                                log::info!(
                                    "hole check (magenta sky) {}",
                                    if self.debug_holes { "ON" } else { "off" }
                                );
                            } else if code == KeyCode::KeyJ {
                                // Debug: rebuild LOD with or without flat-cell
                                // merging, to tell whether an artifact is it.
                                self.lod_merge = !self.lod_merge;
                                self.reset_lod();
                                log::info!(
                                    "LOD flat-cell merging {} (rebuilding LOD)",
                                    if self.lod_merge { "ON" } else { "off" }
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
                            } else if code == KeyCode::KeyP {
                                // Debug: everything needed to rebuild this
                                // exact frame headlessly (the speck hunt).
                                let (w, h) =
                                    self.renderer.as_ref().map_or((0, 0), |r| r.surface_size());
                                log::info!(
                                    "VIEW pos {:.2} {:.2} {:.2} yaw {:.4} pitch {:.4} window {w}x{h} time {:.3} | {} | {:?}",
                                    self.camera.position.x,
                                    self.camera.position.y,
                                    self.camera.position.z,
                                    self.camera.yaw,
                                    self.camera.pitch,
                                    self.world_time.time_of_day(),
                                    self.place_readout(),
                                    self.settings
                                );
                                self.census_requested = true;
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

            // Scroll wheel: flight speed in spectator (number keys still
            // pick blocks there), the selected block in survival.
            WindowEvent::MouseWheel { delta, .. } => {
                if self.cursor_captured {
                    let dir = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => -y.signum() as i32,
                        winit::event::MouseScrollDelta::PixelDelta(p) => -(p.y.signum() as i32),
                    };
                    if dir != 0 {
                        match self.camera.mode {
                            MoveMode::Spectator => {
                                // Wheel up (away from you) is faster.
                                self.camera.adjust_fly_speed(-dir);
                                let speed = self.camera.fly_speed();
                                log::info!(
                                    "fly speed: {speed:.1} m/s (sprint {:.0})",
                                    speed * FlyCamera::SPRINT_MULTIPLIER
                                );
                            }
                            MoveMode::Survival => self.cycle_selection(dir),
                        }
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

                // Benchmark mode (`--bench`) flies the camera itself and ends
                // the program when its script is done.
                if self.bench_requested && self.bench.is_none() && self.renderer.is_some() {
                    self.start_bench();
                }
                if self.bench.is_some() && self.bench_tick(raw_dt) {
                    event_loop.exit();
                    return;
                }

                // Motion integrates in f64: an f32 frame time is exact enough,
                // an f32 POSITION is what loses the step far from the origin.
                if self.bench.is_none() {
                    match self.camera.mode {
                        MoveMode::Spectator => {
                            self.camera.update_spectator(&self.keys, f64::from(dt))
                        }
                        MoveMode::Survival => self.physics_update(f64::from(dt)),
                    }
                }

                // Autosave. Not while benchmarking: nothing is edited there,
                // and a write would land in a measured frame.
                if self.bench.is_none() {
                    self.autosave_accum += raw_dt;
                    if self.autosave_accum >= AUTOSAVE_SECS {
                        self.autosave_accum = 0.0;
                        self.session.save_unsaved("autosave");
                    }
                }

                // Camera's current chunk drives both streaming and the
                // floating-origin render origin.
                let origin_chunk = self.camera.block_pos().chunk();

                // Stream chunks in/out around the camera (borrows all of
                // self), before the render borrow below.
                let edited_arrivals = {
                    let renderer = &mut self.renderer;
                    self.session.tick(origin_chunk, &mut |pos, mesh| {
                        if let Some(renderer) = renderer.as_mut() {
                            renderer.set_chunk_mesh(pos, mesh);
                        }
                    })
                };
                // Chunks that came back from disk edited: the LOD nodes over
                // them were built from the seed (see `invalidate_lod_for_chunk`).
                for pos in edited_arrivals {
                    self.invalidate_lod_for_chunk(pos);
                }
                // Stream coarse LOD nodes for the distant horizon (M08).
                self.lod_tick(origin_chunk);

                // Raycast from the camera to find the targeted block (M03
                // task 3). Uses loaded world data; unloaded cells read as air
                // (get_block returns AIR for missing chunks), so you can only
                // target visible blocks. Computed before the renderer borrow.
                let hit = {
                    let world = &self.session.world;
                    let registry = &self.session.registry;
                    let eye = self.camera.position.to_array();
                    let dir = self.camera.forward().to_array();
                    // PHYSICAL (ADR-0011): only solid blocks can be targeted.
                    vox_core::raycast_voxels(eye, dir, REACH, |p| {
                        registry.is_solid(world.get_block(p))
                    })
                };
                self.targeted = hit;

                let loaded = self.session.streamer.loaded_count();
                // Where the player stands, for the telemetry line (M13) —
                // only on the frame that logs it.
                let place = if self.telemetry_accum + dt >= 1.0 {
                    self.place_readout()
                } else {
                    String::new()
                };
                let in_flight = self.session.gen_in_flight.len();
                let dirty = self.session.dirty.len();
                let relight = self.session.relight.len();

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
                    self.session.save_unsaved("exit");
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
                let fullres = self.session.fullres_columns(origin_chunk);
                let covered = self.covered_lod_nodes(&fullres);

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
                        density: self.settings.effective_atmosphere(),
                        // Above the sea surface (the top of the highest water
                        // block): the height the air thins from.
                        eye_height: (self.camera.position.y
                            - (vox_core::SEA_LEVEL_BLOCKS + 1) as f64)
                            as f32,
                    };
                    let cam_rel = self.camera.render_relative(render_origin);
                    let shape = vox_render::ShapeParams {
                        morph_band: self.settings.lod_morph_band,
                        // The shaders' 1/2R, or 0 for a flat world (ADR-0015).
                        curvature: if self.settings.earth_curvature {
                            vox_core::curvature_coefficient() as f32
                        } else {
                            0.0
                        },
                    };
                    renderer.set_sky(
                        self.world_time,
                        view_proj.inverse().to_cols_array_2d(),
                        [cam_rel.x, cam_rel.y, cam_rel.z],
                        fog,
                        self.settings.star_intensity,
                        shape,
                    );

                    let ui_frame = ui_output.as_ref().map(|o| vox_render::UiFrame {
                        primitives: &o.primitives,
                        textures_delta_set: &o.textures_set,
                        textures_delta_free: o.textures_free.clone(),
                        pixels_per_point: o.pixels_per_point,
                    });
                    renderer.set_suppressed_lod(covered);
                    renderer
                        .set_fullres_columns(origin_chunk, |cx, cz| fullres.contains(&(cx, cz)));
                    renderer.render(view_proj.to_cols_array_2d(), ui_frame);
                    if std::mem::take(&mut self.census_requested) {
                        for (name, holes, first) in
                            renderer.hole_census(view_proj.to_cols_array_2d())
                        {
                            log::info!("HOLES {name}: {holes} {first:?}");
                        }
                    }

                    // Telemetry once per second: FPS, frustum-culling ratio,
                    // and streaming state (resident chunks, gen queue, dirty).
                    self.telemetry_accum += dt;
                    self.telemetry_frames += 1;
                    if self.telemetry_accum >= 1.0 {
                        let fps = self.telemetry_frames as f32 / self.telemetry_accum;
                        let lod_backlog = self.lod_pending_set.len() + self.lod_in_flight.len();
                        log::info!(
                            "{:.0} fps (worst {:.1}ms) | drawn {}/{} | {:.2}M tris | {:.0}MB gpu | lod {}+{} | loaded {} | hmap {}k | gen {} | dirty {} | relight {} | lt {:.0}ms msh {:.0}ms | {}",
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
                            self.session.column_heights.len() / 1000,
                            in_flight,
                            dirty,
                            relight,
                            self.session.relight_ms_accum,
                            self.session.mesh_ms_accum,
                            place,
                        );
                        self.telemetry_accum = 0.0;
                        self.telemetry_frames = 0;
                        self.session.relight_ms_accum = 0.0;
                        self.session.mesh_ms_accum = 0.0;
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

    // `cargo run -p vox-app -- --bench`: the fixed performance run (bench.rs).
    let mut app = App::new(std::env::args().any(|a| a == "--bench"));
    event_loop.run_app(&mut app).expect("event loop error");
}

#[cfg(test)]
mod specks_scratch {
    use super::*;
    use vox_core::{LocalPos, World};
    use vox_mesh::{mesh_chunk, ChunkNeighbors};

    /// The benchmark's water starts on the default world, printed: run with
    /// `--ignored --nocapture` to see where the rivers and waterfall phases
    /// stand (and feed them to `specks` to look).
    #[test]
    #[ignore]
    fn bench_water_starts_on_the_default_world() {
        let g = vox_worldgen::Generator::new(0x7e22ac0de, WorldShape::default());
        let (sx, sz) = (-19712, 20480);
        let path = (bench::SPRINT_SPEED * bench::SPRINT_SECS) as i64;
        let (rivers, fall) = bench_water_starts(&g, sx, sz, path, 40.0);
        println!("rivers {rivers:?}\nwaterfall {fall:?}");
        assert!(rivers.is_some() && fall.is_some());
    }

    /// Scratch: render the LOD ring headlessly with a magenta sky and count
    /// isolated magenta pixels (holes). Env: SPECK_EXTRA, SPECK_X, SPECK_Y
    /// (or SPECK_UP above ground), SPECK_Z, SPECK_YAW, SPECK_PITCH, SPECK_W,
    /// SPECK_H, SPECK_FOV, SPECK_MORPH, SPECK_CURVE, SPECK_OUT — the P key's
    /// VIEW line has them all — and SPECK_FRAMES (time that many frames),
    /// SPECK_NOCLIMATE (no biome colour), SPECK_LIT (daylight the
    /// full-resolution chunks).
    #[test]
    #[ignore]
    fn specks() {
        let env = |k: &str, d: f64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        let (w, h) = (env("SPECK_W", 1280.0) as u32, env("SPECK_H", 720.0) as u32);
        let shape = WorldShape::default();
        let generator = vox_worldgen::Generator::new(0x7e22ac0de, shape);
        let registry = vox_core::BlockRegistry::default_set();
        let mut settings = vox_core::Settings::default();
        settings.fov_degrees = env("SPECK_FOV", settings.fov_degrees as f64) as f32;
        settings.lod_morph_band = env("SPECK_MORPH", settings.lod_morph_band as f64) as f32;
        let extra = env("SPECK_EXTRA", 3.0) as u32;
        let levels = vox_core::LodLevel::extended(&LOD_LEVELS, extra, LOD_INNER_CHUNKS);
        let far_blocks = (levels.last().unwrap().outer_chunks * CHUNK_SIZE_I) as f32;
        let band = (vox_core::WORLD_Y_MIN_BLOCKS, vox_core::WORLD_Y_MAX_BLOCKS);
        let mut ring = LodRing::new(LOD_INNER_CHUNKS, &levels, LOD_UNLOAD_MARGIN_CHUNKS, band);

        let (x, z) = (env("SPECK_X", -19711.5), env("SPECK_Z", 20480.5));
        let ground = generator.surface_height(x as i64, z as i64) as f64;
        let y = std::env::var("SPECK_Y").ok().and_then(|v| v.parse().ok());
        let pos = DVec3::new(x, y.unwrap_or(ground + env("SPECK_UP", 40.0)), z);
        let cam_chunk = ChunkPos::new(
            (pos.x.floor() as i64) >> 5,
            (pos.y.floor() as i64) >> 5,
            (pos.z.floor() as i64) >> 5,
        );
        let update = ring.update(cam_chunk);

        let mut r = vox_render::Renderer::new_headless(w, h);
        r.set_render_origin(cam_chunk);
        if std::env::var("SPECK_NOCLIMATE").is_err() {
            let (texels, cells) = climate_texels(&generator);
            r.set_climate(
                &texels,
                cells,
                (shape.size_x(), shape.size_z()),
                vox_worldgen::climate::LAPSE_PER_BLOCK,
            );
            let (flow, cells) = flow_texels(&generator);
            r.set_flow(&flow, cells);
            // SPECK_WATER_T: the water clock, seconds (a still frame of it).
            r.set_water_time(Some(env("SPECK_WATER_T", 0.0) as f32));
        }
        r.set_debug_holes(std::env::var("SPECK_NOH").is_err());
        let unload = 8 + 2; // default radius 8 + streamer slack, roughly
        use rayon::prelude::*;
        let meshes: Vec<_> = update
            .to_load
            .par_iter()
            .map(|n| {
                let stride = ring.stride(n.level);
                let (bx, by, bz) = ring.node_origin_blocks(*n);
                let reach = unload + ring.inner_snap_chunks(n.level);
                let sampling = if ring.closest_approach_chunks(n.level) <= reach {
                    LodSampling::Exact
                } else {
                    LodSampling::Sparse
                };
                let (heights, tops, water) = generator.lod_surface(bx, bz, stride, sampling);
                let mesh = vox_mesh::mesh_lod_cells(
                    vox_mesh::LodCells {
                        heights: &heights,
                        tops: &tops,
                        water: &water,
                    },
                    stride as i32,
                    by as i32,
                    |b, f| registry.face_layer(b, f),
                    LOD_SKIRT_DEPTH_CELLS * stride as i32,
                    true,
                );
                (*n, mesh)
            })
            .collect();
        for (n, mesh) in &meshes {
            let span = ring.span_blocks(n.level) as f32;
            let origin = App::lod_node_origin_chunk(&ring, *n);
            let morph_end = ring.morph_end_blocks(n.level).map_or(0.0, |b| b as f32);
            r.set_lod_mesh(origin, n.level, span, morph_end, mesh);
        }

        let far = (far_blocks * 1.6).max(1000.0);
        let render_origin = cam_chunk.origin();
        let rel = (pos
            - DVec3::new(
                render_origin.x as f64,
                render_origin.y as f64,
                render_origin.z as f64,
            ))
        .as_vec3();
        let (yaw, pitch) = (env("SPECK_YAW", 0.0), env("SPECK_PITCH", -0.12));
        let fwd = DVec3::new(
            pitch.cos() * yaw.cos(),
            pitch.sin(),
            pitch.cos() * yaw.sin(),
        )
        .as_vec3();
        let proj = vox_render::perspective(
            settings.fov_degrees.to_radians(),
            w as f32 / h as f32,
            0.1,
            far,
        );
        let view_proj = proj * Mat4::look_to_rh(rel, fwd, Vec3::Y);
        let time = vox_core::WorldTime::from_ticks((0.5 * vox_core::TICKS_PER_DAY as f64) as u64);
        let (fog_start, fog_end) = settings.fog_range(far / 1.6);
        let fog = vox_render::FogParams {
            color: [0.68, 0.82, 0.95],
            strength: settings.effective_fog_strength(),
            start: fog_start,
            end: fog_end,
            density: settings.effective_atmosphere(),
            eye_height: (pos.y - (vox_core::SEA_LEVEL_BLOCKS + 1) as f64) as f32,
        };
        let shape_p = vox_render::ShapeParams {
            morph_band: settings.lod_morph_band,
            curvature: if env("SPECK_CURVE", 1.0) > 0.0 {
                vox_core::curvature_coefficient() as f32
            } else {
                0.0
            },
        };
        r.set_sky(
            time,
            view_proj.inverse().to_cols_array_2d(),
            [rel.x, rel.y, rel.z],
            fog,
            1.0,
            shape_p,
        );
        // SPECK_FULLRES=1: full resolution too, unlit, radius 8, like the
        // game's disc, with the mask over the columns meshed.
        let mut fullres: HashSet<(i64, i64)> = HashSet::new();
        if std::env::var("SPECK_FULLRES").is_ok() {
            let mut world = World::new();
            let r8 = 8i64;
            let mut targets = Vec::new();
            for dz in -r8 - 1..=r8 + 1 {
                for dx in -r8 - 1..=r8 + 1 {
                    let (cx, cz) = (cam_chunk.x + dx, cam_chunk.z + dz);
                    // The game's span (ground and standing water, M14).
                    let (lo, hi) = generator.surface_span_chunks(cx, cz);
                    for cy in lo - 3..=hi + 3 {
                        let pos = ChunkPos::new(cx, cy, cz);
                        let mut chunk = generator.generate_chunk(pos);
                        // SPECK_LIT=1: full daylight in every non-opaque cell —
                        // crude next to real propagation (no shade under
                        // overhangs), but enough to judge colour across the
                        // full-res/LOD edge (M13). Unlit by default.
                        if std::env::var("SPECK_LIT").is_ok() {
                            for p in LocalPos::iter() {
                                if !registry.is_opaque(chunk.get(p)) {
                                    chunk.set_sky_light(p, 15);
                                }
                            }
                        }
                        world.insert_chunk(pos, chunk);
                        if dx * dx + dz * dz <= r8 * r8 && (lo - 2..=hi + 2).contains(&cy) {
                            targets.push(pos);
                        }
                    }
                    if dx * dx + dz * dz <= r8 * r8 {
                        fullres.insert((cx, cz));
                    }
                }
            }
            for pos in targets {
                let chunk = world.chunk(pos).unwrap();
                let n = ChunkNeighbors::of(&world, pos);
                let m = mesh_chunk(
                    chunk,
                    &n,
                    |b, f| registry.face_layer(b, f),
                    |b| registry.is_opaque(b),
                );
                if !m.is_empty() {
                    r.set_chunk_mesh(pos, &m);
                }
            }
        }
        r.set_fullres_columns(cam_chunk, |cx, cz| fullres.contains(&(cx, cz)));
        for (name, holes, first) in r.hole_census(view_proj.to_cols_array_2d()) {
            println!("HOLES {name}: {holes} {first:?}");
        }
        r.render(view_proj.to_cols_array_2d(), None);
        let px = r.read_frame();
        // SPECK_FRAMES=n: time n more frames, each rendered and read back so
        // the GPU's work is inside the measurement — on lavapipe, M12's
        // stand-in for weak graphics (CPU-only Vulkan).
        let frames = env("SPECK_FRAMES", 0.0) as usize;
        if frames > 0 {
            let (mut total, mut encode) = (Vec::new(), Vec::new());
            for _ in 0..frames {
                let t = std::time::Instant::now();
                r.render(view_proj.to_cols_array_2d(), None);
                let _ = r.read_frame();
                total.push(t.elapsed().as_secs_f64() * 1000.0);
                encode.push(f64::from(r.encode_ms_last_frame()));
            }
            total.sort_by(f64::total_cmp);
            encode.sort_by(f64::total_cmp);
            println!(
                "FRAMES {frames} at {w}x{h}: median {:.1} ms (encode {:.2} ms), {} draws, {} tris",
                total[frames / 2],
                encode[frames / 2],
                r.draws_last_frame(),
                r.tris_last_frame()
            );
        }

        let magenta = |i: usize| px[i * 4] > 240 && px[i * 4 + 1] < 20 && px[i * 4 + 2] > 240;
        let (wi, hi) = (w as usize, h as usize);
        let mut isolated = 0;
        for y in 1..hi - 1 {
            for x in 1..wi - 1 {
                let i = y * wi + x;
                if magenta(i) {
                    let n = [i - 1, i + 1, i - wi, i + wi]
                        .iter()
                        .filter(|&&j| magenta(j))
                        .count();
                    if n <= 1 {
                        isolated += 1;
                    }
                }
            }
        }
        println!(
            "nodes {} tris {} isolated magenta pixels: {isolated}",
            meshes.len(),
            r.tris_last_frame()
        );
        let out = std::env::var("SPECK_OUT").unwrap_or("/tmp/specks.rgba".into());
        std::fs::write(&out, &px).unwrap();
    }
}

#[cfg(test)]
mod fly_speed {
    use super::*;

    fn camera() -> FlyCamera {
        FlyCamera {
            position: DVec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            velocity: DVec3::ZERO,
            on_ground: false,
            mode: MoveMode::Spectator,
            fly_step: 0,
        }
    }

    /// The wheel starts at the speed the bench is tied to, four notches
    /// double or halve it, and it stops at ⅛× and 8×.
    #[test]
    fn the_wheel_scales_flight_speed_within_bounds() {
        let mut cam = camera();
        assert_eq!(cam.fly_speed(), FlyCamera::SPEED);
        cam.adjust_fly_speed(4);
        assert!((cam.fly_speed() - 2.0 * FlyCamera::SPEED).abs() < 1e-9);
        cam.adjust_fly_speed(100);
        assert!((cam.fly_speed() - 8.0 * FlyCamera::SPEED).abs() < 1e-9);
        cam.adjust_fly_speed(-100);
        assert!((cam.fly_speed() - FlyCamera::SPEED / 8.0).abs() < 1e-9);
    }

    /// Flying covers distance at the wheel's speed, sprint on top.
    #[test]
    fn spectator_moves_at_the_wheel_speed() {
        let mut cam = camera();
        cam.adjust_fly_speed(4);
        let mut keys = HashSet::new();
        keys.insert(KeyCode::KeyW);
        cam.update_spectator(&keys, 1.0);
        assert!((cam.position.x - 60.0).abs() < 1e-9);
        keys.insert(KeyCode::ControlLeft);
        cam.update_spectator(&keys, 1.0);
        assert!((cam.position.x - 300.0).abs() < 1e-9);
    }
}
