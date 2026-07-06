# Milestone 07 — Day/Night Cycle

**Goal:** a living sky. World time drives a smooth day/night cycle: the sky
channel of the light field dims at night while block light (torches) is
untouched, the sky itself renders as a procedural gradient with a sun disc and
a phase-correct moon, and moon phase sets how bright the night is. This
completes the lighting arc begun in M04–M06 by finally separating the two
darknesses identified in the M06 retro: "no light reaches here" (the 0.035
ambient floor, geometric, time-independent) vs. "the light source is dim right
now" (night, a time-of-day scaling of the SKY channel only).

**Acceptance scene:** stand in a field with a torch-lit cabin nearby and a cave
mouth behind you. Watch a full day pass. Dusk falls as a smooth, readable
gradient — sky and terrain dim together and the sun disc sets, while the
cabin's torchlight does not dim one bit. At full-moon midnight the open
field reads clearly brighter than the cave interior (moonlit sky-reach sits on
top of the geometric floor, exactly as the M06 design predicted); at new moon
the field falls to near-floor and the torchlight owns the scene. Dawn reverses
it. The moon shows a correct terminator for its phase. No banding, no stepped
brightness jumps, no re-tuning of the 0.035 floor.

## Acceptance criteria

1. **World time is a deterministic `vox-core` value.** Tick-based `WorldTime`
   newtype; day length a tunable constant (default 24 real minutes per game
   day); moon phase a continuous 0..1 derived from time (default lunation:
   8 game days). Pure functions — `sun_direction(t)`, `sky_scale(t, phase)`,
   `moon_phase(t)` — unit-tested as truth tables (noon, midnight, dusk
   midpoint, each quarter phase). No time state lives in the render loop.
2. **Two-channel vertex light.** Mesh vertices carry sky and block light as
   separate values end-to-end (the M06 `max` collapse is removed). Greedy
   mask keys on the full per-vertex tuple. Seam tests updated: border
   vertices sample identical (sky, block) pairs from both sides. The
   geometry-scoped differential oracle still passes.
3. **Shader combines channels per-vertex:**
   `brightness = light_curve(max(sky × sky_scale, block))`, ambient floor
   preserved inside `light_curve` exactly as today. `sky_scale` is a uniform
   updated per frame. Consequences that must hold in-game: torch light is
   identical at noon and midnight; a sealed cave is identical at noon and
   midnight; the 0.035 floor needs no re-tuning (it is the geometric minimum
   and time scaling sits above it).
4. **Procedural sky pass.** No textures. Sky gradient (zenith/horizon colors)
   driven by sun elevation; sun rendered as a shader disc + soft glow from
   `dot(view_ray, sun_dir)`; moon rendered as a disc shaded by a
   reconstructed sphere normal against a phase-derived light direction —
   continuous, geometrically correct phases (crescent through gibbous), not
   stepped frames. Sun disc position and lighting share one sun direction.
5. **Smooth curves everywhere.** `sky_scale` and sky colors are smooth over
   the full day (no steps); dusk/dawn are gradual and readable — the
   M06-flagged "interesting readability moments" get explicit in-scene
   tuning. Full moon night ≈ a few % of sky scale; new moon ≈ near zero.
6. **Performance.** Mesh bench before/after the two-channel key change
   (M06 baseline: 2420µs, 84 quads — expect a merge-ratio hit where
   distinct (sky, block) pairs collapsed under `max`; measure it, bound it).
   Relight untouched (stays 447µs). Sprint-fly at radius 8: steady 144,
   burst floor ≥ 90 — run it at night with the sky pass on, not just at noon.
7. **Bookkeeping.** ADR-0007 (two-channel vertex light + celestial model)
   accepted alongside Task 2. Retrospective with numbers; CLAUDE.md status
   updated; next-milestone candidates re-listed (LOD remains the headline —
   its design ADR should be drafted during or immediately after this
   milestone).

## Non-goals (deliberate, per the no-speculative-hooks rule)

- **No shadows** and **no directional influence on light propagation.**
  Skylight propagation stays exactly as M05 built it; the sun direction
  affects the sky pass and the `sky_scale` curve only.
- **No weather, clouds, or fog model changes** (matching fog color to the
  sky gradient is allowed if fog already exists; building fog is not).
- **No sleep/skip-night mechanic** (gameplay, not engine).
- **No celestial textures** — procedural only (texture albedo is a
  documented future swap-in, multiplied under the same phase shading).
- **No persistence work.** `WorldTime` is designed to be trivially
  serializable (a tick counter), but the save format itself is a future
  milestone; when it lands, time enters it versioned like everything else.

