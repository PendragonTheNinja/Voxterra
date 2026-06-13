// Voxterra chunk shader, Milestone 02 (floating origin, ADR-0002).
//
// Vertices arrive in LOCAL chunk space (0..32) and pre-colored. Each chunk
// supplies an `offset` = (chunk_world_origin - render_origin), computed on
// the CPU in i64 and narrowed to f32 while small, so positions stay precise
// arbitrarily far from world origin. The camera's view_proj is built with
// the camera at the render origin, so this offset and the view agree.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

// Per-chunk data, bound per draw (group 1). `offset.w` is unused padding so
// the struct is 16-byte aligned.
struct ChunkData {
    offset: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> chunk: ChunkData;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world_rel = in.position + chunk.offset.xyz;
    out.clip_position = camera.view_proj * vec4<f32>(world_rel, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
