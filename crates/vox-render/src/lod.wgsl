// Voxterra coarse-LOD terrain shader (M09 amendment A4, ADR-0009).
//
// Split from shader.wgsl because the two vertex stages genuinely differ. LOD
// terrain is SKYLIGHT-ONLY SURFACE (an explicit M09 non-goal: no block light,
// no caves at distance), and it carries something full-res has no use for —
// `morph_y`, the Y this vertex takes at the NEXT COARSER level. Everything
// downstream of lighting (texture array, day/night sky_scale, the light curve,
// distance fog) is deliberately identical to shader.wgsl, so the two agree
// wherever they meet.
//
// ## Geomorph
//
// Without it, a region handing over from level N to level N+1 changes
// silhouette in one frame (a pop), and at a ring boundary two different
// resolutions abut with no reason to agree (a seam). Both are MOTION, which
// reads straight through fog — so A2 could not hide them.
//
// The fix is to arrive already matching: as the camera approaches the boundary
// this level's terrain lerps toward the coarser silhouette, reaching it exactly
// at the handover radius. The swap then replaces geometry with geometry that
// is already identical.
//
// Distance is CHEBYSHEV FROM THE CAMERA. Chebyshev because the ring's annuli
// are squares; from the camera because the camera is the only reference that
// moves CONTINUOUSLY.
//
// The first version measured from the ring's snapped centre, reasoning that
// this is where the annuli are measured from, so the morph would complete
// exactly where the handover fires. That was exactly backwards. The snapped
// centre is FROZEN between ring updates — it holds still, then teleports a
// whole coarsest-stride (256 blocks) at the very instant the handover happens.
// So `t` never animated at all: it was a step function firing simultaneously
// with the swap it existed to hide, and widening the band changed nothing.
//
// Measuring from the camera means `t` climbs smoothly as the player walks. The
// cost is that the morph no longer completes at exactly the swap radius — the
// snapped centre can sit up to 256 blocks from the camera, so the swap can
// arrive with `t` short of 1.0. A partial pop after most of the change has
// already happened smoothly beats a total pop, and a band wide enough to
// absorb that offset shrinks the remainder.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct ChunkData {
    // xyz = node origin - render origin (floating origin, ADR-0002).
    // w   = distance in blocks from the snapped centre at which THIS node's
    //       level hands over to the next coarser one, i.e. where its morph must
    //       be complete. 0 = never morph (the coarsest ring has nothing to
    //       morph toward).
    offset: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> chunk: ChunkData;

@group(2) @binding(0)
var block_tex: texture_2d_array<f32>;
@group(2) @binding(1)
var block_sampler: sampler;

// Sky / day-night / fog / morph uniform (group 3). Shared verbatim with
// shader.wgsl — same buffer, same layout.
//   cam_scale : xyz = camera position (render-relative), w = sky_scale
//   fog_color : rgb = colour terrain fades toward, w = strength (0 = off)
//   fog_range : x = fog start, y = fog end (blocks), zw reserved
//   morph     : x = morph band width in blocks (0 = morphing off)
struct SkyChunk {
    cam_scale: vec4<f32>,
    fog_color: vec4<f32>,
    fog_range: vec4<f32>,
    morph: vec4<f32>,
};
@group(3) @binding(0)
var<uniform> sky: SkyChunk;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) layer: u32,
    @location(3) sky_light: f32,
    @location(4) morph_y: f32,
    @location(5) shade: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) sky_light: f32,
    @location(3) shade: f32,
    @location(4) world_rel: vec3<f32>,
};

@vertex
fn vs_lod(in: VsIn) -> VsOut {
    var out: VsOut;

    // Morph factor. X/Z never move, so the horizontal position used to measure
    // distance is the same before and after — no feedback loop.
    let flat_rel = in.position + chunk.offset.xyz;
    let morph_end = chunk.offset.w;
    let band = sky.morph.x;
    var t = 0.0;
    if (morph_end > 0.0 && band > 0.0) {
        let d = max(
            abs(flat_rel.x - sky.cam_scale.x),
            abs(flat_rel.z - sky.cam_scale.z),
        );
        t = clamp((d - (morph_end - band)) / band, 0.0, 1.0);
    }

    // Morphing only ever LOWERS the surface (morph_y is the minimum over the
    // coarse cell, so it is <= this vertex's own Y — asserted headlessly by
    // `morph_never_raises_the_surface`). That keeps the never-exceed-real-
    // terrain contract intact mid-morph: coarse terrain sinking below real
    // ground is hidden, coarse terrain rising above it pokes through.
    let morphed = vec3<f32>(
        in.position.x,
        mix(in.position.y, in.morph_y, t),
        in.position.z,
    );
    let world_rel = morphed + chunk.offset.xyz;

    out.clip_position = camera.view_proj * vec4<f32>(world_rel, 1.0);
    out.uv = in.uv;
    out.layer = in.layer;
    out.sky_light = in.sky_light;
    out.shade = in.shade;
    out.world_rel = world_rel;
    return out;
}

// Mirror of vox_mesh::light_curve_f and shader.wgsl's light_curve. The ambient
// floor (0.004) and falloff exponent (1.8) live in three synced places; see the
// CLAUDE.md ambient-floor note before changing any of them.
fn light_curve(level: f32) -> f32 {
    let ambient = 0.004;
    let t = clamp(level, 0.0, 1.0);
    return ambient + (1.0 - ambient) * pow(t, 1.8);
}

@fragment
fn fs_lod(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(block_tex, block_sampler, in.uv, in.layer);

    // Skylight only — no block channel to combine, which is the whole reason
    // this shader is separate. Day/night dims it through the same sky_scale
    // the near field uses, so the two match across the boundary.
    //
    // The `max` against light_curve(0) is NOT dead code. shader.wgsl computes
    // max(sky_lit, light_curve(block)), and LOD vertices carried block = 0, so
    // the shared shader floored LOD brightness at the ambient constant. Dropping
    // it here would make distant terrain darker than near terrain at night —
    // a visible difference across exactly the boundary this milestone exists to
    // hide. Keep the formulas identical wherever they can be.
    let sky_scale = sky.cam_scale.w;
    let brightness = in.shade * max(light_curve(in.sky_light) * sky_scale, light_curve(0.0));
    var color = tex.rgb * brightness;

    // Distance fog (M09 amendment A2), identical to shader.wgsl.
    let fog_strength = sky.fog_color.w;
    if (fog_strength > 0.0) {
        let dist = length(in.world_rel - sky.cam_scale.xyz);
        let t = clamp(
            (dist - sky.fog_range.x) / max(sky.fog_range.y - sky.fog_range.x, 1.0),
            0.0,
            1.0,
        );
        color = mix(color, sky.fog_color.rgb, t * t * fog_strength);
    }
    return vec4<f32>(color, 1.0);
}
