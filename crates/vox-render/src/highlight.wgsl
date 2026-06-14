// Targeted-block highlight (Milestone 03 task 3): a wireframe cube outline
// drawn on the block the camera is looking at. Shares the camera uniform
// (group 0) and uses a per-draw offset (group 1) just like chunks, so it
// sits exactly on the targeted voxel under floating origin (ADR-0002).

struct Camera {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> camera: Camera;

struct Highlight {
    offset: vec4<f32>, // (block_world - render_origin), .w unused
};
@group(1) @binding(0)
var<uniform> hl: Highlight;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    let world_rel = position + hl.offset.xyz;
    return camera.view_proj * vec4<f32>(world_rel, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Solid dark outline.
    return vec4<f32>(0.02, 0.02, 0.02, 1.0);
}
