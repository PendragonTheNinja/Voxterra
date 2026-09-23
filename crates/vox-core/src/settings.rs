//! Live-tunable settings (Milestone 09, amendment A3).
//!
//! Every visual decision in M09 cost a recompile-and-squint cycle: change a
//! constant, rebuild, fly out, judge a screenshot. These are the values that
//! actually needed tuning, gathered into one place so the in-game menu can move
//! them at runtime.
//!
//! The struct lives in `vox-core` rather than the app so the ranges and
//! defaults are testable without a window, and so the renderer can read them
//! without depending on the app.
//!
//! **Applying changes has a cost.** Fog and time are free (uniforms). Render
//! distance and LOD radii rebuild the streaming rings, so the caller decides
//! when to act on them — see [`Settings::rebuild_needed`].

/// Values the settings menu can change while the game runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    // --- View ---
    /// Full-resolution chunk radius.
    pub load_radius: i64,
    /// Extra coarse LOD levels appended beyond the built-in ones.
    ///
    /// Each extra level DOUBLES both its stride and its radius, so it covers
    /// twice the distance for about the same node count — view distance grows
    /// while cost grows only linearly in levels. (Scaling the radii *without*
    /// the strides instead is quadratic: ×4 distance turned 640 nodes into
    /// 10 240 and 4.3 GB of buffers. Hence levels, not a distance multiplier.)
    pub lod_extra_levels: i32,
    /// Draw coarse LOD terrain at all.
    pub lod_enabled: bool,
    /// Vertical field of view, degrees.
    pub fov_degrees: f32,

    // --- Fog ---
    pub fog_enabled: bool,
    /// Scale fog to the LOD horizon automatically.
    ///
    /// Fog distances are absolute blocks, but the horizon moves when LOD levels
    /// change — so a fog tuned for a 2 km horizon hides almost everything at
    /// 8 km, and one tuned for 8 km leaves no fog at all at 2 km. In auto mode
    /// the start/end are fractions of the current horizon, so the fade always
    /// lands in the right place.
    pub fog_auto: bool,
    /// Auto mode: fraction of the LOD horizon where fog begins.
    pub fog_start_frac: f32,
    /// Auto mode: fraction of the LOD horizon where fog is full.
    pub fog_end_frac: f32,
    /// Distance (blocks) where fog begins.
    pub fog_start: f32,
    /// Distance (blocks) where fog reaches full strength.
    pub fog_end: f32,
    /// 0 = off, 1 = full.
    pub fog_strength: f32,

    // --- Geomorph (ADR-0009) ---
    /// Width in blocks of the band before each LOD ring boundary over which
    /// that level's terrain morphs into the next coarser level's silhouette.
    ///
    /// Too narrow and the morph itself reads as a ripple sweeping the ground;
    /// too wide and near terrain is flattened toward coarse detail long before
    /// it needs to be. 0 disables morphing.
    ///
    /// Measured from the camera, while the handover radius is measured from the
    /// ring's snapped centre — which can sit up to a coarsest-stride (256
    /// blocks) away. A band narrower than that offset can leave the morph
    /// incomplete when the swap fires, so the useful range starts wider than
    /// intuition suggests.
    pub lod_morph_band: f32,

    // --- Time ---
    /// Freeze the day/night cycle.
    pub time_paused: bool,
    /// Real seconds per game day.
    pub day_length_secs: f32,
    /// Time-of-day scrub, 0..1 (0 = midnight, 0.5 = noon). Only applied when
    /// the caller sees it change, so it doesn't fight the running clock.
    pub time_of_day: f32,

    // --- Sky ---
    /// Star brightness multiplier.
    pub star_intensity: f32,

    // --- Debug ---
    /// Show the telemetry overlay.
    pub show_telemetry: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            load_radius: 8,
            lod_extra_levels: 0,
            lod_enabled: true,
            fov_degrees: 70.0,
            fog_enabled: true,
            fog_auto: true,
            fog_start_frac: 0.20,
            fog_end_frac: 0.98,
            fog_start: 420.0,
            fog_end: 2000.0,
            fog_strength: 1.0,
            lod_morph_band: 192.0,
            time_paused: false,
            day_length_secs: 24.0 * 60.0,
            time_of_day: 0.3,
            star_intensity: 1.0,
            show_telemetry: true,
        }
    }
}

