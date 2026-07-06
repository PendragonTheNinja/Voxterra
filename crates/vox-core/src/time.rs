//! World time and the celestial model (M07).
//!
//! [`WorldTime`] is the single source of truth for time of day. It is a bare
//! `u64` tick counter where **one tick is one game-second** — a full game day
//! is [`TICKS_PER_DAY`] = 86 400 ticks (24 game-hours × 3600). Everything the
//! renderer needs — sun direction, sky brightness, moon phase — is a **pure
//! function of a `WorldTime`** (see the methods below). No time state lives in
//! the render loop; the loop only advances the counter and reads these.
//!
//! ## Why "tick = game-second"
//!
//! It makes the two speed modes fall out cleanly instead of needing special
//! cases:
//!
//! - **Default speed** ([`DEFAULT_DAY_LENGTH_SECS`], 24 real minutes/day): the
//!   app adds 60 ticks per real second, i.e. 1 game-minute per real-second.
//! - **Real-time-sync** (a future hardcore world option: in-game clock tracks
//!   the player's wall clock): the app adds 1 tick per real second and seeds
//!   the counter from the local wall-clock epoch. Because day length and
//!   lunation are **independent tunable constants** and `WorldTime` is just a
//!   counter, real-time sync needs no engine change — only different advance
//!   rate + a start offset. Keep that property: never couple these functions
//!   to real-world time or assume a fixed day length.
//!
//! `WorldTime` is trivially serializable (one integer) and will enter the save
//! format, versioned, when that milestone lands.
//!
//! ## What this module deliberately does NOT do
//!
//! It does not touch light *propagation*. Day/night dims the **sky light
//! channel at the shader** via [`WorldTime::sky_scale`]; the 3D skylight
//! propagation and the geometric ambient floor (M05/M06) are untouched. The
//! two darknesses stay separate: "no light reaches here" is the constant 0.035
//! ambient floor baked into the shader's `light_curve`; "the sky is dim right
//! now" is this time-of-day scale applied on top. See
//! `docs/milestones/07-day-night.md` and ADR-0007.

use core::f32::consts::TAU;

/// Game-seconds in one full day (24 game-hours). One [`WorldTime`] tick is one
/// game-second, so this is also ticks-per-day.
pub const TICKS_PER_DAY: u64 = 24 * 3600;

/// Length of a full moon cycle (new → full → new), in game-days. Shorter than
/// the real synodic month (~29.5 days) on purpose: the moon modulates night
/// brightness, and that mechanic only teaches the player if the phase visibly
/// changes within a session. A real-time-sync world would override this with
/// the true value.
pub const LUNATION_DAYS: u64 = 8;

/// Default real-world seconds per game day at normal speed (24 real minutes).
/// This is an app-side pacing default recorded here so it is not lost; the core
/// time functions never read it (they are frame-rate and speed independent).
pub const DEFAULT_DAY_LENGTH_SECS: u32 = 24 * 60;

/// Sky brightness at midnight under a new (dark) moon — the darkest the open
/// sky ever gets. With the decoupled lighting (curve each source, then dim sky
/// by this), a fully sky-exposed field at new-moon midnight renders at roughly
/// this value, so it sits faintly above the almost-black ambient floor a sealed
/// cave gets. Lower toward the ambient floor for darker moonless nights.
pub const NIGHT_SKY_MIN: f32 = 0.010;

/// Sky brightness at midnight under a full moon. A few percent — enough that an
/// open field at full-moon midnight reads *above* the sealed-cave ambient floor,
/// because moonlit sky-reach sits on top of that floor rather than replacing it.
/// (M07 task 4: tuned down 0.08 → 0.065 → 0.04 so nights read genuinely dark;
/// new moon bottoms out at the geometric ambient floor and cannot go lower.)
pub const NIGHT_SKY_FULL_MOON: f32 = 0.04;

/// Sun elevation half-width of the dawn/dusk transition band. The sky crosses
/// from full night to full day as the sun's elevation sweeps from `-TWILIGHT`
/// to `+TWILIGHT` (smoothstep). Wider = longer, gentler twilight.
const TWILIGHT: f32 = 0.2;

/// Game ticks (game-seconds) that elapse per real second for a given day
/// length. The app multiplies real frame `dt` by this to advance [`WorldTime`].
///
/// Encodes the speed contract: at the default 24-real-minute day
/// ([`DEFAULT_DAY_LENGTH_SECS`] = 1440 s) this is 60 — 60 game-seconds per real
/// second, i.e. 1 game-minute per real second. A real-time-sync world uses a
/// day length of 86 400 real seconds, giving exactly 1.
#[inline]
pub fn game_ticks_per_second(day_length_secs: f64) -> f64 {
    TICKS_PER_DAY as f64 / day_length_secs
}