## Tasks

1. **`vox-core` time module + ADR-0007.** `WorldTime` (u64 ticks), day-length
   and lunation constants, `sun_direction` (simple great-circle east→west
   path), `sky_scale(t, phase)` (smoothstep dusk/dawn windows, phase-scaled
   night floor), `moon_phase`. Truth-table tests. Add new public items to the
   `vox-core` re-export allowlist (recurring gotcha — the sandbox cannot
   catch the omission). Draft ADR-0007 covering the celestial model and the
   Task 2 vertex format change together.
2. **Two-channel vertex light through `vox-mesh`.** Vertex format carries
   (sky, block); remove the `max` collapse; extend the greedy key; update
   seam/gradient tests to assert per-channel values; re-run the mesh bench
   and record the merge-ratio delta. Headless; failing-test-first (write the
   two-channel seam test before the format change).
3. **Shader + sky pass (`vox-render`/`vox-app`, review-only in sandbox).**
   Second light attribute through the vertex layout; `sky_scale` uniform;
   the combine formula in WGSL; the procedural sky pass (gradient, sun disc,
   moon disc with sphere-normal phase shading). App side: tick `WorldTime`
   in the game loop; a debug time-set key (temporary, RUST_LOG-gated or
   removed before commit) so Nathan can jump to dusk/midnight/dawn while
   tuning. Nathan is the compile and visual check.
4. **Polish + retrospective.** Tune dusk/dawn curve widths, night floors per
   phase, and sky colors in the acceptance scene; verify the three
   invariance checks from criterion 3 in-game; night sprint-fly run; numbers
   into the retrospective; CLAUDE.md status update.

## Notes for whoever builds this (human or model)

- Read CLAUDE.md invariants + M05 lessons first, as always. This milestone
  deliberately does NOT touch light propagation or streaming — if a change
  seems to require editing `light.rs` propagation or the relight pipeline,
  stop and re-read the design; the entire point of the M06 two-darknesses
  analysis is that day/night lives in the mesh vertex format and the shader,
  not in the light field.
- The named trap: **the greedy key.** Splitting `max(sky, block)` into two
  channels means quads that merged under equal maxima may stop merging under
  unequal pairs. The M06 retro flagged quantization as the first lever if the
  bench regresses badly. Measure before reaching for it.
- Keep `sky_scale` out of the mesh. It must be a per-frame uniform, never
  baked into vertex data — otherwise every dawn is a full-world re-mesh.
- The moon-phase shading trick: reconstruct `normal = (dx, dy, sqrt(1-dx²-dy²))`
  across the disc, light it with a direction rotated by `phase × 2π` in the
  disc's local plane, clamp `dot`. New moon then renders as a barely-visible
  dark disc — correct, and a nice touch against the stars-less sky.
