# ADR-0007: Day/night — world time, the celestial model, and two-channel vertex light

**Status:** accepted (M07, shipped 2026-07-06). Both halves — the time/celestial
model and the two-channel vertex light — are implemented and shipped. See the
retrospective in `docs/milestones/07-day-night.md`. Note: during tuning the
shader combine became `max(light_curve(sky) × sky_scale, light_curve(block))`
(curve each source then dim sky) rather than the single-curve form sketched
below; the split keeps the falloff exponent independent of day/night dimming.
The ambient floor was also lowered from 0.035 to 0.004.

## Context

M07 adds a day/night cycle. The M06 retro established the governing idea: there
are **two distinct darknesses**, and they must stay separate.

- **Geometric darkness** — "no light reaches this cell" (sealed cave, overhang
  underside). Constant across time of day. Already handled: the 0.035 ambient
  floor baked into the shader's `light_curve` (M06).
- **Temporal darkness** — "the sky light source is dim right now" (night). A
  time-of-day scaling of the **sky** channel only. Torches (block light) must
  not dim at night.

Delivering the second without corrupting the first drives every decision below.
Two invariants also constrain us: light **propagation** and the ambient floor
are settled (M05/M06) and must not be reopened; and the engine must not grow
speculative machinery (CLAUDE.md).

## Decision

### 1. World time is a pure `vox-core` value

`WorldTime(u64)` where **one tick is one game-second** (`TICKS_PER_DAY =
86_400`). Time of day, sun direction, sky scale, and moon phase are **pure
functions of a `WorldTime`** (`vox-core::time`), unit-tested as truth tables.
No time state lives in the render loop — it advances the counter and reads
these functions.

Rationale for the tick unit: it makes the two speed modes fall out with no
special cases. Default speed (24 real min/day) advances 60 ticks/real-second (1
game-minute per real-second); a future **real-time-sync** hardcore world
advances 1 tick/real-second and seeds the counter from the wall-clock epoch.
Day length and lunation are **independent tunable constants**, so real-time sync
is a config, not a code path. `WorldTime` is one integer — trivially versioned
into the save format when that milestone lands.

### 2. The celestial model is procedural — no textures

- **Sun direction** is one unit vector on an east→overhead→west arc (X–Y plane;
  a north–south latitude tilt is a deliberate future refinement isolated to that
  one function). Everything visual and lighting-related derives from it (disc
  position, gradient orientation, the elevation that drives `sky_scale`), so
  nothing can disagree.
- **Sun disc** is drawn in the sky shader from `dot(view_ray, sun_dir)` — a
  crisp core plus a soft glow. Resolution-independent; no atlas.
- **Moon phase** is continuous (`moon_phase ∈ [0,1)`, new→full→new over
  `LUNATION_DAYS`). The moon disc is shaded by reconstructing a sphere normal
  across the disc and lighting it against a phase-derived direction — correct
  crescents/gibbous/terminator for *every* phase, not 8 texture frames. If a
  cratered albedo is ever wanted, it multiplies under this shading; nothing is
  thrown away.
- **`sky_scale(t)`** maps sun elevation through a smoothstep twilight band to
  `[night_floor, 1.0]`, where `night_floor` runs from ~0 (new moon) to a few %
  (full moon) via `moon_illumination`. Because the floor is layered on top of
  the geometric 0.035 ambient floor, a full-moon open field automatically reads
  brighter than a sealed cave, and the 0.035 value chosen in M06 needs no
  re-tuning. Curves are smooth over the whole day; dusk/dawn are the tuned
  readability moments.

`LUNATION_DAYS` defaults to 8 (shorter than the real ~29.5-day synodic month)
so the phase→night-brightness mechanic visibly changes within a session and is
learnable; a real-time-sync world would use the true value.

### 3. Vertex light becomes two channels (sky, block)

The M06 mesh collapses per-vertex light to one scalar via `max(sky, block)`.
That collapse makes day/night impossible: you cannot dim the sky at the shader
without also dimming torches, because by then they are the same number. So the
vertex carries **sky and block separately** end-to-end, and the shader combines
them per-vertex as:

    brightness = light_curve( max( sky × sky_scale , block ) )

`sky_scale` is a per-frame **uniform**, never baked into vertex data — otherwise
every dawn is a full-world re-mesh. The ambient floor stays inside `light_curve`
exactly as today.

## Consequences

- **Greedy merge ratio drops.** Faces merged under equal `max(sky, block)` may
  differ once split into a `(sky, block)` pair, so the greedy mask keys on the
  full per-vertex tuple. This is the M06-flagged cost; the mesh bench measures
  it against the 2420µs / 84-quad M06 baseline, and quantizing the channels is
  the first lever if it regresses badly. Correctness first, ratio second.
- **Seam tests grow a channel.** Border vertices must sample identical
  `(sky, block)` pairs from both sides; the M06 seam/gradient tests are
  extended per-channel. The geometry-scoped differential oracle (M06) is
  unaffected — it compares covered cells, not brightness.
- **Propagation and the ambient floor are untouched.** If M07 work seems to
  require editing `light.rs` propagation or the relight pipeline, that is a
  signal the two-darknesses split has been violated — day/night lives in the
  vertex format and the shader, nowhere else.
- **Vertex layout widens by one attribute** (`vox-render`/`vox-app`,
  review-only in the sandbox — Nathan compiles). The shader gains the combine
  formula, the `sky_scale` uniform, and the procedural sky pass.
- **Deliberately out of scope** (no speculative hooks): shadows, any
  sun-direction influence on light *propagation*, weather, clouds, sleep/skip,
  celestial textures, latitude tilt, and persistence. Each is a clean future
  addition on top of this foundation, not a hook to add now.
