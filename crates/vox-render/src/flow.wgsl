// --- Flowing water (M14, criterion 10). Shared by shader.wgsl and lod.wgsl,
// so a river drifts the same at the player's feet and kilometres off.
//
// The flow map is the drainage network's velocity per cell (`DrainageMap::
// flow`): red and green are x and z, centred on 128, ±127 = ±FLOW_MAX blocks
// a second — `vox_render::encode_flow`. It covers one lap of the world, as
// the climate map does, so it shares the climate map's placement
// (`climate_a`) and its wrapping, filtering sampler; `climate_b.w` is 1 once
// a flow map is set. `morph.z` is the water clock, in seconds.
//
// Water's tile is sampled twice, each offset along the flow by how far the
// water has moved in its own phase of a FLOW_CYCLE-second cycle, half a
// cycle apart, and cross-faded so the one about to jump back weighs nothing
// (the usual flow-map technique): water drifts at its speed, forever, with
// no stretching. Still water — lakes, the sea — has no flow and does not
// move. Fast water foams. Water's side faces — waterfalls, the steps of
// rapids — run straight down.

@group(3) @binding(4)
var flow_tex: texture_2d<f32>;

// Mirrors: vox_core::registry::L_WATER, vox_render::FLOW_MAX (pinned by a
// test).
const WATER_LAYER: u32 = 8u;
const FLOW_MAX: f32 = 4.0;
const FLOW_CYCLE: f32 = 4.0;
// How fast falling water's sides run down, blocks a second. A whole number
// of blocks per clock wrap (4 000 s), so the wrap never shows.
const FALL_SPEED: f32 = 3.0;

// The water's velocity under a point, blocks per second (x, z).
fn flow_at(world_rel: vec3<f32>) -> vec2<f32> {
    if (sky.climate_b.w < 0.5) {
        return vec2<f32>(0.0, 0.0);
    }
    let uv = sky.climate_a.xy + world_rel.xz * sky.climate_a.zw;
    let f = textureSampleLevel(flow_tex, climate_sampler, uv, 0.0).rg;
    return (f * 255.0 - 128.0) / 127.0 * FLOW_MAX;
}

// A translucent surface's colour and opacity: water drifting with its flow,
// anything else as its face has it. `face_uv` is the face's own texture
// coordinate; water's top takes the world's, so near and far tile alike.
fn water_sample(layer: u32, face_uv: vec2<f32>, world_rel: vec3<f32>, normal: vec3<f32>) -> vec4<f32> {
    let is_water = layer == WATER_LAYER;
    let top = is_water && abs(normal.y) > 0.5;
    let t = sky.morph.z;
    let uv = select(face_uv, world_rel.xz, top);
    let falling_uv = vec2<f32>(world_rel.x + world_rel.z, -world_rel.y - t * FALL_SPEED);
    // Derivatives taken here, in uniform control flow, so each kind of water
    // below pays only for the samples it uses (M14 bench: three samples on
    // every water pixel, the open sea's included, cost ~10% of frame rate):
    // still water one, a river two, falling water one. Flow offsets are
    // constant across a pixel, so the base uv's derivatives serve them all.
    let dx = dpdx(uv);
    let dy = dpdy(uv);
    let fdx = dpdx(falling_uv);
    let fdy = dpdy(falling_uv);
    var flow = vec2<f32>(0.0, 0.0);
    if (is_water) {
        flow = flow_at(world_rel);
    }
    var c: vec4<f32>;
    if (is_water && !top) {
        // Water's sides are falling water — a waterfall's sheet, a rapid's
        // step: its texture runs straight down.
        c = textureSampleGrad(block_tex, block_sampler, falling_uv, layer, fdx, fdy);
    } else if (flow.x != 0.0 || flow.y != 0.0) {
        let p0 = fract(t / FLOW_CYCLE);
        let p1 = fract(t / FLOW_CYCLE + 0.5);
        let s0 = textureSampleGrad(block_tex, block_sampler, uv - flow * (p0 * FLOW_CYCLE), layer, dx, dy);
        let s1 = textureSampleGrad(block_tex, block_sampler, uv - flow * (p1 * FLOW_CYCLE), layer, dx, dy);
        c = mix(s0, s1, abs(1.0 - 2.0 * p0));
    } else {
        c = textureSampleGrad(block_tex, block_sampler, uv, layer, dx, dy);
    }
    // Fast water foams; falling water is white water.
    let foam = max(smoothstep(1.5, 3.5, length(flow)) * 0.6, select(0.0, 0.35, is_water && !top));
    return vec4<f32>(mix(c.rgb, vec3<f32>(0.92, 0.95, 0.97), foam), mix(c.a, 1.0, foam * 0.8));
}
