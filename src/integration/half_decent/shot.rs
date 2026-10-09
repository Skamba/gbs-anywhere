//! Finds the shot in the scale's readings: from the first drops to the flow
//! stopping, and the weight in the cup. No I/O: readings carry the scale's
//! own clock, so network delays don't change the time.
//!
//! Every reading goes through a median of three first, which removes the
//! single-reading spikes the scale shows (up to ±0.45 g on an empty scale).
//! Then:
//!
//! 1. **Settling**: wait for half a second of steady weight; that is the
//!    baseline (the cup, or the empty scale).
//! 2. **Ready**: a jump of more than 10 g within 0.3 s is a cup placed or
//!    lifted: settle again. The first drops are 1 g over the baseline, held
//!    for a second; the shot starts at the last reading still at the
//!    baseline.
//! 3. **Flowing**: the flow has stopped once the weight rose less than
//!    0.5 g in 3 s, with at least 5 g in the cup, or half the recipe weight
//!    when the grinder sent one ([`Detector::expecting`]), so a pause after
//!    pre-infusion is not taken for the end. It ended in the middle of the
//!    last second in which the weight still rose 0.6 g. A drop of more than
//!    10 g is the cup being lifted: the shot ends there.
//!
//! A stop under 5 s or 5 g is a pause, not the end: keep watching from the
//! same start. Weight that goes back to the baseline was a hand, not coffee.
//!
//! [`Detector::after_tare`] is for a detector started together with a tare:
//! it waits for the tare to land (a reading near zero) so the jump to zero
//! is not taken for the first drops.

use std::collections::VecDeque;
use std::time::Duration;

/// One reading: the scale's uptime and the weight on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub ms: u64,
    pub grams: f64,
}

/// A finished shot: first drops to flow stop, and the coffee in the cup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub time: Duration,
    /// Rounded to 0.1 g, the scale's resolution.
    pub grams: f64,
}

/// Where the detector is after a reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    /// Waiting for a steady weight to measure from.
    Settling,
    /// Steady; waiting for the first drops.
    Ready,
    /// Coffee is flowing: grams in the cup so far.
    Flowing { grams: f64 },
    /// The flow stopped or the cup was lifted. Start a new detector for the
    /// next shot.
    Done(Shot),
}

/// Steady for this long before measuring from it.
const SETTLE_MS: u64 = 500;
/// ... within this much of the median.
const SETTLE_SPREAD_G: f64 = 0.3;
/// A change this big this fast is a cup placed or lifted, not coffee.
const STEP_MS: u64 = 300;
const STEP_G: f64 = 10.0;
/// Below the baseline by this much: something was taken off; settle again.
const BELOW_BASELINE_G: f64 = 1.0;
/// Still "at the baseline" up to this much above it.
const AT_BASELINE_G: f64 = 0.3;
/// First drops: this much over the baseline ...
const DROPS_G: f64 = 1.0;
/// ... held this long, not falling back under `DROPS_RESET_G`.
const DROPS_HOLD_MS: u64 = 1000;
const DROPS_RESET_G: f64 = 0.5;
/// Flow stopped: less than `STOP_RISE_G` more over `STOP_WINDOW_MS` ...
const STOP_WINDOW_MS: u64 = 3000;
const STOP_RISE_G: f64 = 0.5;
/// ... once at least this much is in the cup (or `RECIPE_SHARE` of the
/// recipe weight, if more).
const MIN_SHOT_G: f64 = 5.0;
const RECIPE_SHARE: f64 = 0.5;
/// Still flowing while it rises `FLOW_RISE_G` within `FLOW_WINDOW_MS`.
const FLOW_WINDOW_MS: u64 = 1000;
const FLOW_RISE_G: f64 = 0.6;
/// When the cup is lifted, readings this close before the drop are the hand
/// on the cup, not coffee.
const LIFT_GUARD_MS: u64 = 800;
/// Shorter than this is not a shot.
const MIN_SHOT_MS: u64 = 5000;
/// How much history is kept outside a shot, and at most (10 min at 10 Hz).
const KEEP_MS: u64 = STOP_WINDOW_MS + 2000;
const KEEP_MAX: usize = 6000;
/// After a tare: the tare has landed once a reading is this close to zero ...
const TARE_ZERO_G: f64 = 0.5;
/// ... or this long after the first reading, whatever the scale shows.
const TARE_MAX_MS: u64 = 3000;

