//! vox-render: the wgpu renderer.
//!
//! Milestone 00, tasks 3+4+6: GPU connection, depth-tested render pipeline
//! for chunk meshes, camera uniform, mesh upload, and the per-frame draw.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use vox_core::{CHUNK_SIZE, ChunkPos, WorldPos};
use vox_mesh::{LodMeshData, MeshData};
use wgpu::util::DeviceExt;
use winit::window::Window;

// ---------------------------------------------------------------------------
// Depth convention: REVERSED-Z (M10 A3).
//
// Near maps to depth 1, far to 0, the buffer clears to 0, and "nearer" is
// GREATER. With a float depth buffer this spends the float's precision where
// perspective needs it: a surface is resolved to ~0.0001 blocks at 2 km and
// ~0.0003 at 8 km. Standard-Z, where both sit bunched up against 1.0, resolved
// ~2.7 blocks at 2 km and ~20 at 4 km — enough that every LOD node's border
// skirt tied with its neighbour's top face and drew as a grid of lines across
// distant terrain. M11's horizon only makes the distances larger.
//
// Everything that encodes the convention lives here, so it cannot be half
// changed: the projection, the clear value, and the two compares. The sky
// pass and the frustum culler read depth too and are written for it.
// ---------------------------------------------------------------------------

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Depth the buffer clears to: the FAR end under reversed-Z.
const DEPTH_CLEAR: f32 = 0.0;

/// "Nearer than what is there" under reversed-Z.
const DEPTH_NEARER: wgpu::CompareFunction = wgpu::CompareFunction::Greater;

/// "Nearer than, or exactly at, what is there" under reversed-Z.
const DEPTH_NEARER_OR_EQUAL: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;

/// The perspective projection for this renderer's depth convention.
///
/// Every view-projection the renderer is given MUST be built from this: a
/// standard projection against these compares draws the world inside out.
/// Right-handed, depth 0..1, with `near` mapped to 1 and `far` to 0 — glam's
/// `perspective_rh` with the planes swapped.
pub fn perspective(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    Mat4::perspective_rh(fov_y_radians, aspect, far, near)
}

/// A view frustum as six inward-facing planes, extracted from a
/// view-projection matrix (Gribb–Hartmann). Used to skip drawing chunks
/// the camera can't see.
struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    fn from_view_proj(vp: Mat4) -> Self {
        let r0 = vp.row(0);
        let r1 = vp.row(1);
        let r2 = vp.row(2);
        let r3 = vp.row(3);
        let raw = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            // Depth: wgpu's clip range is 0 <= z <= w (not OpenGL's -w..w,
            // which the old `r3 + r2` assumed and which merely culled too
            // little). Under reversed-Z `z <= w` is the near plane and
            // `z >= 0` the far one; the pair bounds the frustum either way.
            r2,      // z >= 0
            r3 - r2, // z <= w
        ];
        let mut planes = [Vec4::ZERO; 6];
        for (i, p) in raw.iter().enumerate() {
            let len = Vec3::new(p.x, p.y, p.z).length();
            planes[i] = if len > 0.0 { *p / len } else { *p };
        }
        Self { planes }
    }

    /// Conservative AABB test: returns false only when the box is wholly
    /// outside the frustum (so no visible chunk is ever wrongly culled).
    fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for plane in &self.planes {
            let n = Vec3::new(plane.x, plane.y, plane.z);
            let positive_vertex = Vec3::new(
                if n.x >= 0.0 { max.x } else { min.x },
                if n.y >= 0.0 { max.y } else { min.y },
                if n.z >= 0.0 { max.z } else { min.z },
            );
            if n.dot(positive_vertex) + plane.w < 0.0 {
                return false;
            }
        }
        true
    }
}

/// A mesh that has been uploaded to GPU buffers, with its world-space
/// bounding box for frustum culling and its per-chunk offset uniform for
/// floating-origin rendering (ADR-0002).
struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    /// Vertex + index bytes on the GPU, tracked so the renderer can report
    /// total buffer memory without walking every mesh each frame.
    bytes: u64,
    aabb_min: Vec3,
    aabb_max: Vec3,
    /// The AABB's offset from the mesh's own origin. Zero for chunks; for LOD
    /// nodes it is the mesh's `y_min`, so a render-origin move can reposition
    /// the box without flattening it back onto the node origin.
    aabb_offset: Vec3,
    /// Uniform holding `offset.xyz = (chunk_world_origin - render_origin)`,
    /// rewritten when the render origin moves.
    offset_buffer: wgpu::Buffer,
    offset_bind_group: wgpu::BindGroup,
    /// The uniform's `.w`, kept so a render-origin move can rewrite `.xyz`
    /// WITHOUT destroying it. LOD nodes carry their geomorph completion
    /// distance there (ADR-0009); full-res chunks carry 0.
    offset_w: f32,
}

/// Sky-pass uniform (M07 task 3b, ADR-0007). Must match `SkyUniform` in
/// sky.wgsl exactly (std140: mat4 then three vec4s). `inv_view_proj`
/// reconstructs per-pixel world ray directions; `sun`/`moon` carry direction +
/// (sky_scale / illumination) in `.w`; `params.x` is the star-intensity knob.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniformData {
    inv_view_proj: [[f32; 4]; 4],
    sun: [f32; 4],
    moon: [f32; 4],
    params: [f32; 4],
}

