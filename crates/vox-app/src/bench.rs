//! Benchmark mode (`--bench`): a fixed, repeatable performance run.
//!
//! Every milestone retro used to need the owner to fly by hand — stand here,
//! sprint there, at these settings, pasting log lines — and the numbers moved
//! with whatever else the machine was doing and wherever the flight went. This
//! runs one script instead: the same settings, the same places, the same
//! flight path, the same light, every time, then writes a table and quits.
//!
//! The script is eight phases: {6, 8 LOD levels} × {land, ocean} × {standing
//! 20 s, sprint-flight 30 s}. Before each phase the world is allowed to settle
//! — every streaming queue empty for a second, or 60 s at most — so a phase
//! measures that phase, not the previous one's backlog. The settle time is
//! reported too: it is how long the world took to catch up.
//!
//! Everything here is plain data and arithmetic; the app feeds it one
//! [`Sample`] per frame and does what each [`Tick`] says.

/// Where a phase runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The spawn: land.
    Land,
    /// The nearest deep water around spawn that stays deep along the whole
    /// sprint path.
    Ocean,
}

/// What the camera does during a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Still,
    /// Spectator sprint-flight, in a straight line along +X.
    Sprint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase {
    /// The settings menu's "extra LOD levels" (3 levels + this).
    pub extra_levels: i32,
    pub place: Place,
    pub motion: Motion,
    pub secs: f64,
}

impl Phase {
    pub fn label(&self) -> String {
        format!(
            "{} levels, {}, {}",
            3 + self.extra_levels,
            match self.place {
                Place::Land => "land",
                Place::Ocean => "ocean",
            },
            match self.motion {
                Motion::Still => "standing",
                Motion::Sprint => "sprint",
            }
        )
    }
}

/// Spectator sprint-flight speed, blocks per second (CLAUDE.md: 120 m/s).
pub const SPRINT_SPEED: f64 = 120.0;
/// How far a sprint phase flies — the length of path the ocean start must
/// keep deep water under.
pub const SPRINT_SECS: f64 = 30.0;
pub const STILL_SECS: f64 = 20.0;
/// The world counts as settled once every queue has been empty this long.
pub const SETTLE_QUIET_SECS: f64 = 1.0;
/// A phase starts anyway after this long, and the report says so.
pub const SETTLE_TIMEOUT_SECS: f64 = 60.0;
/// Camera pitch for every phase: a little below the horizon, so the view
/// holds near ground, the far LOD and the horizon at once.
pub const PITCH: f64 = -0.12;

/// The fixed script. Ordered so each place's standing phase settles the world
/// the sprint then starts from.
pub fn script() -> Vec<Phase> {
    let mut phases = Vec::new();
    for place in [Place::Land, Place::Ocean] {
        for extra_levels in [3, 5] {
            for (motion, secs) in [(Motion::Still, STILL_SECS), (Motion::Sprint, SPRINT_SECS)] {
                phases.push(Phase {
                    extra_levels,
                    place,
                    motion,
                    secs,
                });
            }
        }
    }
    phases
}

/// One frame's measurements.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// Raw frame time, milliseconds (unclamped).
    pub frame_ms: f32,
    pub dirty: usize,
    pub relight: usize,
    pub gen_in_flight: usize,
    pub lod_backlog: usize,
    pub lod_nodes: usize,
    pub gpu_mb: f32,
    pub tris: usize,
    pub loaded: usize,
    /// Running totals since startup, milliseconds — relight and mesh time
    /// never reset, so a phase's cost is the difference across it.
    pub relight_ms_total: f64,
    pub mesh_ms_total: f64,
}

impl Sample {
    /// Anything still queued: the world has not caught up yet.
    fn busy(&self) -> bool {
        self.dirty > 0 || self.relight > 0 || self.gen_in_flight > 0 || self.lod_backlog > 0
    }
}

