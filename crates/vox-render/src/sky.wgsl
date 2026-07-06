// Voxterra procedural sky, Milestone 07 task 3b (ADR-0007).
//
// A fullscreen pass drawn BEFORE the terrain: it fills every pixel with sky,
// then terrain draws over it (terrain writes depth and tests Less against the
// cleared 1.0; this pass writes no depth and always passes, so it never
// occludes geometry). No textures — gradient, sun disc, phase-correct moon, and
// a starfield are all procedural.
//
// Everything is computed from a per-pixel WORLD-SPACE ray direction,
// reconstructed from the inverse view-projection. That makes the sun, moon, and
// especially the STARS locked to world directions: they hold still as the
// camera rotates (no screen-space crawl — the #1 "fake" tell), and antialias
// smoothly because each star is a soft angular point, not a hard pixel.

struct SkyUniform {
    inv_view_proj: mat4x4<f32>,
    // xyz = sun direction (toward sun), w = sky_scale (0 night .. 1 day)
    sun: vec4<f32>,
    // xyz = moon direction (toward moon), w = moon illumination (0 new .. 1 full)
    moon: vec4<f32>,
    // x = star intensity knob, y = milky-way knob, z = twinkle knob, w spare
    params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> sky: SkyUniform;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vid: u32) -> VsOut {
    // One oversized triangle covering the screen (no vertex buffer).
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    let ndc = vec2<f32>(x, y) * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

// --- hashing (cheap, decent quality) -------------------------------------

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 345.45));
    q += dot(q, q + 34.345);
    return fract(q.x * q.y);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let k = vec2<f32>(hash21(p), hash21(p + 19.19));
    return k;
}

// Interleaved gradient noise (Jimenez): a dither pattern that reads as fine
// film grain rather than a structured texture, so it breaks 8-bit banding
// without looking like an overlay on the smooth sky.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// --- starfield ------------------------------------------------------------
//
// Cube-face projection: map the ray direction to a 2D coordinate on its
// dominant-axis face, then place jittered stars in a cell grid. Jitter breaks
// the grid (no fake regularity); a power-law magnitude gives a few bright
// standouts among many faint ones; a per-star soft radius antialiases; a
// keep-probability leaves cells empty so the field is sparse and Poisson-ish.
//
// Known minor artifact: the pattern can shift slightly across cube-face seams
// (where the dominant axis changes). Usually invisible with small dense stars;
// if a seam shows during tuning, the fix is an overlapping/seamless
// parametrization — deferred as a tuning follow-up.

fn star_face(dir: vec3<f32>) -> vec3<f32> {
    let a = abs(dir);
    var uv: vec2<f32>;
    var face: f32;
    if (a.x >= a.y && a.x >= a.z) {
        uv = dir.yz / a.x;
        face = select(0.0, 1.0, dir.x < 0.0);
    } else if (a.y >= a.z) {
        uv = dir.xz / a.y;
        face = select(2.0, 3.0, dir.y < 0.0);
    } else {
        uv = dir.xy / a.z;
        face = select(4.0, 5.0, dir.z < 0.0);
    }

    let density = 90.0;           // stars-per-face scale (tuning knob)
    let p = uv * density;
    let cell = floor(p);
    let fp = fract(p);
    let face_salt = vec2<f32>(face * 37.1, face * 17.7);

    // Pixel footprint in cell units, computed ONCE here in uniform control flow
    // (the face if/else above has reconverged). CLAMPED: at a cube-face seam the
    // raw derivative spikes, which previously drew the seams as bright dashed
    // lines — the clamp caps that so seams stay invisible. This footprint sets
    // each star's minimum on-screen size so sub-pixel stars stay stable instead
    // of aliasing on/off when the camera moves.
    let px = min(length(fwidth(p)), 0.02) + 1e-5;

    var col = vec3<f32>(0.0);
    for (var j: i32 = -1; j <= 1; j = j + 1) {
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            let off = vec2<f32>(f32(i), f32(j));
            let id = cell + off + face_salt;

            let keep = hash21(id + 5.7);
            if (keep < 0.42) {                 // ~42% of cells host a star
                let jitter = hash22(id + 1.3); // random position within the cell
                let star_pos = off + jitter;
                let d = distance(fp, star_pos);

                // Magnitude: bias hard toward faint, few bright ones.
                let mag = pow(hash21(id + 9.1), 5.0);
                let radius = mix(0.010, 0.05, mag);

                // Soft round point (this is the look that reads as real stars —
                // bright ones concentrated and crisp, faint ones dim), but with
                // the falloff radius floored to ~1px so sub-pixel stars fade
                // smoothly instead of flickering when the camera moves. This
                // keeps magnitude CONTRAST (bright stars stay small+bright)
                // rather than flattening everything to a uniform sheet.
                let soft = max(radius, px * 1.3);
                let core = smoothstep(soft, 0.0, d);
                let bright = core * mix(0.12, 1.0, mag);

                // Faint color-temperature variation (blue-white to warm).
                let temp = hash21(id + 3.9);
                let tint = mix(vec3<f32>(0.72, 0.82, 1.0), vec3<f32>(1.0, 0.86, 0.7), temp);
                col += tint * bright;
            }
        }
    }
    return col;
}