/// Inclusive slider bounds, kept beside the field they describe so the menu and
/// the clamping cannot drift apart.
pub mod ranges {
    pub const LOAD_RADIUS: (i64, i64) = (2, 24);
    /// Capped at 2 deliberately. The LOD ring requires every boundary radius to
    /// be a multiple of the COARSEST stride (that is what keeps the levels
    /// exactly partitioned), so adding very coarse levels drags the FINE
    /// levels' radii outward too — and their node count is quadratic in radius.
    /// Measured: 0 → 640 nodes / 2 km, 1 → 832 / 4 km, 2 → 2416 / 8 km,
    /// 3 → 9196 / 16 km (which is where memory becomes a problem). Lifting this
    /// needs a partition that doesn't tie fine radii to the coarsest grid.
    pub const LOD_EXTRA_LEVELS: (i32, i32) = (0, 2);
    pub const FOV_DEGREES: (f32, f32) = (30.0, 120.0);
    pub const FOG_START: (f32, f32) = (0.0, 8000.0);
    pub const FOG_END: (f32, f32) = (16.0, 16000.0);
    pub const FOG_STRENGTH: (f32, f32) = (0.0, 1.0);
    pub const FOG_FRAC: (f32, f32) = (0.0, 1.0);
    pub const LOD_MORPH_BAND: (f32, f32) = (0.0, 512.0);
    pub const DAY_LENGTH_SECS: (f32, f32) = (10.0, 86_400.0);
    pub const STAR_INTENSITY: (f32, f32) = (0.0, 3.0);
}

impl Settings {
    /// Clamp every value into its valid range and repair invariants.
    ///
    /// Called after any edit: a slider can be dragged to an extreme, and text
    /// entry (which egui allows) can put anything in the field. Nothing
    /// downstream should have to defend itself against a negative radius or a
    /// fog range that ends before it starts.
    pub fn sanitize(&mut self) {
        self.load_radius = self
            .load_radius
            .clamp(ranges::LOAD_RADIUS.0, ranges::LOAD_RADIUS.1);
        self.lod_extra_levels = self
            .lod_extra_levels
            .clamp(ranges::LOD_EXTRA_LEVELS.0, ranges::LOD_EXTRA_LEVELS.1);
        self.fov_degrees = self
            .fov_degrees
            .clamp(ranges::FOV_DEGREES.0, ranges::FOV_DEGREES.1);
        self.fog_start = self
            .fog_start
            .clamp(ranges::FOG_START.0, ranges::FOG_START.1);
        self.fog_end = self.fog_end.clamp(ranges::FOG_END.0, ranges::FOG_END.1);
        // Fog must not end before it starts, or the ramp divides by a negative
        // and distant terrain inverts.
        if self.fog_end <= self.fog_start {
            self.fog_end = self.fog_start + 16.0;
        }
        self.fog_strength = self
            .fog_strength
            .clamp(ranges::FOG_STRENGTH.0, ranges::FOG_STRENGTH.1);
        self.lod_morph_band = self
            .lod_morph_band
            .clamp(ranges::LOD_MORPH_BAND.0, ranges::LOD_MORPH_BAND.1);
        self.fog_start_frac = self
            .fog_start_frac
            .clamp(ranges::FOG_FRAC.0, ranges::FOG_FRAC.1);
        self.fog_end_frac = self
            .fog_end_frac
            .clamp(ranges::FOG_FRAC.0, ranges::FOG_FRAC.1);
        if self.fog_end_frac <= self.fog_start_frac {
            self.fog_end_frac = (self.fog_start_frac + 0.05).min(1.0);
        }
        self.day_length_secs = self
            .day_length_secs
            .clamp(ranges::DAY_LENGTH_SECS.0, ranges::DAY_LENGTH_SECS.1);
        self.star_intensity = self
            .star_intensity
            .clamp(ranges::STAR_INTENSITY.0, ranges::STAR_INTENSITY.1);
        self.time_of_day = self.time_of_day.rem_euclid(1.0);
    }

    /// Does moving from `old` to `self` require rebuilding the streaming rings?
    ///
    /// Fog, time and stars are uniforms — free to change every frame. Radius
    /// changes re-request the world, so the caller applies them deliberately
    /// rather than on every slider pixel.
    pub fn rebuild_needed(&self, old: &Settings) -> bool {
        self.load_radius != old.load_radius
            || self.lod_extra_levels != old.lod_extra_levels
            || self.lod_enabled != old.lod_enabled
    }