/// One phase's results.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseStats {
    pub label: String,
    pub settle_secs: f64,
    pub settle_timed_out: bool,
    pub frames: usize,
    pub fps_median: f64,
    /// 1000 / the 99th-percentile frame time: the frame rate the slowest 1%
    /// of frames run at.
    pub fps_1pct_low: f64,
    pub worst_ms: f64,
    pub relight_ms_per_s: f64,
    pub mesh_ms_per_s: f64,
    pub dirty_peak: usize,
    pub dirty_end: usize,
    pub lod_nodes: usize,
    pub gpu_mb: f32,
    pub tris_median: usize,
    pub loaded_end: usize,
}

/// The `q` quantile (0..=1) of sorted values, nearest-rank.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

/// Summarise one phase's samples. `elapsed` is the phase's wall time.
pub fn summarize(
    label: String,
    settle_secs: f64,
    settle_timed_out: bool,
    samples: &[Sample],
    elapsed: f64,
) -> PhaseStats {
    let mut ms: Vec<f64> = samples.iter().map(|s| f64::from(s.frame_ms)).collect();
    ms.sort_by(f64::total_cmp);
    let mut tris: Vec<usize> = samples.iter().map(|s| s.tris).collect();
    tris.sort_unstable();
    let (first, last) = (
        samples.first().copied().unwrap_or_default(),
        samples.last().copied().unwrap_or_default(),
    );
    let per_s = |a: f64, b: f64| {
        if elapsed > 0.0 {
            (b - a) / elapsed
        } else {
            0.0
        }
    };
    let fps = |frame_ms: f64| {
        if frame_ms > 0.0 {
            1000.0 / frame_ms
        } else {
            0.0
        }
    };
    PhaseStats {
        label,
        settle_secs,
        settle_timed_out,
        frames: samples.len(),
        fps_median: fps(quantile(&ms, 0.5)),
        fps_1pct_low: fps(quantile(&ms, 0.99)),
        worst_ms: ms.last().copied().unwrap_or(0.0),
        relight_ms_per_s: per_s(first.relight_ms_total, last.relight_ms_total),
        mesh_ms_per_s: per_s(first.mesh_ms_total, last.mesh_ms_total),
        dirty_peak: samples.iter().map(|s| s.dirty).max().unwrap_or(0),
        dirty_end: last.dirty,
        lod_nodes: last.lod_nodes,
        gpu_mb: last.gpu_mb,
        tris_median: tris.get(tris.len() / 2).copied().unwrap_or(0),
        loaded_end: last.loaded,
    }
}

/// The results as a markdown table, under `header` lines (machine, world).
pub fn report(header: &[String], results: &[PhaseStats]) -> String {
    let mut out = String::from("# Voxterra benchmark\n\n");
    for line in header {
        out.push_str(&format!("- {line}\n"));
    }
    out.push_str(
        "\n| Phase | Settle s | fps median | 1% low | Worst ms | Relight ms/s | Mesh ms/s \
         | Dirty peak / end | LOD nodes | GPU MB | Tris M | Loaded |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in results {
        out.push_str(&format!(
            "| {} | {:.1}{} | {:.0} | {:.0} | {:.1} | {:.0} | {:.0} | {} / {} | {} | {:.0} | {:.2} | {} |\n",
            r.label,
            r.settle_secs,
            if r.settle_timed_out { " (timed out)" } else { "" },
            r.fps_median,
            r.fps_1pct_low,
            r.worst_ms,
            r.relight_ms_per_s,
            r.mesh_ms_per_s,
            r.dirty_peak,
            r.dirty_end,
            r.lod_nodes,
            r.gpu_mb,
            r.tris_median as f64 / 1.0e6,
            r.loaded_end,
        ));
    }
    out
}

/// What the app must do this frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tick {
    /// Apply this many extra LOD levels (a phase changed them).
    pub apply_levels: Option<i32>,
    /// Put the camera here — eye position — looking along +X at [`PITCH`].
    pub camera: Option<[f64; 3]>,
    /// The run is over: write this report and quit.
    pub done: Option<String>,
}

#[derive(Debug, Clone)]
enum State {
    /// Entering the phase at `index`.
    Enter,
    Settling {
        since: f64,
        quiet_since: Option<f64>,
    },
    Running {
        since: f64,
        settle_secs: f64,
        timed_out: bool,
    },
    Finished,
}