/// A point in world time. One tick is one game-second; see the module docs.
///
/// A bare counter by design: cheap to copy, trivially serializable, and
/// seedable from any epoch (including the wall clock, for a real-time-sync
/// world).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorldTime {
    ticks: u64,
}

impl WorldTime {
    /// Midnight of day zero.
    pub const ZERO: WorldTime = WorldTime { ticks: 0 };

    #[inline]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self { ticks }
    }

    #[inline]
    pub const fn ticks(self) -> u64 {
        self.ticks
    }

    /// Advance time by `dt_ticks` game-seconds (saturating; a world never runs
    /// out of time).
    #[inline]
    pub const fn advance(self, dt_ticks: u64) -> Self {
        Self {
            ticks: self.ticks.saturating_add(dt_ticks),
        }
    }

    /// Which game-day this is (0-based).
    #[inline]
    pub const fn day(self) -> u64 {
        self.ticks / TICKS_PER_DAY
    }

    /// Fraction of the current day elapsed, in `[0.0, 1.0)`. `0.0` is midnight,
    /// `0.25` sunrise, `0.5` noon, `0.75` sunset.
    #[inline]
    pub fn time_of_day(self) -> f32 {
        (self.ticks % TICKS_PER_DAY) as f32 / TICKS_PER_DAY as f32
    }

    /// Total elapsed game-days as a continuous value (integer day + fraction).
    /// Used for the moon, which advances smoothly across day boundaries.
    #[inline]
    fn day_continuous(self) -> f64 {
        self.ticks as f64 / TICKS_PER_DAY as f64
    }

    /// Unit vector pointing from the world toward the sun. Right-handed, Y-up.
    ///
    /// The sun arcs due east → overhead → due west in the X–Y plane (no
    /// north–south tilt yet — a latitude tilt is a future refinement and would
    /// change only this function):
    ///
    /// - midnight (`0.0`): straight down `(0, -1, 0)`
    /// - sunrise (`0.25`): due east `(+1, 0, 0)`
    /// - noon (`0.5`): straight up `(0, +1, 0)`
    /// - sunset (`0.75`): due west `(-1, 0, 0)`
    ///
    /// Everything that needs "where is the sun" — the sky gradient, the sun
    /// disc, the elevation that drives [`sky_scale`](Self::sky_scale) — derives
    /// from this one vector so they can never disagree.
    #[inline]
    pub fn sun_direction(self) -> [f32; 3] {
        let a = self.time_of_day() * TAU;
        [a.sin(), -a.cos(), 0.0]
    }

    /// Sun elevation in `[-1, 1]`: `+1` at the zenith (noon), `0` at the
    /// horizon (sunrise/sunset), `-1` straight down (midnight). This is just
    /// the Y component of [`sun_direction`](Self::sun_direction), named for the
    /// callers that only care about height above the horizon.
    #[inline]
    pub fn sun_elevation(self) -> f32 {
        -(self.time_of_day() * TAU).cos()
    }

    /// Moon phase in `[0.0, 1.0)`: `0.0` new moon, `0.5` full moon, wrapping
    /// smoothly. Advances continuously across day boundaries over
    /// [`LUNATION_DAYS`].
    #[inline]
    pub fn moon_phase(self) -> f32 {
        (self.day_continuous() / LUNATION_DAYS as f64).fract() as f32
    }

    /// Unit vector pointing from the world toward the moon.
    ///
    /// The moon rides the same arc plane as the sun but **drifts relative to
    /// it** over the lunation: it is the sun direction rotated about the arc's
    /// normal (Z) by `moon_phase × 2π`. This makes the geometry produce correct
    /// phases and correct placement at once:
    ///
    /// - new moon (`phase 0`): moon ≈ sun — near the sun, up in daytime, its
    ///   lit face turned away, so the night is dark.
    /// - full moon (`phase 0.5`): moon opposite the sun — high at local
    ///   midnight, fully lit.
    /// - quarters: 90° from the sun.
    ///
    /// The renderer lights the moon disc with the real [`sun_direction`], so it
    /// never needs a separate "phase" input — the terminator is a consequence
    /// of where the moon and sun are. (ADR-0007.)
    #[inline]
    pub fn moon_direction(self) -> [f32; 3] {
        let a = self.moon_phase() * TAU;
        let [sx, sy, sz] = self.sun_direction();
        // Rotate the sun direction about the Z axis (the normal of the E–W arc
        // plane) by the phase angle.
        let (sin_a, cos_a) = (a.sin(), a.cos());
        [sx * cos_a - sy * sin_a, sx * sin_a + sy * cos_a, sz]
    }

    /// Fraction of the moon's disc that is lit, in `[0.0, 1.0]`: `0` new,
    /// `1` full, `0.5` at the quarters. A smooth cosine of the phase — this is
    /// what scales the night sky brightness and (in the shader) the moon's
    /// glow.
    #[inline]
    pub fn moon_illumination(self) -> f32 {
        0.5 * (1.0 - (self.moon_phase() * TAU).cos())
    }

    /// Multiplier applied to the **sky light channel** for this time of day, in
    /// `[NIGHT_SKY_MIN .. 1.0]`. Full daylight is `1.0`; night falls to a
    /// moon-dependent floor between [`NIGHT_SKY_MIN`] (new moon) and
    /// [`NIGHT_SKY_FULL_MOON`] (full moon). Dawn and dusk are a smooth
    /// [`TWILIGHT`]-wide transition as the sun crosses the horizon.
    ///
    /// This scales *only* sky light. Block light (torches) is never touched by
    /// time of day — that separation is the whole point of the two-channel
    /// vertex light (ADR-0007). The result multiplies the sky channel; the
    /// geometric ambient floor still applies underneath in the shader's
    /// `light_curve`, so this can dim the sky to zero without a cave getting
    /// any darker than its constant floor.
    #[inline]
    pub fn sky_scale(self) -> f32 {
        let day_factor = smoothstep(-TWILIGHT, TWILIGHT, self.sun_elevation());
        let night_floor =
            NIGHT_SKY_MIN + self.moon_illumination() * (NIGHT_SKY_FULL_MOON - NIGHT_SKY_MIN);
        lerp(night_floor, 1.0, day_factor)
    }
}

