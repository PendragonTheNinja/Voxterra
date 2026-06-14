// Voxterra chunk shader, Milestone 03 (texture array, ADR-0003).
//
// Vertices arrive in LOCAL chunk space (0..32) with texture coordinates
// `uv` (running 0..W / 0..H across a greedy-merged quad), a texture-array
// `layer` index per face, and a directional `brightness` scalar. Each chunk
// supplies an `offset` = (chunk_world_origin - render_origin), narrowed to
// f32 while small for floating-origin precision (ADR-0002).
//
// The fragment samples the block texture array at (uv, layer) with a Repeat
// sampler (so the layer tiles across merged faces) and nearest filtering
// (crisp voxel pixels), then multiplies by brightness for face shading.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct ChunkData {
    offset: vec4<f32>, // .w unused padding (16-byte align)
};

@group(1) @binding(0)
var<uniform> chunk: ChunkData;

@group(2) @binding(0)
var block_tex: texture_2d_array<f32>;
@group(2) @binding(1)
var block_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    @location(3) brightness: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) brightness: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world_rel = in.position + chunk.offset.xyz;
    out.clip_position = camera.view_proj * vec4<f32>(world_rel, 1.0);
    out.uv = in.uv;
    out.layer = in.layer;
    out.brightness = in.brightness;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(block_tex, block_sampler, in.uv, in.layer);
    return vec4<f32>(tex.rgb * in.brightness, 1.0);
}
