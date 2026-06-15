//! vox-render: the wgpu renderer.
//!
//! Milestone 00, tasks 3+4+6: GPU connection, depth-tested render pipeline
//! for chunk meshes, camera uniform, mesh upload, and the per-frame draw.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use vox_core::{CHUNK_SIZE, ChunkPos, WorldPos};
use vox_mesh::MeshData;
use wgpu::util::DeviceExt;
use winit::window::Window;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

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
            r3 + r2, // near
            r3 - r2, // far
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
    aabb_min: Vec3,
    aabb_max: Vec3,
    /// Uniform holding `offset.xyz = (chunk_world_origin - render_origin)`,
    /// rewritten when the render origin moves.
    offset_buffer: wgpu::Buffer,
    offset_bind_group: wgpu::BindGroup,
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

        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface not supported by adapter");
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
        // position [f32;3], uv [f32;2], layer u32, brightness f32. ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk pipeline layout"),
            bind_group_layouts: &[&camera_bgl, &chunk_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<vox_mesh::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            // Matches vox_mesh::Vertex (texture array, ADR-0003):
            // position [f32;3], uv [f32;2], layer u32, brightness f32.
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3,
                1 => Float32x2,
                2 => Uint32,
                3 => Float32
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
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        // --- Targeted-block highlight pipeline (M03 task 3): a line-list
        // wireframe cube. Reuses camera (group 0) + an offset uniform
        // (chunk_bgl, group 1). No culling; drawn after chunks. ---
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
                depth_compare: wgpu::CompareFunction::LessEqual,
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
            highlight_pipeline,
            highlight_vertices,
            highlight_offset_buffer,
            highlight_bind_group,
            highlight_target: None,
        }
    }

    /// Upload (or replace) the mesh for one chunk. An empty mesh removes
    /// the chunk's entry entirely. Build once, draw many — call this when a
    /// chunk's geometry changes, NOT every frame. Meshes are in LOCAL chunk
    /// space (0..32); world placement happens via the per-chunk offset
    /// (floating origin, ADR-0002).
    pub fn set_chunk_mesh(&mut self, pos: ChunkPos, mesh: &MeshData) {
        if mesh.is_empty() {
            self.meshes.remove(&pos);
            return;
        }
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        // Render-relative offset of this chunk's min corner, plus its AABB
        // in the same (render-relative) space the frustum uses.
        let offset = chunk_offset(pos, self.render_origin);
        let aabb_min = offset;
        let aabb_max = offset + Vec3::splat(CHUNK_SIZE as f32);

        let offset_data = [offset.x, offset.y, offset.z, 0.0];
        let offset_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk offset uniform"),
                contents: bytemuck::cast_slice(&offset_data),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let offset_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chunk offset bind group"),
            layout: &self.chunk_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: offset_buffer.as_entire_binding(),
            }],
        });

        self.meshes.insert(
            pos,
            GpuMesh {
                vertex_buffer,
                index_buffer,
                index_count: mesh.indices.len() as u32,
                aabb_min,
                aabb_max,
                offset_buffer,
                offset_bind_group,
            },
        );
    }

    /// Chunks drawn in the most recent frame, after frustum culling.
    pub fn drawn_last_frame(&self) -> usize {
        self.drawn_last_frame
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
            let data = [offset.x, offset.y, offset.z, 0.0];
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
    pub fn render(&mut self, view_proj: [[f32; 4]; 4]) {
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

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.45,
                            g: 0.70,
                            b: 0.95,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if !self.meshes.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                // Group 2 (block textures) is shared by all chunks — bind once.
                pass.set_bind_group(2, &self.texture_bind_group, &[]);
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