/// One frame of tessellated UI, handed to [`Renderer::render`].
///
/// The app owns the egui context (it needs winit events), so it runs egui and
/// passes the result here rather than the renderer driving the UI. This keeps
/// `vox-render` free of window-event concerns.
pub struct UiFrame<'a> {
    pub primitives: &'a [egui::ClippedPrimitive],
    pub textures_delta_set: &'a [(egui::TextureId, egui::epaint::ImageDelta)],
    pub textures_delta_free: Vec<egui::TextureId>,
    pub pixels_per_point: f32,
}

/// How terrain fades into the distance (M09 amendment A2).
///
/// Fog is what makes a LOD transition unreadable: contrast drops before the
/// change in detail becomes visible, so the eye never finds the seam. Colour
/// should track the sky near the horizon (and dim with it at night) so terrain
/// dissolves into the sky rather than into a grey band.
#[derive(Debug, Clone, Copy)]
pub struct FogParams {
    /// Linear RGB the terrain fades toward.
    pub color: [f32; 3],
    /// 0 disables fog; 1 is full strength at `end`.
    pub strength: f32,
    /// Distance (blocks) where fog begins.
    pub start: f32,
    /// Distance (blocks) where fog reaches full strength.
    pub end: f32,
}

/// Geomorph inputs for one frame (M09 amendment A4, ADR-0009).
///
/// Distant LOD lerps toward the next coarser level's silhouette as the camera
/// nears a ring boundary, so the handover swaps geometry for geometry that
/// already matches. Per-node data (where each level's morph completes) rides in
/// that node's offset uniform; these two values are global to the frame.
#[derive(Debug, Clone, Copy)]
pub struct MorphParams {
    /// Width in blocks of the band before each boundary over which the morph
    /// runs. 0 disables morphing.
    ///
    /// Distance is measured from the CAMERA (`cam_scale.xyz`, already in this
    /// uniform for fog) rather than the ring's snapped centre, because the
    /// snapped centre is frozen between ring updates — see lod.wgsl.
    pub band: f32,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    /// Layout for each chunk's per-draw offset uniform (group 1).
    chunk_bgl: wgpu::BindGroupLayout,
    /// Block texture array + sampler bind group (group 2), bound once per
    /// frame (shared by all chunks).
    texture_bind_group: wgpu::BindGroup,
    /// One GPU mesh per chunk, keyed by chunk position. Empty/air chunks
    /// have no entry and cost nothing to "draw".
    meshes: HashMap<ChunkPos, GpuMesh>,
    /// The chunk position all rendering is currently relative to
    /// (ADR-0002). Updated when the camera crosses into a new chunk.
    render_origin: ChunkPos,
    /// Chunks drawn in the most recent frame (after frustum culling).
    drawn_last_frame: usize,
    /// Triangles submitted last frame (full-res + LOD), for telemetry: mesh
    /// count alone hides how much geometry the GPU is actually chewing.
    tris_last_frame: usize,
    /// Running total of vertex+index buffer bytes for all resident meshes
    /// (chunk + LOD) — the practical ceiling on render distance.
    buffer_bytes: u64,

    // --- Targeted-block highlight (M03 task 3) ---
    /// Line-list pipeline for the wireframe cube outline.
    highlight_pipeline: wgpu::RenderPipeline,
    /// 24 line vertices (12 cube edges) in local 0..1 space, uploaded once.
    highlight_vertices: wgpu::Buffer,
    /// Per-draw offset uniform (reuses `chunk_bgl`) placing the outline on
    /// the targeted voxel under floating origin.
    highlight_offset_buffer: wgpu::Buffer,
    highlight_bind_group: wgpu::BindGroup,
    /// The currently targeted block in world coords, or `None`.
    highlight_target: Option<WorldPos>,

    // --- Day/night (M07 task 3, ADR-0007) ---
    /// Sky uniform (group 3): currently a single `sky_scale` in `.x`, padded to
    /// 16 bytes. The fragment shader multiplies the SKY light channel by it so
    /// night dims sky-lit surfaces without touching block light. Room in .yzw
    /// for the sky-pass additions (sun direction, moon) in task 3b.
    sky_buffer: wgpu::Buffer,
    sky_bind_group: wgpu::BindGroup,
    /// Most recent sky_scale, also used to dim the background clear color until
    /// the procedural sky pass replaces it (task 3b).
    sky_scale: f32,

    // --- Procedural sky pass (M07 task 3b) ---
    /// Fullscreen sky pipeline (gradient + sun + moon + stars), drawn first.
    sky_pipeline: wgpu::RenderPipeline,
    sky_pass_buffer: wgpu::Buffer,
    sky_pass_bind_group: wgpu::BindGroup,

    // --- UI overlay (M09 amendment A3) ---
    /// egui's wgpu backend. The settings menu draws last, over everything.
    egui_renderer: egui_wgpu::Renderer,

    // --- LOD (M08) ---
    /// LOD nodes the app has asked us not to draw this frame: their ground is
    /// fully covered by resident full-resolution chunks.
    ///
    /// LOD underlaps the full-res region, so without this ANY hole the player
    /// digs shows coarse terrain behind it — the neighbouring coarse cells'
    /// walls, visible through the gap. Suppressing covered nodes is what makes
    /// a dug hole show sky/cave instead of a "ghost block".
    suppressed_lod: HashSet<(ChunkPos, u32)>,