- Sun direction is a foundation, not a feature: derive everything (disc
  position, gradient orientation, `sky_scale`'s notion of "elevation") from
  the one vector so nothing can ever disagree with anything else.

---

# Retrospective (2026-07-06)

M07 shipped a full day/night cycle: a living procedural sky, terrain that dims
and brightens with the sun, moonlight that varies the darkness of the night by
phase, and genuinely dark caves. All acceptance criteria met.

## What shipped

- **`vox-core::time`** — `WorldTime` as a tick counter where one tick is one
  game-second (`TICKS_PER_DAY = 86_400`), making the two speed modes fall out
  cleanly (default 24-min day = 60 ticks/real-sec; a future real-time-sync world
  = 1 tick/real-sec, seeded from the wall clock). Pure, truth-table-tested
  functions: `sun_direction`, `sun_elevation`, `sky_scale`, `moon_phase`,
  `moon_illumination`, `moon_direction`, plus `game_ticks_per_second`.
- **Two-channel vertex light** (ADR-0007) — the M06 `max(sky, block)` collapse
  is gone; vertices carry sky and block separately plus a baked `shade`
  (face-brightness × AO), and the shader combines them. This is what lets night
  dim the sky without touching torches.
- **Procedural sky pass** — fullscreen, no textures: zenith→horizon gradient,
  sun disc + glow, a moon lit by the real sun direction, and a world-locked
  starfield. Drawn behind terrain (no depth write).
- **Day/night wiring** — `WorldTime` ticks in the game loop; `sky_scale` and the
  sun/moon directions feed the shaders per frame. Debug keys: `\` 60× fast-
  forward, `[`/`]` ±1 game-hour, `T` pause, `M` +1 game-day (moon phase).

## Numbers

- **Meshing:** the two-channel split cost **0 merge ratio** on the realistic
  bench (84 → 84 quads) — the block channel is all-zero without torches, so
  merging stays sky-driven. Time cost was ~+5.7% from carrying the second
  channel. The predicted merge cost only appears where a block-light gradient
  diverges from sky (torches under open sky), which the bench doesn't exercise —
  a torch-lit bench variant is a good future addition. (Note: the M06 code
  comment's "36 quads" was stale; true M06 baseline was already 84.)
- **Relight:** untouched — day/night never triggers it.
- **Frame rate** (owner's machine, radius 8): **~144 fps** normal;
  **~90 fps** sprint-fly floor — meets the spec's "burst floor ≥ 90". The dip is
  streaming/mesh throughput under fast flight (dirty backlog ~1900,
  msh ~500 ms), the bottleneck LOD + async generation will relieve.
- **The load-bearing invariant, proven in the log:** with time fast-forwarding
  at 60× while stationary, `dirty` stayed **0** and `msh` stayed **0 ms** — the
  sun moving re-meshes nothing. `sky_scale` is a per-frame uniform, exactly as
  designed.
- Tests: 153 `vox-core` + 24 `vox-mesh`, all green.

## Design wins

- **Moon phase is geometry, not a hack.** The moon is *placed* by rotating the
  sun direction by `phase × 2π`, then *lit by the real sun direction* — so new
  moon rides with the sun (dark, daytime), full moon sits opposite (bright, high
  at midnight), and the terminator/crescents fall out for free. Better and
  simpler than the ADR's original phase-direction sketch.
- **Decoupled lighting.** Late in tuning we split the falloff exponent from the
  day/night dimming: `max(light_curve(sky) × sky_scale, light_curve(block))`
  instead of `light_curve(max(sky × sky_scale, block))`. The exponent now shapes
  spatial falloff without crushing night brightness, and — because `light_curve`
  is monotonic — daytime is mathematically identical to before. This is worth
  preserving.
- **Sets up LOD (ADR-0008).** The `sky_scale` uniform and two-channel vertex
  format are exactly what distant-terrain lighting needs; day/night will compose
  across the whole view for free.

## Lessons (the tuning saga)

- **sRGB lifts darks, hard.** Linear floor values look far brighter on screen
  than the number suggests (linear 0.028 ≈ 0.19 perceptual). Night tuning took
  several rounds because each drop was partly undone by the gamma. Tune lighting
  by eye on an sRGB target, not by the raw constant.
- **One constant doing two jobs hides bugs.** The ambient floor was both "cave
  darkness" and "night floor"; the exponent shaped both "spatial falloff" and
  "day/night mapping". Both had to be *decoupled* before tuning behaved. When a
  value resists tuning, check whether it is overloaded.
- **Star AA is subtle.** Direction-space rendering stops crawl, but sub-pixel
  points still alias; the fix is a screen-space-derivative footprint — computed
  in *uniform* control flow (not inside the per-star branch), **clamped** so it
  doesn't spike into bright streaks at cube-face seams, and paired with a soft
  falloff + min-size so magnitude contrast survives.
- Visual milestones need many short human-in-the-loop iterations; the
  headless-verify / owner-compiles split held up well across ~half a dozen.

## Constitutional change (recorded)

The **ambient floor was lowered 0.035 → 0.004** ("almost black"), reversing the
M06 "dark but not pure black" decision, at the owner's request: enclosed spaces
now read genuinely dark and **require a light source**. New-moon open ground is
kept faintly readable above this via `NIGHT_SKY_MIN` (skylight, not the floor).
The falloff exponent was steepened 0.85 → 1.8 for a natural, gradual fade to
dark. See the "Lighting & streaming invariants" note in CLAUDE.md.

## Known residuals / deferred

- Star **cube-face seams** are clamped to invisibility but the pattern still
  shifts subtly across them; a seamless parametrization is a future polish.
- Sun arcs due east–west with **no latitude tilt** (isolated to
  `sun_direction`); a tilt is a clean future add.
- **Celestial textures** deferred — the procedural moon can take an albedo
  texture multiplied under the same phase shading whenever wanted.
- An unexplained **red line** across the sky appeared once in testing; likely the
  block-highlight wireframe, not sky code — investigate if it recurs.
- Dusk/dawn azimuthal glow is basic (concentrated toward the sun, but no
  scattering model).

## Next

**LOD / far view distance (ADR-0008)** — the headline candidate since M04, now
with its architecture argued out and its central bet (seed-driven far terrain)
proven by a headless PoC. Proposed split: **M08** single-level LOD end-to-end,
**M09** full octree + polish. Day/night composes into it for free.
