# ADR-0016: Aerial perspective

- **Status:** Proposed (M11 task 6, 2026-10-16). Accepted once the owner has
  seen it in play.
- **Context:** M11 criterion 9 ("distant terrain fades toward sky colour so
  range reads as depth — aerial perspective, not a wall"), made
  wavelength-dependent at the owner's request on 2026-10-01: far mountains
  should turn blue-violet as they do through real air, not fade to white
  (`docs/vision.md` §1.5). With a 64 km horizon (ADRs 0014, 0015) it is the
  main cue for distance.

## Decision

**Physical extinction and in-scattering, per colour channel, in one shared
shader function, in front of the existing distance fade.**

`atmosphere.wgsl` is appended to both terrain shaders, like curvature
(ADR-0015), so near and far terrain see through the same air. For a surface
`dist` blocks away it computes an optical depth per channel and blends:

    depth = dist × density × (β_R · e^(−h/8000) + β_M · e^(−h/1200))
    colour = colour × e^(−depth) + horizon_sky × (1 − e^(−depth))

- **Real coefficients**, sea level, per metre (= per block): Rayleigh
  β_R = (5.8, 13.5, 33.1) × 10⁻⁶ for red, green, blue — blue scatters about
  six times as strongly as red — and haze (Mie) β_M = 21 × 10⁻⁶, the same for
  every colour.
- **Thinning with height.** Air falls off over 8 km, haze over 1.2 km; `h` is
  the path's mid-height above sea level. A view from a summit looks over most
  of the haze.
- **In-scatter colour** is the horizon sky colour the fog already used
  (dimmed with `sky_scale`, so night fades distant terrain to dark).
- **The distance fade stays**, after the air: a squared ramp to the horizon
  colour over the far part of the LOD horizon. Air alone does not hide where
  terrain ends against the sky — at a 2 km horizon it barely acts — and the
  fade is what keeps LOD detail changes and the terrain's edge unreadable.
- **Inputs** ride the terrain uniform's spare slots: air density in
  `fog_range.w`, eye height above sea level in `morph.y`. The menu's "air
  density" slider scales density (1 = Earth's, 0 = off); the fog toggle
  switches the whole atmosphere.

What it does to a dark green hill (0.12, 0.30, 0.10), seen from the plain:

| Distance | Transmitted R / G / B | Colour |
|---|---|---|
| 0.5 km | 0.99 / 0.98 / 0.97 | unchanged |
| 10 km | 0.76 / 0.71 / 0.58 | teal-grey (0.25, 0.45, 0.46) |
| 32 km | 0.42 / 0.33 / 0.18 | pale blue (0.44, 0.65, 0.80) |
| 64 km | 0.18 / 0.11 / 0.03 | almost sky (0.58, 0.76, 0.92) |

From a 2 000-block summit the same hill at 32 km keeps 0.63 / 0.51 / 0.29 and
reads blue-green rather than pale.

## Consequences

- Distance reads as depth across the whole horizon: each successive ridge is
  bluer and fainter, the effect the owner described.
- Bright surfaces (snow, once biomes exist) shift slightly warm at range,
  because blue leaves them faster than the air adds it back — physically
  right, and free.
- The density is the one tuning knob. Earth's sea-level air is a hazy day;
  clearer days are lower densities. A per-biome or per-weather density is the
  natural extension once those exist.
- The distance used is the flat one; curvature changes it by under 0.01%.

## Tests

`vox-render`: both terrain shaders end with the shared curvature and
atmosphere functions, neither keeps its own fog, and both light through
`atmosphere()`; every shader parses and validates with naga.
`vox_core::settings`: the density clamps to its range like every other slider.