    /// Coarse LOD node meshes, keyed by (origin chunk, level). The level is
    /// part of the key because different levels can share an origin chunk
    /// (their node grids nest), and both may be resident briefly during
    /// handover. Drawn with the depth-biased `lod_pipeline` so full-res
    /// occludes them where they overlap.
    lod_meshes: HashMap<(ChunkPos, u32), GpuMesh>,
    lod_pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no suitable GPU adapter found");

        log::info!("GPU: {:?}", adapter.get_info().name);

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("main device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .expect("failed to acquire device");

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface not supported by adapter");

        // Present mode: UNCAPPED by default (Immediate → no vsync), so the
        // frame rate shows the engine's real ceiling instead of being pinned to
        // the display. Trade-offs are tearing and a GPU running past what the
        // monitor shows — neither harmful. Set VOXTERRA_VSYNC=1 to force Fifo.
        // Falls back Immediate → Mailbox → Fifo depending on adapter support.
        if std::env::var("VOXTERRA_VSYNC").is_ok_and(|v| v != "0") {
            log::info!("VOXTERRA_VSYNC set: present mode Fifo (vsync on)");
        } else {
            let caps = surface.get_capabilities(&adapter);
            match [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox]
                .into_iter()
                .find(|m| caps.present_modes.contains(m))
            {
                Some(mode) => {
                    log::info!("present mode {mode:?} (uncapped; VOXTERRA_VSYNC=1 to cap)");
                    config.present_mode = mode;
                }
                None => log::warn!("adapter supports only Fifo (vsync); frame rate is capped"),
            }
        }
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);

