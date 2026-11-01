
// --- Biome colour (M13 task 4, ADR-0018). Shared by shader.wgsl and
// lod.wgsl, so the colour of grass is the same at the player's feet and at
// 64 km: no step where full resolution meets the LOD.
//
// The stage-1 climate map (ADR-0017) is a small texture: per cell, the
// sea-level temperature (r, linear over -50..50 °C) and the annual
// precipitation (g, as sqrt over 0..9 000 mm) — `vox_render::encode_climate`.
// It wraps like the world (the torus, ADR-0012), so a lookup never meets a
// seam. A fragment cools its temperature for its own height, at the
// generator's lapse rate, then takes a grass colour from its climate:
// deep green where hot and wet, yellowed where hot and dry, fresh green where
// mild and wet, olive and then brown toward the cold.
//
//   climate_a : xy = where the render origin sits on the map (0..1, wrapped),
//               zw = 1 / world size in blocks (x, z)
//   climate_b : x = the render origin's height in blocks, y = 1 when a map
//               is set (0: grass untinted), z = lapse rate (°C per block),
//               w reserved

@group(3) @binding(2)
var climate_tex: texture_2d<f32>;
@group(3) @binding(3)
var climate_sampler: sampler;

// The grass tiles' layers (vox_core::registry), and the colour of the grass
// tile itself — the tint is relative to it, so mild, wet grass is unchanged.
const GRASS_TOP_LAYER: u32 = 2u;
const GRASS_SIDE_LAYER: u32 = 3u;
const GRASS_REFERENCE: vec3<f32> = vec3<f32>(0.3137, 0.5882, 0.2510);

// Grass colours at the corners of climate (Rust mirror: `grass_tint`).
const GRASS_HOT_WET: vec3<f32> = vec3<f32>(0.16, 0.47, 0.13);
const GRASS_HOT_DRY: vec3<f32> = vec3<f32>(0.72, 0.66, 0.30);
const GRASS_MILD_WET: vec3<f32> = vec3<f32>(0.3137, 0.5882, 0.2510);
const GRASS_COLD_WET: vec3<f32> = vec3<f32>(0.27, 0.45, 0.30);
const GRASS_COLD_DRY: vec3<f32> = vec3<f32>(0.52, 0.50, 0.33);

// Grass colour for mean annual temperature `t` (°C) and precipitation `p`
// (mm/yr). Wetness is judged against Köppen's aridity threshold, as the
// generator classifies biomes (vox_worldgen::biome).
fn grass_tint(t: f32, p: f32) -> vec3<f32> {
    let arid = 20.0 * max(t, 0.0) + 140.0;
    let wet = smoothstep(0.6, 2.5, p / arid);
    let dry = mix(GRASS_COLD_DRY, GRASS_HOT_DRY, smoothstep(-8.0, 26.0, t));
    let humid = mix(
        mix(GRASS_COLD_WET, GRASS_MILD_WET, smoothstep(-8.0, 12.0, t)),
        GRASS_HOT_WET,
        smoothstep(14.0, 26.0, t),
    );
    return mix(dry, humid, wet);
}

// The albedo a fragment is lit with: grass tinted by its climate, anything
// else as it is.
fn biome_albedo(albedo: vec3<f32>, layer: u32, world_rel: vec3<f32>) -> vec3<f32> {
    if (sky.climate_b.y < 0.5 || (layer != GRASS_TOP_LAYER && layer != GRASS_SIDE_LAYER)) {
        return albedo;
    }
    let uv = sky.climate_a.xy + world_rel.xz * sky.climate_a.zw;
    // Explicit level: this runs under a branch, where implicit derivatives
    // are not allowed — and the map has no mips anyway.
    let c = textureSampleLevel(climate_tex, climate_sampler, uv, 0.0);
    let sea_level_c = c.r * 100.0 - 50.0;
    let precip = c.g * c.g * 9000.0;
    let height = max(sky.climate_b.x + world_rel.y, 0.0);
    let t = sea_level_c - sky.climate_b.z * height;
    // The tile is sampled from an sRGB texture, so `albedo` is linear light:
    // the tint's ratio must be taken in linear light too, or it comes out
    // far too weak (dry grass stayed green).
    return albedo * to_linear(grass_tint(t, precip)) / to_linear(GRASS_REFERENCE);
}

// sRGB to linear, near enough for a tint (gamma 2.2).
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}
