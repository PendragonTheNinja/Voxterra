
// ---- Atmosphere (M11 task 6, ADR-0016) ----
//
// Appended to BOTH terrain shaders, like curvature.wgsl, so near and far
// terrain see through the same air. Two effects, in order:
//
// 1. Aerial perspective. Air scatters short wavelengths most (Rayleigh), so
//    light from distant terrain loses its blue on the way to the eye while the
//    air between adds scattered sky light in front of it: dark distant ground
//    turns blue-grey and bright ground slightly warm — why far mountains look
//    blue. Haze (Mie) dims all colours alike. Both thin with height, haze much
//    faster, so the view from a summit is clearer than from the plain below.
//    Coefficients are real sea-level values per metre — one block.
// 2. The edge fade (M09 amendment A2): a squared ramp to the horizon colour
//    over the far part of the LOD horizon, which hides where terrain ends
//    against the sky and where LOD detail changes.
//
// Inputs, from the terrain uniform: the in-scatter colour is `fog_color.rgb`
// (the horizon sky, dimmed at night); `fog_range.w` scales the air's density
// (0 = no aerial perspective); `morph.y` is the eye's height above sea level.
// The distance is the flat one — curvature changes it by under 0.01%.

const RAYLEIGH = vec3<f32>(5.8e-6, 13.5e-6, 33.1e-6);
const MIE = 21.0e-6;
const RAYLEIGH_HEIGHT = 8000.0;
const MIE_HEIGHT = 1200.0;

fn atmosphere(color: vec3<f32>, world_rel: vec3<f32>) -> vec3<f32> {
    let to_point = world_rel - sky.cam_scale.xyz;
    let dist = length(to_point);
    var c = color;

    let density = sky.fog_range.w;
    if (density > 0.0) {
        // Density at the path's mid-height stands in for the integral; the
        // path rarely spans more than a few hundred blocks of height.
        let mid = max(sky.morph.y + 0.5 * to_point.y, 0.0);
        let depth = dist * density * (
            RAYLEIGH * exp(-mid / RAYLEIGH_HEIGHT) + vec3<f32>(MIE * exp(-mid / MIE_HEIGHT))
        );
        let t = exp(-depth);
        c = c * t + sky.fog_color.rgb * (1.0 - t);
    }

    let fade = sky.fog_color.w;
    if (fade > 0.0) {
        let t = clamp(
            (dist - sky.fog_range.x) / max(sky.fog_range.y - sky.fog_range.x, 1.0),
            0.0,
            1.0,
        );
        c = mix(c, sky.fog_color.rgb, t * t * fade);
    }
    return c;
}