    /// Fog start/end in blocks for the current LOD horizon. In auto mode these
    /// are fractions of the horizon, so the fade follows the view distance
    /// instead of having to be re-tuned every time LOD levels change.
    pub fn fog_range(&self, lod_horizon_blocks: f32) -> (f32, f32) {
        if self.fog_auto {
            let start = lod_horizon_blocks * self.fog_start_frac;
            let end = lod_horizon_blocks * self.fog_end_frac;
            (start, end.max(start + 16.0))
        } else {
            (self.fog_start, self.fog_end)
        }
    }

    /// Effective fog strength (0 when fog is switched off), for the renderer.
    pub fn effective_fog_strength(&self) -> f32 {
        if self.fog_enabled {
            self.fog_strength
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_already_valid() {
        let mut s = Settings::default();
        let before = s;
        s.sanitize();
        assert_eq!(s, before, "defaults should survive sanitize unchanged");
    }

    #[test]
    fn sanitize_clamps_out_of_range_values() {
        let mut s = Settings {
            load_radius: 9999,
            lod_extra_levels: 99,
            fov_degrees: -10.0,
            fog_strength: 5.0,
            star_intensity: -1.0,
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.load_radius, ranges::LOAD_RADIUS.1);
        assert_eq!(s.lod_extra_levels, ranges::LOD_EXTRA_LEVELS.1);
        assert_eq!(s.fov_degrees, ranges::FOV_DEGREES.0);
        assert_eq!(s.fog_strength, ranges::FOG_STRENGTH.1);
        assert_eq!(s.star_intensity, ranges::STAR_INTENSITY.0);
    }

    /// Fog ending before it starts would divide by a negative span and invert
    /// the ramp — distant terrain fading the wrong way.
    #[test]
    fn sanitize_repairs_inverted_fog_range() {
        let mut s = Settings {
            fog_start: 3000.0,
            fog_end: 100.0,
            ..Default::default()
        };
        s.sanitize();
        assert!(s.fog_end > s.fog_start, "fog range still inverted");
    }

    #[test]
    fn time_of_day_wraps_rather_than_clamping() {
        let mut s = Settings {
            time_of_day: 1.25,
            ..Default::default()
        };
        s.sanitize();
        assert!((s.time_of_day - 0.25).abs() < 1e-6);
        let mut s2 = Settings {
            time_of_day: -0.25,
            ..Default::default()
        };
        s2.sanitize();
        assert!((s2.time_of_day - 0.75).abs() < 1e-6);
    }

    /// Only the expensive settings ask for a rebuild; the free ones must not,
    /// or dragging the fog slider would re-stream the world every frame.
    #[test]
    fn only_expensive_changes_request_a_rebuild() {
        let base = Settings::default();

        let mut cheap = base;
        cheap.fog_start = 100.0;
        cheap.fog_strength = 0.4;
        cheap.time_of_day = 0.9;
        cheap.star_intensity = 2.0;
        cheap.fov_degrees = 100.0;
        assert!(!cheap.rebuild_needed(&base), "cheap edits must not rebuild");

        let mut radius = base;
        radius.load_radius = 12;
        assert!(radius.rebuild_needed(&base));

        let mut levels = base;
        levels.lod_extra_levels = 2;
        assert!(levels.rebuild_needed(&base));

        let mut toggled = base;
        toggled.lod_enabled = false;
        assert!(toggled.rebuild_needed(&base));
    }

    /// Auto fog must follow the horizon: the same settings produce a
    /// proportionally further fade when LOD levels extend the view. Without
    /// this, fog tuned at one horizon is wrong at every other.
    #[test]
    fn auto_fog_scales_with_the_horizon() {
        let s = Settings::default();
        assert!(s.fog_auto);
        let (near_start, near_end) = s.fog_range(2048.0);
        let (far_start, far_end) = s.fog_range(8192.0);
        assert!(far_start > near_start * 3.5, "start did not follow horizon");
        assert!(far_end > near_end * 3.5, "end did not follow horizon");
        assert!(near_end > near_start && far_end > far_start);
    }

    /// Manual mode still uses the absolute sliders.
    #[test]
    fn manual_fog_ignores_the_horizon() {
        let s = Settings {
            fog_auto: false,
            fog_start: 100.0,
            fog_end: 900.0,
            ..Default::default()
        };
        assert_eq!(s.fog_range(2048.0), (100.0, 900.0));
        assert_eq!(s.fog_range(16384.0), (100.0, 900.0));
    }

    #[test]
    fn disabling_fog_zeroes_its_strength() {
        let mut s = Settings::default();
        assert!(s.effective_fog_strength() > 0.0);
        s.fog_enabled = false;
        assert_eq!(s.effective_fog_strength(), 0.0);
    }
}