/// Hermite smoothstep: `0` for `x <= edge0`, `1` for `x >= edge1`, smooth in
/// between. Matches the GPU `smoothstep` so CPU-side tuning agrees with the
/// shader.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks for a given fraction of day zero — keeps the truth tables readable.
    fn at(fraction: f64) -> WorldTime {
        WorldTime::from_ticks((fraction * TICKS_PER_DAY as f64) as u64)
    }

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "expected ~{b}, got {a}");
    }

    fn approx3(a: [f32; 3], b: [f32; 3]) {
        for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate() {
            assert!(
                (av - bv).abs() < 1e-3,
                "component {i}: expected ~{bv}, got {av}"
            );
        }
    }

    #[test]
    fn time_of_day_landmarks() {
        approx(WorldTime::ZERO.time_of_day(), 0.0);
        approx(at(0.25).time_of_day(), 0.25);
        approx(at(0.5).time_of_day(), 0.5);
        approx(at(0.75).time_of_day(), 0.75);
        // Wraps: exactly one day in is midnight of day 1.
        approx(WorldTime::from_ticks(TICKS_PER_DAY).time_of_day(), 0.0);
    }

    #[test]
    fn day_counter_advances() {
        assert_eq!(WorldTime::ZERO.day(), 0);
        assert_eq!(WorldTime::from_ticks(TICKS_PER_DAY - 1).day(), 0);
        assert_eq!(WorldTime::from_ticks(TICKS_PER_DAY).day(), 1);
        assert_eq!(WorldTime::from_ticks(3 * TICKS_PER_DAY + 5).day(), 3);
    }

    #[test]
    fn ticks_per_second_speed_contract() {
        // Default 24-min day: 60 game-seconds per real second (1 game-minute
        // per real second).
        approx(
            game_ticks_per_second(DEFAULT_DAY_LENGTH_SECS as f64) as f32,
            60.0,
        );
        // Real-time sync: a full real day per game day -> exactly 1:1.
        approx(game_ticks_per_second(86_400.0) as f32, 1.0);
        // A 12-minute day runs twice as fast as default.
        approx(game_ticks_per_second(12.0 * 60.0) as f32, 120.0);
    }

    #[test]
    fn one_tick_is_one_game_second() {
        // The defining property of the tick unit.
        assert_eq!(TICKS_PER_DAY, 86_400);
        // A game hour is 3600 ticks; 6:00 (sunrise) is a quarter day.
        assert_eq!(6 * 3600, TICKS_PER_DAY / 4);
    }

    #[test]
    fn advance_is_saturating() {
        let t = WorldTime::from_ticks(u64::MAX - 2);
        assert_eq!(t.advance(10).ticks(), u64::MAX);
    }

    #[test]
    fn sun_direction_landmarks() {
        approx3(WorldTime::ZERO.sun_direction(), [0.0, -1.0, 0.0]); // midnight: down
        approx3(at(0.25).sun_direction(), [1.0, 0.0, 0.0]); // sunrise: east
        approx3(at(0.5).sun_direction(), [0.0, 1.0, 0.0]); // noon: up
        approx3(at(0.75).sun_direction(), [-1.0, 0.0, 0.0]); // sunset: west
    }

    #[test]
    fn sun_direction_is_unit_length() {
        for i in 0..24 {
            let d = at(i as f64 / 24.0).sun_direction();
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            approx(len, 1.0);
        }
    }

    #[test]
    fn sun_elevation_matches_direction_y() {
        for i in 0..24 {
            let t = at(i as f64 / 24.0);
            approx(t.sun_elevation(), t.sun_direction()[1]);
        }
    }

    #[test]
    fn moon_phase_quarters() {
        // New at day 0, full at half a lunation, back to new after a full one.
        approx(WorldTime::ZERO.moon_phase(), 0.0);
        approx(at(LUNATION_DAYS as f64 / 2.0).moon_phase(), 0.5);
        approx(at(LUNATION_DAYS as f64 / 4.0).moon_phase(), 0.25);
        // A whole lunation wraps back to new.
        approx(at(LUNATION_DAYS as f64).moon_phase(), 0.0);
    }

    #[test]
    fn moon_direction_placement() {
        // New moon (day 0 midnight): moon coincides with the sun (both down).
        approx3(
            WorldTime::ZERO.moon_direction(),
            WorldTime::ZERO.sun_direction(),
        );

        // Full moon = half a lunation later. At that day's local midnight the
        // sun is straight down, so the moon must be straight up.
        let full_midnight = at(LUNATION_DAYS as f64 / 2.0);
        approx(full_midnight.time_of_day(), 0.0); // midnight
        approx3(full_midnight.moon_direction(), [0.0, 1.0, 0.0]);

        // Full moon is opposite the sun in the sky.
        let s = full_midnight.sun_direction();
        let m = full_midnight.moon_direction();
        approx3(m, [-s[0], -s[1], -s[2]]);
    }

    #[test]
    fn moon_direction_is_unit_length() {
        for i in 0..40 {
            let d = at(i as f64 / 5.0).moon_direction(); // spans several lunations
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            approx(len, 1.0);
        }
    }

    #[test]
    fn moon_illumination_endpoints() {
        approx(WorldTime::ZERO.moon_illumination(), 0.0); // new: dark
        approx(at(LUNATION_DAYS as f64 / 2.0).moon_illumination(), 1.0); // full: lit
        approx(at(LUNATION_DAYS as f64 / 4.0).moon_illumination(), 0.5); // quarter
    }

    #[test]
    fn sky_scale_full_daylight_at_noon() {
        approx(at(0.5).sky_scale(), 1.0);
    }

    #[test]
    fn sky_scale_new_moon_midnight_is_floor() {
        // New moon: midnight sky is the minimum (near-black).
        approx(WorldTime::ZERO.sky_scale(), NIGHT_SKY_MIN);
    }

    #[test]
    fn sky_scale_full_moon_midnight_beats_cave_floor() {
        // Full-moon midnight sky must lift the open sky above a new-moon
        // midnight (moonlight brightens the night), and stay positive so an
        // open field reads brighter than a sealed cave (which sits at the
        // geometric ambient floor). Comparing phases keeps this independent of
        // the exact ambient value (defined in the mesher/shader).
        let full = at(LUNATION_DAYS as f64 / 2.0); // midnight of the full-moon day
        approx(full.time_of_day(), 0.0);
        approx(full.sky_scale(), NIGHT_SKY_FULL_MOON);
        assert!(
            full.sky_scale() > WorldTime::ZERO.sky_scale(),
            "full-moon night ({}) must beat new-moon night ({})",
            full.sky_scale(),
            WorldTime::ZERO.sky_scale()
        );
        assert!(full.sky_scale() > 0.0);
    }

    #[test]
    fn sky_scale_is_monotonic_through_dawn() {
        // From pre-dawn to noon the sky only brightens (no dips in the curve).
        let mut prev = -1.0;
        for i in 0..=50 {
            let frac = 0.5 * (i as f64 / 50.0); // midnight → noon
            let s = at(frac).sky_scale();
            assert!(
                s >= prev - 1e-4,
                "sky_scale dipped at frac {frac}: {s} < {prev}"
            );
            prev = s;
        }
    }

    #[test]
    fn sky_scale_bounded() {
        for i in 0..200 {
            let s = at(i as f64 / 200.0).sky_scale();
            assert!(
                (NIGHT_SKY_MIN..=1.0).contains(&s),
                "sky_scale out of range: {s}"
            );
        }
    }
}