/// The benchmark's progress through the script.
pub struct Bench {
    phases: Vec<Phase>,
    index: usize,
    state: State,
    samples: Vec<Sample>,
    results: Vec<PhaseStats>,
    /// Eye positions where each place's phases start.
    land: [f64; 3],
    ocean: [f64; 3],
    header: Vec<String>,
    levels_applied: Option<i32>,
}

impl Bench {
    pub fn new(land: [f64; 3], ocean: [f64; 3], header: Vec<String>) -> Self {
        Self {
            phases: script(),
            index: 0,
            state: State::Enter,
            samples: Vec::new(),
            results: Vec::new(),
            land,
            ocean,
            header,
            levels_applied: None,
        }
    }

    fn start_of(&self, place: Place) -> [f64; 3] {
        match place {
            Place::Land => self.land,
            Place::Ocean => self.ocean,
        }
    }

    /// Advance by one frame. `now` is seconds since the run began.
    pub fn tick(&mut self, now: f64, sample: Sample) -> Tick {
        let mut out = Tick::default();
        let Some(&phase) = self.phases.get(self.index) else {
            return out;
        };
        let start = self.start_of(phase.place);
        match self.state.clone() {
            State::Enter => {
                if self.levels_applied != Some(phase.extra_levels) {
                    self.levels_applied = Some(phase.extra_levels);
                    out.apply_levels = Some(phase.extra_levels);
                }
                log::info!(
                    "bench: phase {}/{}: {} — settling",
                    self.index + 1,
                    self.phases.len(),
                    phase.label()
                );
                self.state = State::Settling {
                    since: now,
                    quiet_since: None,
                };
                out.camera = Some(start);
            }
            State::Settling { since, quiet_since } => {
                out.camera = Some(start);
                let quiet_since = if sample.busy() {
                    None
                } else {
                    Some(quiet_since.unwrap_or(now))
                };
                let settled = quiet_since.is_some_and(|q| now - q >= SETTLE_QUIET_SECS);
                let timed_out = now - since >= SETTLE_TIMEOUT_SECS;
                if settled || timed_out {
                    log::info!(
                        "bench: running {} ({:.1} s to settle)",
                        phase.label(),
                        now - since
                    );
                    self.samples.clear();
                    self.state = State::Running {
                        since: now,
                        settle_secs: now - since,
                        timed_out: timed_out && !settled,
                    };
                } else {
                    self.state = State::Settling { since, quiet_since };
                }
            }
            State::Running {
                since,
                settle_secs,
                timed_out,
            } => {
                let t = now - since;
                out.camera = Some(match phase.motion {
                    Motion::Still => start,
                    Motion::Sprint => [start[0] + SPRINT_SPEED * t, start[1], start[2]],
                });
                self.samples.push(sample);
                if t >= phase.secs {
                    let stats = summarize(phase.label(), settle_secs, timed_out, &self.samples, t);
                    log::info!(
                        "bench: {} — {:.0} fps median, {:.0} 1% low, worst {:.1} ms",
                        stats.label,
                        stats.fps_median,
                        stats.fps_1pct_low,
                        stats.worst_ms
                    );
                    self.results.push(stats);
                    self.index += 1;
                    if self.index == self.phases.len() {
                        self.state = State::Finished;
                        out.done = Some(report(&self.header, &self.results));
                    } else {
                        self.state = State::Enter;
                    }
                }
            }
            State::Finished => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(frame_ms: f32, dirty: usize) -> Sample {
        Sample {
            frame_ms,
            dirty,
            ..Default::default()
        }
    }

    #[test]
    fn the_script_covers_both_levels_places_and_motions_in_order() {
        let s = script();
        assert_eq!(s.len(), 8);
        assert_eq!(s[0].label(), "6 levels, land, standing");
        assert_eq!(s[1].label(), "6 levels, land, sprint");
        assert_eq!(s[3].label(), "8 levels, land, sprint");
        assert_eq!(s[7].label(), "8 levels, ocean, sprint");
        assert!(s.iter().all(|p| match p.motion {
            Motion::Still => p.secs == STILL_SECS,
            Motion::Sprint => p.secs == SPRINT_SECS,
        }));
    }

    #[test]
    fn statistics_are_nearest_rank() {
        // 100 frames: 98 at 2 ms, one at 10 ms, one at 50 ms.
        let mut v = vec![sample(2.0, 0); 98];
        v.push(sample(10.0, 7));
        v.push(sample(50.0, 3));
        let s = summarize("x".into(), 0.0, false, &v, 1.0);
        assert_eq!(s.frames, 100);
        assert!((s.fps_median - 500.0).abs() < 1e-9);
        assert!(
            (s.fps_1pct_low - 100.0).abs() < 1e-9,
            "99th percentile is the 10 ms frame"
        );
        assert_eq!(s.worst_ms, 50.0);
        assert_eq!((s.dirty_peak, s.dirty_end), (7, 3));
    }

    #[test]
    fn per_second_costs_are_the_difference_across_the_phase() {
        let mut a = sample(5.0, 0);
        a.relight_ms_total = 1000.0;
        a.mesh_ms_total = 2000.0;
        let mut b = a;
        b.relight_ms_total = 1300.0;
        b.mesh_ms_total = 2900.0;
        let s = summarize("x".into(), 0.0, false, &[a, b], 2.0);
        assert_eq!((s.relight_ms_per_s, s.mesh_ms_per_s), (150.0, 450.0));
    }

    /// Run the whole script against a world that settles after `busy_for`
    /// seconds of every phase, at 100 fps.
    fn run(busy_for: f64) -> (Bench, Vec<Tick>) {
        let mut b = Bench::new([0.0, 50.0, 0.0], [9000.0, 40.0, 0.0], vec!["test".into()]);
        let mut ticks = Vec::new();
        let mut entered = 0.0;
        for frame in 0..200_000 {
            let now = frame as f64 * 0.01;
            let settling = matches!(b.state, State::Enter | State::Settling { .. });
            if matches!(b.state, State::Enter) {
                entered = now;
            }
            let busy = settling && now - entered < busy_for;
            let t = b.tick(now, sample(10.0, usize::from(busy)));
            let done = t.done.is_some();
            ticks.push(t);
            if done {
                break;
            }
        }
        (b, ticks)
    }

    #[test]
    fn a_full_run_reports_every_phase_once_and_ends() {
        let (b, ticks) = run(2.0);
        let report = ticks
            .last()
            .and_then(|t| t.done.clone())
            .expect("the run ended");
        assert_eq!(b.results.len(), 8);
        for phase in script() {
            assert_eq!(report.matches(&format!("| {} |", phase.label())).count(), 1);
        }
        // Levels are applied only when they change: 6, 8, 6, 8.
        let applied: Vec<i32> = ticks.iter().filter_map(|t| t.apply_levels).collect();
        assert_eq!(applied, vec![3, 5, 3, 5]);
        // Each phase waited for the world: 2 s busy + 1 s quiet.
        assert!(b
            .results
            .iter()
            .all(|r| (r.settle_secs - 3.0).abs() < 0.02 && !r.settle_timed_out));
    }

    #[test]
    fn a_world_that_never_settles_times_out_and_says_so() {
        let (b, _) = run(f64::INFINITY);
        assert!(b.results.iter().all(|r| r.settle_timed_out));
        assert!(b
            .results
            .iter()
            .all(|r| r.settle_secs >= SETTLE_TIMEOUT_SECS));
    }

    #[test]
    fn a_sprint_flies_along_x_at_sprint_speed_from_its_start() {
        let (_, ticks) = run(0.0);
        // The first sprint's camera positions, after its settle.
        let xs: Vec<f64> = ticks
            .iter()
            .filter_map(|t| t.camera)
            .filter(|c| c[1] == 50.0)
            .map(|c| c[0])
            .collect();
        let far = xs.iter().copied().fold(0.0, f64::max);
        assert!(
            (far - SPRINT_SPEED * SPRINT_SECS).abs() < SPRINT_SPEED * 0.02,
            "flew {far}"
        );
        // Standing phases never move.
        assert!(ticks.iter().filter_map(|t| t.camera).all(|c| c[2] == 0.0));
    }
}