/// Finds one shot. See the module docs.
#[derive(Debug)]
pub struct Detector {
    /// The last three readings as they came, for the median.
    raw: VecDeque<Reading>,
    /// Median-filtered readings, oldest first.
    history: VecDeque<Reading>,
    state: State,
    /// Waiting for a tare to land: `Some(first reading's ms)` once one came.
    tare: Option<Option<u64>>,
    /// The flow can only stop with this much in the cup.
    min_grams: f64,
}

impl Default for Detector {
    fn default() -> Self {
        Self {
            raw: VecDeque::new(),
            history: VecDeque::new(),
            state: State::default(),
            tare: None,
            min_grams: MIN_SHOT_G,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
enum State {
    #[default]
    Settling,
    Ready {
        baseline: f64,
        /// The last reading still at the baseline: where the shot starts.
        at_baseline_ms: u64,
        /// Since when the weight has been over `DROPS_G`.
        above_since: Option<u64>,
    },
    Flowing {
        baseline: f64,
        start_ms: u64,
    },
    Done,
}

impl Detector {
    pub fn new() -> Self {
        Self::default()
    }

    /// With the grinder's recipe weight: the flow only stops once half of
    /// it is in the cup.
    pub fn expecting(mut self, recipe_g: Option<f64>) -> Self {
        let share = recipe_g.filter(|g| g.is_finite()).unwrap_or(0.0) * RECIPE_SHARE;
        self.min_grams = MIN_SHOT_G.max(share);
        self
    }

    fn with_min_grams(mut self, g: f64) -> Self {
        self.min_grams = g;
        self
    }

    /// For a detector started together with a tare: readings count only
    /// from the first one near zero (at most `TARE_MAX_MS` later).
    pub fn after_tare() -> Self {
        Self {
            tare: Some(None),
            ..Self::default()
        }
    }

    /// Takes the next reading.
    pub fn push(&mut self, r: Reading) -> Progress {
        let restarted = self.raw.back().is_some_and(|last| r.ms <= last.ms);
        if restarted || matches!(self.state, State::Done) {
            *self = Self::new().with_min_grams(self.min_grams);
        }
        if let Some(first) = &mut self.tare {
            let first = *first.get_or_insert(r.ms);
            if r.grams.abs() > TARE_ZERO_G && r.ms < first + TARE_MAX_MS {
                return Progress::Settling;
            }
            self.tare = None;
        }
        self.raw.push_back(r);
        if self.raw.len() > 3 {
            self.raw.pop_front();
        }
        if self.raw.len() < 3 {
            return Progress::Settling;
        }
        let mut g = [self.raw[0].grams, self.raw[1].grams, self.raw[2].grams];
        g.sort_by(f64::total_cmp);
        let s = Reading {
            ms: self.raw[1].ms,
            grams: g[1],
        };
        self.history.push_back(s);
        self.trim(s.ms);
        self.step(s)
    }

    fn step(&mut self, s: Reading) -> Progress {
        let change = self
            .grams_at(s.ms.saturating_sub(STEP_MS))
            .map(|old| s.grams - old);
        match self.state {
            State::Settling | State::Done => match self.settled(s.ms) {
                Some(baseline) => {
                    self.state = State::Ready {
                        baseline,
                        at_baseline_ms: s.ms,
                        above_since: None,
                    };
                    Progress::Ready
                }
                None => Progress::Settling,
            },
            State::Ready {
                baseline,
                mut at_baseline_ms,
                mut above_since,
            } => {
                if change.is_some_and(|d| d.abs() > STEP_G) || s.grams < baseline - BELOW_BASELINE_G
                {
                    self.state = State::Settling;
                    return Progress::Settling;
                }
                if s.grams <= baseline + AT_BASELINE_G {
                    at_baseline_ms = s.ms;
                }
                if s.grams < baseline + DROPS_RESET_G {
                    above_since = None;
                } else if s.grams >= baseline + DROPS_G {
                    let since = *above_since.get_or_insert(s.ms);
                    if s.ms - since >= DROPS_HOLD_MS {
                        self.state = State::Flowing {
                            baseline,
                            start_ms: at_baseline_ms,
                        };
                        return Progress::Flowing {
                            grams: s.grams - baseline,
                        };
                    }
                }
                self.state = State::Ready {
                    baseline,
                    at_baseline_ms,
                    above_since,
                };
                Progress::Ready
            }
            State::Flowing { baseline, start_ms } => {
                if change.is_some_and(|d| d > STEP_G) {
                    // A cup arrived: what looked like drops was not coffee.
                    self.state = State::Settling;
                    return Progress::Settling;
                }
                if change.is_some_and(|d| d < -STEP_G) {
                    return self.finish(baseline, start_ms, s.ms.saturating_sub(LIFT_GUARD_MS));
                }
                if s.grams < baseline + DROPS_RESET_G {
                    // Back to the baseline: it was a press, not coffee.
                    self.state = State::Ready {
                        baseline,
                        at_baseline_ms: s.ms,
                        above_since: None,
                    };
                    return Progress::Ready;
                }
                let stopped = s.grams - baseline >= self.min_grams
                    && s.ms >= start_ms + STOP_WINDOW_MS
                    && self
                        .grams_at(s.ms - STOP_WINDOW_MS)
                        .is_some_and(|old| s.grams - old < STOP_RISE_G);
                if stopped {
                    return self.finish(baseline, start_ms, s.ms);
                }
                Progress::Flowing {
                    grams: s.grams - baseline,
                }
            }
        }
    }

    /// Ends the shot with what was seen up to `until_ms`.
    fn finish(&mut self, baseline: f64, start_ms: u64, until_ms: u64) -> Progress {
        let end_ms = self
            .history
            .iter()
            .rev()
            .filter(|r| r.ms <= until_ms && r.ms >= start_ms + FLOW_WINDOW_MS)
            .find(|r| {
                self.grams_at(r.ms - FLOW_WINDOW_MS)
                    .is_some_and(|old| r.grams - old >= FLOW_RISE_G)
            })
            .map_or(until_ms, |r| r.ms - FLOW_WINDOW_MS / 2);
        let mut last: Vec<f64> = self
            .history
            .iter()
            .filter(|r| r.ms <= until_ms && r.ms + FLOW_WINDOW_MS >= until_ms)
            .map(|r| r.grams)
            .collect();
        last.sort_by(f64::total_cmp);
        let in_cup = last.get(last.len() / 2).map_or(0.0, |g| g - baseline);
        let time_ms = end_ms.saturating_sub(start_ms);
        if time_ms < MIN_SHOT_MS || in_cup < MIN_SHOT_G {
            // A pause, not the end (or a lift too early to be a shot): keep
            // the start and watch on.
            return Progress::Flowing { grams: in_cup };
        }
        self.state = State::Done;
        Progress::Done(Shot {
            time: Duration::from_millis(time_ms),
            grams: (in_cup * 10.0).round() / 10.0,
        })
    }

    /// The median of the last `SETTLE_MS`, if everything in it is within
    /// `SETTLE_SPREAD_G` of it.
    fn settled(&self, now_ms: u64) -> Option<f64> {
        let from = now_ms.checked_sub(SETTLE_MS)?;
        if self.history.front().is_none_or(|r| r.ms > from) {
            return None;
        }
        let mut g: Vec<f64> = self
            .history
            .iter()
            .filter(|r| r.ms >= from)
            .map(|r| r.grams)
            .collect();
        g.sort_by(f64::total_cmp);
        let median = g[g.len() / 2];
        g.iter()
            .all(|x| (x - median).abs() <= SETTLE_SPREAD_G)
            .then_some(median)
    }

    /// The weight at `ms`: the last filtered reading at or before it.
    fn grams_at(&self, ms: u64) -> Option<f64> {
        self.history
            .iter()
            .rev()
            .find(|r| r.ms <= ms)
            .map(|r| r.grams)
    }

    /// Drops history nobody will look at again.
    fn trim(&mut self, now_ms: u64) {
        let keep_from = match self.state {
            State::Flowing { start_ms, .. } => start_ms.saturating_sub(FLOW_WINDOW_MS),
            _ => now_ms.saturating_sub(KEEP_MS),
        };
        while self.history.len() > KEEP_MAX
            || self.history.front().is_some_and(|r| r.ms < keep_from)
        {
            self.history.pop_front();
        }
    }
}

#[cfg(test)]
pub(crate) mod synth {
    //! Readings shaped like the ones a Half Decent Scale sends at 10 Hz.

    use super::Reading;

    /// Fixed jitter, about what the scale shows with a cup on it.
    const NOISE: [f64; 6] = [0.0, 0.08, -0.05, 0.1, -0.09, 0.03];

    /// `secs` of readings every 100 ms from `ms0`: `grams(t)` at `t`
    /// seconds, plus noise.
    pub(crate) fn readings(ms0: u64, secs: f64, grams: impl Fn(f64) -> f64) -> Vec<Reading> {
        let n = (secs * 10.0).round() as u64;
        (0..n)
            .map(|i| {
                let t = i as f64 / 10.0;
                Reading {
                    ms: ms0 + i * 100,
                    grams: grams(t) + NOISE[(i % 6) as usize],
                }
            })
            .collect()
    }

    /// Cup weight at `t` s: empty until 2 s, `rate` g/s for `flow_s`, then
    /// 3 s of drips at 0.1 g/s, then still.
    pub(crate) fn espresso(t: f64, flow_s: f64, rate: f64) -> f64 {
        let end = 2.0 + flow_s;
        if t < 2.0 {
            0.0
        } else if t < end {
            rate * (t - 2.0)
        } else {
            rate * flow_s + 0.1 * (t - end).min(3.0)
        }
    }

    /// Like [`espresso`] with a profile: from 2 s, each `(seconds, g/s)` in
    /// turn (a rate of 0 is a pause), then the same drips.
    pub(crate) fn profile(t: f64, phases: &[(f64, f64)]) -> f64 {
        let mut at = 2.0;
        let mut g = 0.0;
        for &(secs, rate) in phases {
            if t < at + secs {
                return g + rate * (t - at).max(0.0);
            }
            at += secs;
            g += rate * secs;
        }
        g + 0.1 * (t - at).min(3.0)
    }
}

#[cfg(test)]
mod tests {
    use super::synth::{espresso, profile, readings};
    use super::*;

    /// Feeds everything; the shot if one finished.
    fn run(rs: &[Reading]) -> Option<Shot> {
        let mut d = Detector::new();
        rs.iter().find_map(|&r| match d.push(r) {
            Progress::Done(s) => Some(s),
            _ => None,
        })
    }

    fn assert_shot(shot: Option<Shot>, secs: f64, grams: f64) {
        let s = shot.expect("a shot");
        let t = s.time.as_secs_f64();
        assert!((t - secs).abs() <= 0.4, "time {t:.2} s, expected {secs} s");
        assert!(
            (s.grams - grams).abs() <= 0.4,
            "weight {:.2} g, expected {grams} g",
            s.grams
        );
    }

    #[test]
    fn a_shot_from_first_drops_to_flow_stop() {
        // 18 s at 2 g/s: 36 g, then 0.3 g of drips.
        let rs = readings(60_000, 32.0, |t| espresso(t, 18.0, 2.0));
        assert_shot(run(&rs), 18.0, 36.3);
        // A slower, longer one.
        let rs = readings(0, 45.0, |t| espresso(t, 30.0, 1.2));
        assert_shot(run(&rs), 30.0, 36.3);
    }

    #[test]
    fn the_cup_weight_is_not_coffee() {
        // A 150 g cup on an empty scale, placed at 1 s (0.3 s, overshoot),
        // shot from 3 s.
        let cup = |t: f64| {
            if t < 1.0 {
                0.0
            } else if t < 1.3 {
                (t - 1.0) / 0.3 * 152.0
            } else if t < 1.4 {
                152.0
            } else {
                150.0
            }
        };
        let rs = readings(0, 36.0, |t| cup(t) + espresso(t - 1.0, 18.0, 2.0));
        assert_shot(run(&rs), 18.0, 36.3);
    }

    #[test]
    fn lifting_the_cup_ends_the_shot() {
        // Lifted 2 s after the flow stopped: back to 0 within 0.2 s.
        let lifted = |t: f64| {
            let g = 150.0 + espresso(t, 18.0, 2.0);
            if t < 22.0 {
                g
            } else if t < 22.2 {
                g * (22.2 - t) / 0.2
            } else {
                0.0
            }
        };
        let rs = readings(0, 26.0, lifted);
        let s = run(&rs).expect("a shot");
        assert!((s.time.as_secs_f64() - 18.0).abs() <= 0.5, "{s:?}");
        assert!((s.grams - 36.2).abs() <= 0.5, "{s:?}");
    }

    #[test]
    fn bumps_are_not_shots() {
        // Single-reading spikes like an empty scale shows, a 2 g bump for
        // 0.5 s, and a hand pressing 3 g for 2 s then letting go.
        let bumpy = |t: f64| {
            let spike = if (t - 1.5).abs() < 0.05 || (t - 4.0).abs() < 0.05 {
                0.45
            } else {
                0.0
            };
            let bump = if (6.0..6.5).contains(&t) { 2.0 } else { 0.0 };
            let press = if (9.0..11.0).contains(&t) { 3.0 } else { 0.0 };
            150.0 + spike + bump + press
        };
        let rs = readings(0, 20.0, bumpy);
        let mut d = Detector::new();
        for &r in &rs {
            let p = d.push(r);
            assert!(!matches!(p, Progress::Done(_)), "no shot, got {p:?}");
        }
        // Back to waiting for drops at the end.
        assert_eq!(
            d.push(Reading {
                ms: 20_000,
                grams: 150.0
            }),
            Progress::Ready
        );
    }

    #[test]
    fn press_before_shot_does_not_shift_start() {
        // A 3 g press for 2 s ending at 6 s, then a shot from 10 s.
        let g = |t: f64| {
            let press = if (4.0..6.0).contains(&t) { 3.0 } else { 0.0 };
            150.0 + press + espresso(t - 8.0, 18.0, 2.0)
        };
        let rs = readings(0, 40.0, g);
        assert_shot(run(&rs), 18.0, 36.3);
    }

    #[test]
    fn progress_goes_settling_ready_flowing() {
        let rs = readings(0, 32.0, |t| espresso(t, 18.0, 2.0));
        let mut d = Detector::new();
        let seen: Vec<Progress> = rs.iter().map(|&r| d.push(r)).collect();
        assert_eq!(seen[0], Progress::Settling);
        assert!(seen.contains(&Progress::Ready));
        let flowing = seen
            .iter()
            .filter_map(|p| match p {
                Progress::Flowing { grams } => Some(*grams),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(flowing.first().is_some_and(|g| *g < 4.0), "{flowing:?}");
        assert!(flowing.last().is_some_and(|g| *g > 30.0), "{flowing:?}");
    }

    #[test]
    fn clock_going_back_starts_over() {
        let mut d = Detector::new();
        for r in readings(500_000, 10.0, |t| espresso(t, 18.0, 2.0)) {
            d.push(r);
        }
        assert!(matches!(
            d.push(Reading {
                ms: 510_000,
                grams: 16.0
            }),
            Progress::Flowing { .. }
        ));
        // The scale restarted: its uptime starts again near 0.
        assert_eq!(
            d.push(Reading {
                ms: 1_200,
                grams: 16.0
            }),
            Progress::Settling
        );
        // A full shot after the restart still comes out right.
        let rs = readings(1_300, 32.0, |t| espresso(t, 18.0, 2.0));
        assert_shot(
            rs.iter().find_map(|&r| match d.push(r) {
                Progress::Done(s) => Some(s),
                _ => None,
            }),
            18.0,
            36.3,
        );
    }

    #[test]
    fn a_tare_is_not_coffee() {
        // -3 g left from an earlier tare; the tare lands at 0.9 s and the
        // scale reads 0 from then on. The shot starts at 4 s.
        let g = |t: f64| {
            let before = if t < 0.9 { -3.0 } else { 0.0 };
            before + espresso(t - 2.0, 18.0, 2.0)
        };
        let rs = readings(0, 36.0, g);
        let mut d = Detector::after_tare();
        let shot = rs.iter().find_map(|&r| match d.push(r) {
            Progress::Done(s) => Some(s),
            _ => None,
        });
        assert_shot(shot, 18.0, 36.3);
        // Without the pause, the same jump would look like first drops.
        let early = readings(0, 3.5, g);
        let mut d = Detector::new();
        assert!(
            early
                .iter()
                .any(|&r| matches!(d.push(r), Progress::Flowing { .. })),
            "the test should show why after_tare exists"
        );
    }

    #[test]
    fn a_pause_after_preinfusion_is_not_the_end() {
        // 3 s of drops (6 g), 4 s pause, then the main flow to 36 g.
        let short = [(3.0, 2.0), (4.0, 0.0), (15.0, 2.0)];
        let rs = readings(0, 36.0, |t| profile(t, &short));
        assert_shot(run(&rs), 22.0, 36.3);

        // 6 s of slow drops (7.2 g), 4 s pause, main flow: only the recipe
        // weight tells this pause from the end of a small shot.
        let long = [(6.0, 1.2), (4.0, 0.0), (14.4, 2.0)];
        let rs = readings(0, 38.0, |t| profile(t, &long));
        let mut d = Detector::new().expecting(Some(36.0));
        let shot = rs.iter().find_map(|&r| match d.push(r) {
            Progress::Done(s) => Some(s),
            _ => None,
        });
        assert_shot(shot, 24.4, 36.3);
    }

    #[test]
    fn too_short_or_too_small_is_not_a_shot() {
        // 3 s of flow at 2 g/s: 6 g but under 5 s.
        let rs = readings(0, 15.0, |t| espresso(t, 3.0, 2.0));
        assert_eq!(run(&rs), None);
    }
}