        // --- Camera uniform: one mat4, rewritten every frame. ---
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<[[f32; 4]; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        // --- Sky / day-night / fog / morph uniform (group 3, M07 task 3).
        // Starts zeroed; the first `set_sky` fills it before anything draws.
        //
        // VERTEX_FRAGMENT, not FRAGMENT: lighting and fog read it per-fragment,
        // but geomorph reads the snapped LOD centre and band width in lod.wgsl's
        // VERTEX stage to place each vertex (ADR-0009). A stage missing from
        // these flags is a pipeline-creation panic, not a compile error. ---
        let sky_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky uniform"),
            // 4 vec4s: camera+sky_scale, fog colour, fog range + snapped LOD
            // centre, morph params (ADR-0009). Must match `SkyChunk` in both
            // shader.wgsl and lod.wgsl.
            size: std::mem::size_of::<[f32; 16]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sky_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky bind group"),
            layout: &sky_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sky_buffer.as_entire_binding(),
            }],
        });

        // --- Per-chunk offset uniform layout (group 1), for floating
        // origin. One small uniform per chunk, rebound per draw. ---
        let chunk_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk offset bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // --- Block texture array (group 2), ADR-0003. One layer per tile;
        // Repeat sampler tiles a layer across greedy-merged faces, nearest
        // filtering keeps voxel pixels crisp. ---
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("block texture bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let texture_bind_group = create_block_texture(&device, &queue, &texture_bgl);

        // --- Pipeline. Vertex layout must match vox_mesh::Vertex exactly:
        // position [f32;3], uv [f32;2], layer u32, sky f32, block f32,
        // shade f32 (M07/ADR-0007). ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk pipeline layout"),
            bind_group_layouts: &[&camera_bgl, &chunk_bgl, &texture_bgl, &sky_bgl],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<vox_mesh::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            // Matches vox_mesh::Vertex: position [f32;3], uv [f32;2],
            // layer u32, then the M07 light channels sky f32, block f32,
            // shade f32 (ADR-0007). The shader combines sky/block with the
            // per-frame sky_scale, applies the light curve, then × shade.
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3,
                1 => Float32x2,
                2 => Uint32,
                3 => Float32,
                4 => Float32,
                5 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chunk pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[vertex_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: DEPTH_NEARER,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        // --- LOD pipeline (M08): identical to the chunk pipeline but with a
        // small depth bias that pushes coarse LOD terrain slightly back, so
        // where a near LOD ring overlaps full-res chunks the full-res surface
        // wins the depth test (no z-fighting, no double terrain). LOD node
        // meshes carry the same vertex format and use the same bind groups. ---
        // Coarse LOD gets its OWN shader (ADR-0009): its vertices carry a
        // geomorph target where full-res carries block light, and it is
        // skylight-only by design. Same bind groups, same uniforms.
        let lod_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lod shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lod.wgsl").into()),
        });
        let lod_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lod pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &lod_shader,
                entry_point: Some("vs_lod"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<vox_mesh::LodVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    // Matches vox_mesh::LodVertex: position, uv, layer, sky,
                    // morph_y, shade. Slot 4 is the MORPH TARGET here, where
                    // the full-res layout has block light.
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32x2, 2 => Uint32,
                        3 => Float32, 4 => Float32, 5 => Float32
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &lod_shader,
                entry_point: Some("fs_lod"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: DEPTH_NEARER,
                stencil: Default::default(),
                // Push LOD back so full-res occludes it in the overlap band.
                //
                // NEGATIVE under reversed-Z: back is toward 0. For a float
                // buffer the constant term scales with the depth's own
                // exponent, so 16 units is a push of ~2e-6 of the distance —
                // 0.0005 blocks at the full-res edge. That is all it has to
                // be: LOD never rises above real terrain, so the only contest
                // is an exact tie where the two surfaces coincide, and 2e-6 is
                // ~30x the float noise of computing the same point twice.
                // (Under standard-Z the same 16 pushed ~0.6 blocks at 256.)
                bias: wgpu::DepthBiasState {
                    constant: -16,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let highlight_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("highlight shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("highlight.wgsl").into()),
        });
        let highlight_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("highlight pipeline layout"),
            bind_group_layouts: &[&camera_bgl, &chunk_bgl],
            push_constant_ranges: &[],
        });
        let highlight_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (3 * std::mem::size_of::<f32>()) as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        };
        let highlight_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("highlight pipeline"),
            layout: Some(&highlight_layout),
            vertex: wgpu::VertexState {
                module: &highlight_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[highlight_vertex_layout],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: DEPTH_NEARER_OR_EQUAL,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            fragment: Some(wgpu::FragmentState {
                module: &highlight_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let highlight_vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("highlight vertices"),
            contents: bytemuck::cast_slice(&cube_edge_vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let highlight_offset_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("highlight offset"),
                contents: bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let highlight_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight offset bind group"),
            layout: &chunk_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: highlight_offset_buffer.as_entire_binding(),
            }],
        });

        // --- Procedural sky pass (M07 task 3b). Fullscreen triangle (no vertex
        // buffer); its own uniform (inv view-proj + sun/moon + star knob). Drawn
        // first, writes no depth, always passes depth so terrain overdraws it. ---
        let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky.wgsl").into()),
        });
        let sky_pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky pass uniform"),
            size: std::mem::size_of::<SkyUniformData>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_pass_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky pass bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sky_pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky pass bind group"),
            layout: &sky_pass_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sky_pass_buffer.as_entire_binding(),
            }],
        });
        let sky_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky pipeline layout"),
            bind_group_layouts: &[&sky_pass_bgl],
            push_constant_ranges: &[],
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky pipeline"),
            layout: Some(&sky_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &sky_shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[], // fullscreen triangle generated from vertex_index
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // No depth write; always pass. Drawn before terrain, which then
            // overdraws it wherever geometry exists.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            fragment: Some(wgpu::FragmentState {
                module: &sky_shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        // Build the UI renderer BEFORE the struct literal: `device` and
        // `config` are moved into Self there, so borrowing them inside it
        // would be a use-after-move. No depth attachment — the settings menu
        // draws over the finished frame.
        let egui_renderer = egui_wgpu::Renderer::new(&device, config.format, None, 1, false);
        Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            pipeline,
            camera_buffer,
            camera_bind_group,
            chunk_bgl,
            texture_bind_group,
            meshes: HashMap::new(),
            render_origin: ChunkPos::new(0, 0, 0),
            drawn_last_frame: 0,
            tris_last_frame: 0,
            buffer_bytes: 0,
            highlight_pipeline,
            highlight_vertices,
            highlight_offset_buffer,
            highlight_bind_group,
            highlight_target: None,
            sky_buffer,
            sky_bind_group,
            sky_scale: 1.0,
            sky_pipeline,
            sky_pass_buffer,
            sky_pass_bind_group,
            lod_meshes: HashMap::new(),
            suppressed_lod: HashSet::new(),
            lod_pipeline,
            egui_renderer,
        }
    }

    /// Upload (or replace) the mesh for one chunk. An empty mesh removes
    /// the chunk's entry entirely. Build once, draw many — call this when a
    /// chunk's geometry changes, NOT every frame. Meshes are in LOCAL chunk
    /// space (0..32); world placement happens via the per-chunk offset
    /// (floating origin, ADR-0002).
    pub fn set_chunk_mesh(&mut self, pos: ChunkPos, mesh: &MeshData) {
        if mesh.is_empty() {
            if let Some(old) = self.meshes.remove(&pos) {
                self.buffer_bytes -= old.bytes;
            }
            return;
        }
        let offset = chunk_offset(pos, self.render_origin);
        let gpu = self.build_gpu_mesh(
            offset,
            Vec3::splat(CHUNK_SIZE as f32),
            0.0,
            &mesh.vertices,
            &mesh.indices,
        );
        self.buffer_bytes += gpu.bytes;
        if let Some(old) = self.meshes.insert(pos, gpu) {
            self.buffer_bytes -= old.bytes;
        }
    }

    /// Build GPU buffers + offset uniform for a mesh whose vertices are in block
    /// units relative to `offset` (render-relative), with an AABB of the given
    /// `extent` in blocks. Shared by chunk and LOD uploads.
    ///
    /// The extent must be the geometry's TRUE bounds on all three axes. A LOD
    /// node is not a cube — it is `span` wide horizontally but spans the whole
    /// world Y band — and using the horizontal span for height makes the box
    /// far too short, so frustum culling drops nodes that are plainly on
    /// screen (visible as terrain vanishing below you when flying high).
    /// Upload one mesh. Generic over the vertex type so the full-resolution
    /// and LOD formats (`vox_mesh::Vertex` / `LodVertex`) share it — they
    /// differ only in what slot 4 means (ADR-0009).
    ///
    /// `extra` rides in the offset uniform's unused `.w`: the LOD path puts
    /// the node's geomorph completion distance there; full-res passes 0.
    fn build_gpu_mesh<V: bytemuck::Pod>(
        &self,
        offset: Vec3,
        extent: Vec3,
        extra: f32,
        vertices: &[V],
        indices: &[u32],
    ) -> GpuMesh {
        self.build_gpu_mesh_at(offset, Vec3::ZERO, extent, extra, vertices, indices)
    }

    /// As [`Self::build_gpu_mesh`], but with the culling AABB offset from the
    /// mesh's own origin — LOD nodes are wide and (relative to a 20 000-block
    /// world) short, and sit at whatever height their terrain does.
    fn build_gpu_mesh_at<V: bytemuck::Pod>(
        &self,
        offset: Vec3,
        aabb_offset: Vec3,
        extent: Vec3,
        extra: f32,
        vertices: &[V],
        indices: &[u32],
    ) -> GpuMesh {
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let offset_data = [offset.x, offset.y, offset.z, extra];
        let offset_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh offset uniform"),
                contents: bytemuck::cast_slice(&offset_data),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let offset_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh offset bind group"),
            layout: &self.chunk_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: offset_buffer.as_entire_binding(),
            }],
        });
        let bytes = (std::mem::size_of_val(vertices) + std::mem::size_of_val(indices)) as u64;
        GpuMesh {
            offset_w: extra,
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            bytes,
            aabb_min: offset + aabb_offset,
            aabb_max: offset + aabb_offset + extent,
            aabb_offset,
            offset_buffer,
            offset_bind_group,
        }
    }

    /// Upload (or replace) a coarse LOD node mesh (M08). `origin` is the node's
    /// origin chunk; `span_blocks` is the node's horizontal size per side and
    /// `height_blocks` its vertical extent (the world Y band, NOT the span)
    /// (`CHUNK_SIZE × stride`). The mesh's vertices are already scaled to block
    /// units (baked by `mesh_lod_heightfield`), so this reuses the chunk offset
    /// path unchanged.
    pub fn set_lod_mesh(
        &mut self,
        origin: ChunkPos,
        level: u32,
        span_blocks: f32,
        morph_end_blocks: f32,
        mesh: &LodMeshData,
    ) {
        let key = (origin, level);
        if mesh.is_empty() {
            if let Some(old) = self.lod_meshes.remove(&key) {
                self.buffer_bytes -= old.bytes;
            }
            return;
        }
        let offset = chunk_offset(origin, self.render_origin);
        // Vertical extent comes from the MESH, not the world's Y band. With
        // M10's ~20 000-block world a band-height AABB would span everything
        // and vertical frustum culling would stop rejecting anything.
        let gpu = self.build_gpu_mesh_at(
            offset,
            Vec3::new(0.0, mesh.y_min, 0.0),
            Vec3::new(span_blocks, mesh.y_max - mesh.y_min, span_blocks),
            morph_end_blocks,
            &mesh.vertices,
            &mesh.indices,
        );
        self.buffer_bytes += gpu.bytes;
        if let Some(old) = self.lod_meshes.insert(key, gpu) {
            self.buffer_bytes -= old.bytes;
        }
    }

    /// Set the LOD nodes to skip drawing (fully covered by full-res).
    pub fn set_suppressed_lod(&mut self, set: HashSet<(ChunkPos, u32)>) {
        self.suppressed_lod = set;
    }

    /// Remove a LOD node mesh.
    pub fn remove_lod_mesh(&mut self, origin: ChunkPos, level: u32) {
        if let Some(old) = self.lod_meshes.remove(&(origin, level)) {
            self.buffer_bytes -= old.bytes;
        }
    }

    /// Drop all LOD meshes (e.g. when toggling LOD off).
    pub fn clear_lod(&mut self) {
        for (_, m) in self.lod_meshes.drain() {
            self.buffer_bytes -= m.bytes;
        }
    }

    /// Number of LOD node meshes currently uploaded.
    pub fn lod_count(&self) -> usize {
        self.lod_meshes.len()
    }

    /// Chunks drawn in the most recent frame, after frustum culling.
    pub fn drawn_last_frame(&self) -> usize {
        self.drawn_last_frame
    }

    /// Triangles submitted last frame (full-res + LOD).
    pub fn tris_last_frame(&self) -> usize {
        self.tris_last_frame
    }

    /// Total GPU vertex+index buffer bytes across all resident meshes.
    pub fn buffer_bytes(&self) -> u64 {
        self.buffer_bytes
    }

    /// Move the render origin (ADR-0002) and recompute every chunk's offset
    /// uniform and render-relative AABB. Call when the camera crosses into a
    /// new chunk; cheap relative to how often that happens. No-op if the
    /// origin is unchanged.
    pub fn set_render_origin(&mut self, origin: ChunkPos) {
        if origin == self.render_origin {
            return;
        }
        self.render_origin = origin;
        for (&pos, mesh) in self.meshes.iter_mut() {
            let offset = chunk_offset(pos, origin);
            mesh.aabb_min = offset;
            mesh.aabb_max = offset + Vec3::splat(CHUNK_SIZE as f32);
            let data = [offset.x, offset.y, offset.z, mesh.offset_w];
            self.queue
                .write_buffer(&mesh.offset_buffer, 0, bytemuck::cast_slice(&data));
        }
        // LOD meshes: same reposition, but preserve each node's (larger) span
        // AND its vertical offset — the AABB hugs the node's terrain, which is
        // rarely at the node's own origin in a 20 000-block-tall world.
        //
        // `.w` MUST be carried through, not re-zeroed. It holds the node's
        // geomorph completion distance, and the render origin moves every time
        // the camera crosses a chunk — every 32 blocks. Zeroing it here silently
        // switched morphing off for the entire world a few steps into any walk,
        // which is exactly when morphing is the thing you would notice.
        for (&(pos, _level), mesh) in self.lod_meshes.iter_mut() {
            let extent = mesh.aabb_max - mesh.aabb_min;
            let offset = chunk_offset(pos, origin);
            mesh.aabb_min = offset + mesh.aabb_offset;
            mesh.aabb_max = mesh.aabb_min + extent;
            let data = [offset.x, offset.y, offset.z, mesh.offset_w];
            self.queue
                .write_buffer(&mesh.offset_buffer, 0, bytemuck::cast_slice(&data));
        }
    }

    /// The current render origin.
    pub fn render_origin(&self) -> ChunkPos {
        self.render_origin
    }

    /// Set (or clear) the targeted-block highlight. Updates the outline's
    /// floating-origin offset so it sits exactly on the targeted voxel.
    pub fn set_highlight(&mut self, target: Option<WorldPos>) {
        self.highlight_target = target;
        if let Some(pos) = target {
            let origin = self.render_origin.origin();
            let offset = [
                (pos.x - origin.x) as f32,
                (pos.y - origin.y) as f32,
                (pos.z - origin.z) as f32,
                0.0,
            ];
            self.queue.write_buffer(
                &self.highlight_offset_buffer,
                0,
                bytemuck::cast_slice(&offset),
            );
        }
    }

    /// Number of chunk meshes currently uploaded (for debug/telemetry).
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Update everything day/night for this frame from the world time and the
    /// camera's inverse view-projection (for the sky pass's per-pixel ray
    /// reconstruction). Writes both the chunk-shader sky uniform (sky_scale) and
    /// the sky-pass uniform (sun/moon directions, star knob), and stores
    /// sky_scale for the fallback clear color.
    pub fn set_sky(
        &mut self,
        time: vox_core::WorldTime,
        inv_view_proj: [[f32; 4]; 4],
        camera_rel: [f32; 3],
        fog: FogParams,
        star_intensity: f32,
        morph: MorphParams,
    ) {
        let sky_scale = time.sky_scale().clamp(0.0, 1.0);
        self.sky_scale = sky_scale;

        // Chunk-shader uniform (group 3), 4 vec4s:
        //   0: camera position (render-relative) + sky_scale
        //   1: fog colour rgb + strength
        //   2: fog start, fog end, reserved, reserved
        //   3: geomorph band width, reserved, reserved, reserved
        // The camera position is needed per-fragment to measure view distance
        // for fog; it is render-relative so it matches vertex positions under
        // the floating origin (ADR-0002). Geomorph reuses it in the VERTEX
        // stage (ADR-0009), which is why `sky_bgl` is VERTEX_FRAGMENT.
        //
        // The reserved slots are written as 0 here. If one ever gains a
        // meaning, check every writer of the whole uniform first — see the
        // "unused padding is a promise that expires" note in
        // docs/notes/clippy-lints.md.
        let chunk_uniform: [f32; 16] = [
            camera_rel[0],
            camera_rel[1],
            camera_rel[2],
            sky_scale,
            fog.color[0],
            fog.color[1],
            fog.color[2],
            fog.strength,
            fog.start,
            fog.end,
            0.0,
            0.0,
            morph.band,
            0.0,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.sky_buffer, 0, bytemuck::cast_slice(&chunk_uniform));

        // Sky-pass uniform.
        let sun = time.sun_direction();
        let moon = time.moon_direction();
        let sky_uniform = SkyUniformData {
            inv_view_proj,
            sun: [sun[0], sun[1], sun[2], sky_scale],
            moon: [moon[0], moon[1], moon[2], time.moon_illumination()],
            // params.x = star intensity knob, driven by the settings slider
            // (it was a hardcoded 1.0, so the slider did nothing); yzw reserved.
            params: [star_intensity, 0.0, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.sky_pass_buffer, 0, bytemuck::bytes_of(&sky_uniform));
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        // Depth buffer dimensions must always match the surface.
        self.depth_view = create_depth_view(&self.device, &self.config);
    }

    /// Width/height ratio, for the projection matrix.
    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// Render one frame with the given view-projection matrix
    /// (column-major, as produced by `glam::Mat4::to_cols_array_2d`).
    /// Draw a frame. `ui` carries the settings menu's tessellated output when
    /// the menu is open (M09 amendment A3); pass `None` to draw the world
    /// alone. The UI is drawn last, over the finished frame, with no depth
    /// attachment.
    pub fn render(&mut self, view_proj: [[f32; 4]; 4], ui: Option<UiFrame<'_>>) {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                log::error!("surface error: {e:?}");
                return;
            }
        };

        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&view_proj));

        // Build the view frustum once per frame for culling.
        let frustum = Frustum::from_view_proj(Mat4::from_cols_array_2d(&view_proj));
        let mut drawn = 0usize;
        let mut tris = 0usize;

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            // Daytime sky-blue, dimmed toward near-black by sky_scale so the
            // background tracks day/night until the procedural sky pass (task
            // 3b) replaces this flat clear with a real gradient + sun/moon.
            let s = self.sky_scale as f64;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.45 * s + 0.02 * (1.0 - s),
                            g: 0.70 * s + 0.02 * (1.0 - s),
                            b: 0.95 * s + 0.05 * (1.0 - s),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Procedural sky first (fullscreen). Writes no depth and always
            // passes, so terrain below overdraws it wherever geometry exists.
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.sky_pass_bind_group, &[]);
            pass.draw(0..3, 0..1);

            if !self.meshes.is_empty() || !self.lod_meshes.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                // Group 2 (block textures) is shared by all chunks — bind once.
                pass.set_bind_group(2, &self.texture_bind_group, &[]);
                // Group 3 (sky/day-night) — shared by all chunks, bind once.
                pass.set_bind_group(3, &self.sky_bind_group, &[]);
                // Chunk world offset is baked into vertex positions at mesh
                // time, so all chunks share one pipeline and bind group and
                // differ only by their vertex/index buffers.
                for mesh in self.meshes.values() {
                    // Frustum culling: skip chunks the camera can't see.
                    if !frustum.intersects_aabb(mesh.aabb_min, mesh.aabb_max) {
                        continue;
                    }
                    pass.set_bind_group(1, &mesh.offset_bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    drawn += 1;
                    tris += (mesh.index_count / 3) as usize;
                }

                // Coarse LOD nodes (M08): same camera/texture/sky bind groups,
                // but the depth-biased LOD pipeline so full-res occludes them
                // where they overlap. Drawn after full-res.
                if !self.lod_meshes.is_empty() {
                    pass.set_pipeline(&self.lod_pipeline);
                    for (key, mesh) in self.lod_meshes.iter() {
                        // Skip nodes whose ground full-res already covers.
                        if self.suppressed_lod.contains(key) {
                            continue;
                        }
                        if !frustum.intersects_aabb(mesh.aabb_min, mesh.aabb_max) {
                            continue;
                        }
                        pass.set_bind_group(1, &mesh.offset_bind_group, &[]);
                        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            mesh.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                        drawn += 1;
                        tris += (mesh.index_count / 3) as usize;
                    }
                    // Restore the full-res pipeline for the highlight pass below.
                    pass.set_pipeline(&self.pipeline);
                }

                // Targeted-block highlight (M03 task 3): wireframe cube on
                // the looked-at block, after chunks, sharing the depth pass.
                if self.highlight_target.is_some() {
                    pass.set_pipeline(&self.highlight_pipeline);
                    pass.set_bind_group(0, &self.camera_bind_group, &[]);
                    pass.set_bind_group(1, &self.highlight_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.highlight_vertices.slice(..));
                    pass.draw(0..24, 0..1);
                }
            }
        }

        self.drawn_last_frame = drawn;
        self.tris_last_frame = tris;

        // --- UI overlay (M09 amendment A3), last so it sits over the world ---
        if let Some(ui) = ui {
            let desc = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [self.config.width, self.config.height],
                pixels_per_point: ui.pixels_per_point,
            };
            for (id, delta) in ui.textures_delta_set {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
            self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                ui.primitives,
                &desc,
            );
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("ui pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                // Keep the rendered world; the UI blends on top.
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                self.egui_renderer.render(&mut pass, ui.primitives, &desc);
            }
            for id in ui.textures_delta_free {
                self.egui_renderer.free_texture(&id);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

/// Render-relative offset (in blocks) of a chunk's min corner from the
/// render origin. Computed in i64 and narrowed to f32 while small, so it
/// stays exact regardless of absolute distance (ADR-0002).
fn chunk_offset(pos: ChunkPos, render_origin: ChunkPos) -> Vec3 {
    let d = CHUNK_SIZE as i64;
    Vec3::new(
        ((pos.x - render_origin.x) * d) as f32,
        ((pos.y - render_origin.y) * d) as f32,
        ((pos.z - render_origin.z) * d) as f32,
    )
}

/// The 24 vertices (12 edges, 2 verts each) of a unit cube outline, slightly
/// inflated so the wireframe hugs the block faces without z-fighting. Local
/// space; the highlight offset uniform places it on the targeted voxel.
fn cube_edge_vertices() -> [[f32; 3]; 24] {
    const E: f32 = 0.002; // small inflation
    let lo = -E;
    let hi = 1.0 + E;
    // 8 corners.
    let c = [
        [lo, lo, lo],
        [hi, lo, lo],
        [hi, hi, lo],
        [lo, hi, lo],
        [lo, lo, hi],
        [hi, lo, hi],
        [hi, hi, hi],
        [lo, hi, hi],
    ];
    // 12 edges as corner-index pairs.
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0), // bottom face
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4), // top face
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7), // verticals
    ];
    let mut out = [[0.0f32; 3]; 24];
    let mut i = 0;
    for (a, b) in edges {
        out[i] = c[a];
        out[i + 1] = c[b];
        i += 2;
    }
    out
}