// --- main -----------------------------------------------------------------

@fragment
fn fs_sky(in: VsOut) -> @location(0) vec4<f32> {
    // Reconstruct a world-space ray direction (origin-independent, so floating
    // origin doesn't matter for the sky).
    let near = sky.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = sky.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - near.xyz / near.w);

    let sun_dir = normalize(sky.sun.xyz);
    let sky_scale = sky.sun.w;
    let moon_dir = normalize(sky.moon.xyz);
    let sun_elev = sun_dir.y;
    let up = clamp(dir.y, 0.0, 1.0);

    // --- gradient: zenith→horizon, blended day↔night by sky_scale ---
    let day_zenith = vec3<f32>(0.20, 0.42, 0.80);
    let day_horizon = vec3<f32>(0.68, 0.82, 0.95);
    // Night base darkened in M07 task 4 so the sky reads darker at night.
    let night_zenith = vec3<f32>(0.004, 0.006, 0.018);
    let night_horizon = vec3<f32>(0.012, 0.018, 0.038);
    let horizon_w = pow(1.0 - up, 1.6);
    let day = mix(day_zenith, day_horizon, horizon_w);
    let night = mix(night_zenith, night_horizon, horizon_w);
    var col = mix(night, day, sky_scale);

    // --- warm dawn/dusk glow, concentrated toward the sun's azimuth ---
    // The sun arcs due east↔west, so its glow should sit on THAT side of the
    // horizon and fade to cool blue opposite it — not tint the whole ring.
    let low_sun = smoothstep(0.35, 0.0, abs(sun_elev)); // 1 when sun near horizon
    let near_horizon = smoothstep(0.30, 0.0, up);
    let dir_h = normalize(vec2<f32>(dir.x, dir.z) + vec2<f32>(1e-5, 0.0));
    let sun_h = normalize(vec2<f32>(sun_dir.x, sun_dir.z) + vec2<f32>(1e-5, 0.0));
    let toward_sun = clamp(dot(dir_h, sun_h), 0.0, 1.0); // 1 toward sun, 0 away
    // Slightly deeper/redder right at the sun; softer amber to the sides.
    let warm = mix(vec3<f32>(0.95, 0.45, 0.22), vec3<f32>(1.0, 0.62, 0.34), toward_sun);
    let glow_amt = low_sun * near_horizon * (0.15 + 0.75 * toward_sun);
    col = mix(col, warm, glow_amt);

    // --- stars: fade in as it darkens, thin out toward the horizon ---
    let night_factor = smoothstep(0.55, 0.05, sky_scale);
    let above = smoothstep(-0.03, 0.18, dir.y); // none below horizon, thin near it
    let star_knob = max(sky.params.x, 0.0);
    col += star_face(dir) * night_factor * above * star_knob;

    // --- moon disc, lit by the real sun direction (phase = geometry) ---
    let md = dot(dir, moon_dir);
    let moon_cos = 0.9994;                 // cos(angular radius) — tuning knob
    if (md > moon_cos) {
        let f = moon_dir;
        let r = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), f));
        let u = cross(f, r);
        let ang_r = sqrt(max(1.0 - moon_cos * moon_cos, 1e-6)); // sin(radius)
        let dx = dot(dir, r) / ang_r;
        let dy = dot(dir, u) / ang_r;
        let rr = dx * dx + dy * dy;
        if (rr <= 1.0) {
            let nz = sqrt(1.0 - rr);
            // Outward normal of the near hemisphere (center points at observer).
            let normal = normalize(dx * r + dy * u - nz * f);
            let lit = clamp(dot(normal, sun_dir), 0.0, 1.0);
            // Sharp terminator, near-zero dark side so the unlit part sinks
            // into the sky (crescents read as crescents, not a grey disc).
            let shade = mix(0.006, 1.0, smoothstep(0.02, 0.16, lit));
            let edge = smoothstep(1.0, 0.9, rr); // AA the disc rim
            let moon_col = vec3<f32>(0.94, 0.93, 0.88) * shade;
            col = col + moon_col * edge;
        }
    }

    // --- sun disc + glow, only above the horizon ---
    let sd = dot(dir, sun_dir);
    let sun_vis = smoothstep(-0.08, 0.03, sun_elev);
    let disc = smoothstep(0.99965, 0.99975, sd);      // crisp ~0.5° disc
    let glow = pow(max(sd, 0.0), 220.0) * 0.45;
    col += (vec3<f32>(1.0, 0.97, 0.88) * disc + vec3<f32>(1.0, 0.8, 0.5) * glow) * sun_vis;

    // Dither to break 8-bit banding, as fine film-grain noise. Scaled DOWN at
    // night (low sky_scale) so the dark sky doesn't show visible static grain —
    // banding only really appears in the bright daytime gradient and sun glow,
    // so that is where the dither runs at full strength.
    let dith_amt = mix(0.35, 1.5, clamp(sky_scale, 0.0, 1.0)) / 255.0;
    col += vec3<f32>((ign(in.clip.xy) - 0.5) * dith_amt);

    return vec4<f32>(col, 1.0);
}
