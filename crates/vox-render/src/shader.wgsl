// Voxterra chunk shader, Milestone 03 (texture array, ADR-0003) + M07 day/night
// (ADR-0007).
//
// Vertices arrive in LOCAL chunk space (0..32) with texture coordinates `uv`
// (running 0..W / 0..H across a greedy-merged quad), a texture-array `layer`
// index per face, and the two light channels `sky` and `block` (each 0..1)
// plus a baked `shade` (directional face brightness × ambient occlusion). Each
// chunk supplies an `offset` = (chunk_world_origin - render_origin), narrowed
// to f32 while small for floating-origin precision (ADR-0002).
//
// The fragment samples the block texture array at (uv, layer) with a Repeat
// sampler (so the layer tiles across merged faces) and nearest filtering
// (crisp voxel pixels), then applies lighting:
//   brightness = shade * light_curve(max(sky * sky_scale, block))
// sky_scale dims the SKY channel by time of day WITHOUT touching block light
// (torches). See ADR-0007.

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

// Sky / day-night uniform (M07 task 3, ADR-0007). x = sky_scale (0..1); yzw
// reserved for the sky pass (task 3b).
@group(3) @binding(0)
var<uniform> sky: vec4<f32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    @location(3) sky: f32,
    @location(4) block: f32,
    @location(5) shade: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) sky: f32,
    @location(3) block: f32,
    @location(4) shade: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world_rel = in.position + chunk.offset.xyz;
    out.clip_position = camera.view_proj * vec4<f32>(world_rel, 1.0);
    out.uv = in.uv;
    out.layer = in.layer;
    out.sky = in.sky;
    out.block = in.block;
    out.shade = in.shade;
    return out;
}

// Mirror of vox_mesh::light_curve_f (M06 task 5): a small ambient floor so
// unlit geometry is dim but not pure black, with a gentle gamma. Input is the
// already-combined, already-sky-scaled light in 0..1.
fn light_curve(level: f32) -> f32 {
    // Ambient floor: brightness where NO light reaches (sealed cave, overhang
    // underside). M07 task 4: a hair above zero — "almost black", not a dead
    // #000 (owner's request). Night darkness comes from sky × sky_scale, not
    // this floor. Keep in sync with vox_mesh::light_curve_f and the CLAUDE.md
    // ambient-floor note.
    let ambient = 0.004;
    let t = clamp(level, 0.0, 1.0);
    // Steep falloff (exponent > 1, ~inverse-square feel) so light fades
    // GRADUALLY into darkness across the low levels instead of holding bright
    // then snapping to black. Higher = more dramatic; lower = flatter.
    return ambient + (1.0 - ambient) * pow(t, 1.8);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(block_tex, block_sampler, in.uv, in.layer);

    // Day/night: dim the SKY channel by sky_scale, leaving block light (torches)
    // untouched. sky_scale is driven per-frame by vox_core::WorldTime.
    let sky_scale = sky.x;

    // Curve each light source FIRST, then combine. This keeps the falloff
    // exponent (spatial shading, in light_curve) separate from the day/night
    // dimming: sky_scale scales curved sky light LINEARLY, so night brightness
    // and moon-phase differences survive instead of being crushed by the
    // exponent. Block light (torches) is never dimmed by time of day. Because
    // light_curve is monotonic, at full daylight (sky_scale = 1) this equals the
    // old max()-then-curve exactly — daytime is unchanged; only night differs.
    let sky_lit = light_curve(in.sky) * sky_scale;
    let block_lit = light_curve(in.block);
    let brightness = in.shade * max(sky_lit, block_lit);
    return vec4<f32>(tex.rgb * brightness, 1.0);
}