/// Build the block texture array (ADR-0003): one 16×16 RGBA layer per tile,
/// generated procedurally so the engine ships no image assets yet. Layer
/// indices match the block registry's assignment (L_STONE=0 .. L_PLANKS=6).
/// Real PNG tiles can replace `tile_pixels` later with no format change.
///
/// Returns the bind group (texture view + Repeat/nearest sampler) for group 2.
fn create_block_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::BindGroup {
    const TILE: u32 = 16;
    const LAYERS: u32 = 8; // = registry DEFAULT_LAYER_COUNT

    // Generate all layers back-to-back (the upload expects layers contiguous).
    let mut data = Vec::with_capacity((TILE * TILE * 4 * LAYERS) as usize);
    for layer in 0..LAYERS {
        data.extend_from_slice(&tile_pixels(layer, TILE));
    }

    let size = wgpu::Extent3d {
        width: TILE,
        height: TILE,
        depth_or_array_layers: LAYERS,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("block texture array"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * TILE),
            rows_per_image: Some(TILE),
        },
        size,
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("block sampler"),
        // Repeat so a layer tiles across greedy-merged faces (ADR-0003).
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        // Nearest for crisp voxel pixels.
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("block texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    })
}

/// Procedural RGBA pixels for one tile layer. A simple per-layer base color
/// plus a cheap deterministic per-texel variation so surfaces read as
/// textured rather than flat. Placeholder until real art drops in.
fn tile_pixels(layer: u32, tile: u32) -> Vec<u8> {
    // Base colors keyed to the registry's layer assignment.
    let base: [u8; 3] = match layer {
        0 => [128, 128, 134], // stone
        1 => [134, 96, 64],   // dirt
        2 => [80, 150, 64],   // grass top
        3 => [96, 132, 70],   // grass side (dirt-with-green-ish)
        4 => [206, 192, 138], // sand
        5 => [120, 120, 126], // cobblestone
        6 => [156, 116, 70],  // planks
        7 => [255, 236, 170], // lamp (warm, bright)
        _ => [255, 0, 255],   // magenta = missing
    };
    let mut px = Vec::with_capacity((tile * tile * 4) as usize);
    for y in 0..tile {
        for x in 0..tile {
            // Cheap hash-based dither, deterministic per texel.
            let h = (x
                .wrapping_mul(73)
                .wrapping_add(y.wrapping_mul(151))
                .wrapping_add(layer.wrapping_mul(977)))
                & 0x1F;
            let jitter = h as i32 - 16; // -16..15
            let shade = |c: u8| -> u8 { (c as i32 + jitter).clamp(0, 255) as u8 };
            px.push(shade(base[0]));
            px.push(shade(base[1]));
            px.push(shade(base[2]));
            px.push(255);
        }
    }
    px
}

fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOV: f32 = 1.2;
    const ASPECT: f32 = 16.0 / 9.0;
    const NEAR: f32 = 0.1;
    const FAR: f32 = 9000.0;

    /// Depth of a point `d` blocks straight ahead (camera at origin, looking
    /// down -Z, the right-handed convention).
    fn depth_at(d: f32) -> f32 {
        let c = perspective(FOV, ASPECT, NEAR, FAR) * Vec4::new(0.0, 0.0, -d, 1.0);
        c.z / c.w
    }

    #[test]
    fn projection_is_reversed_z() {
        assert!(
            (depth_at(NEAR) - 1.0).abs() < 1e-6,
            "near {}",
            depth_at(NEAR)
        );
        assert!(depth_at(FAR).abs() < 1e-6, "far {}", depth_at(FAR));
        // Nearer is GREATER, all the way out — what DEPTH_NEARER assumes.
        let mut previous = depth_at(NEAR);
        for d in [1.0, 10.0, 100.0, 1_000.0, 5_000.0, FAR] {
            let z = depth_at(d);
            assert!(z < previous, "depth not decreasing at {d}");
            previous = z;
        }
    }

    /// THE bug: standard-Z could not tell apart surfaces several blocks apart
    /// at the distances LOD draws, so skirts tied with the terrain beside them.
    /// Reversed-Z must separate a hundredth of a block at 8 km, and order it.
    #[test]
    fn distant_surfaces_are_resolved() {
        for d in [256.0f32, 1_000.0, 2_000.0, 4_000.0, 8_000.0] {
            let (a, b) = (depth_at(d), depth_at(d + 0.01));
            assert!(
                a > b,
                "at {d} blocks, 0.01 blocks of depth is lost ({a} vs {b})"
            );
        }
    }

    /// The same check against the old projection, to show what was lost —
    /// and that the test above would have caught it.
    #[test]
    fn standard_z_could_not_resolve_them() {
        let standard = Mat4::perspective_rh(FOV, ASPECT, NEAR, FAR);
        let z = |d: f32| {
            let c = standard * Vec4::new(0.0, 0.0, -d, 1.0);
            c.z / c.w
        };
        assert_eq!(
            z(4_000.0),
            z(4_001.0),
            "standard-Z separated 1 block at 4 km"
        );
    }

    fn frustum() -> Frustum {
        let view = Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        Frustum::from_view_proj(perspective(FOV, ASPECT, NEAR, FAR) * view)
    }

    fn cube(center: Vec3) -> (Vec3, Vec3) {
        (center - Vec3::splat(0.5), center + Vec3::splat(0.5))
    }

    #[test]
    fn frustum_keeps_what_is_in_front() {
        let f = frustum();
        for d in [1.0, 100.0, 5_000.0, FAR - 10.0] {
            let (lo, hi) = cube(Vec3::new(0.0, 0.0, -d));
            assert!(f.intersects_aabb(lo, hi), "culled a box {d} ahead");
        }
    }

    /// Both depth planes bound the frustum. The old extraction used OpenGL's
    /// -w..w range, so its "near" plane sat behind the camera and culled too
    /// little; under reversed-Z it would also have lost the far plane.
    #[test]
    fn frustum_culls_behind_and_beyond() {
        let f = frustum();
        let (lo, hi) = cube(Vec3::new(0.0, 0.0, 5.0));
        assert!(!f.intersects_aabb(lo, hi), "kept a box behind the camera");
        let (lo, hi) = cube(Vec3::new(0.0, 0.0, -(FAR + 100.0)));
        assert!(
            !f.intersects_aabb(lo, hi),
            "kept a box beyond the far plane"
        );
        let (lo, hi) = cube(Vec3::new(5_000.0, 0.0, -10.0));
        assert!(!f.intersects_aabb(lo, hi), "kept a box far off to the side");
    }
}
