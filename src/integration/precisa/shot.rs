//! Finding the end of a shot in the scale's readings. Pure, so it is tested
//! without a scale.
//!
//! A shot ends when the scale's timer, once running, is stopped (someone
//! pressed the timer button, or the scale stopped it), or when the weight in
//! the cup has stopped rising. In the second case the shot's time is the
//! moment the weight last rose, not the moment that was noticed, and it must
//! be at least the minimum time: a pause before that (preinfusion) is not the
//! end. A timer stopped before the minimum time still ends the shot; the
//! caller decides what a too short shot means.

use std::time::Duration;

use super::precisa::Reading;

/// When a shot counts as finished.
#[derive(Debug, Clone, Copy)]
pub struct EndRule {
    /// Readings this soon after the knob press are ignored: the tare takes a
    /// moment and the scale may still report the cup.
    pub settle: Duration,
    /// The weight must not rise for this long ...
    pub stable_for: Duration,
    /// ... by this much or more (smaller steps are noise and drips) ...
    pub rise_g: f64,
    /// ... with at least this much in the cup ...
    pub min_weight_g: f64,
    /// ... and not before this time.
    pub min_time: Duration,
}

impl Default for EndRule {
    fn default() -> Self {
        Self {
            settle: Duration::from_secs(1),
            stable_for: Duration::from_secs(3),
            rise_g: 0.3,
            min_weight_g: 5.0,
            min_time: Duration::from_secs(20),
        }
    }
}

/// The end of a shot: time since the knob press, weight in the cup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct End {
    pub time: Duration,
    pub grams: f64,
}

#[derive(Debug)]
pub struct ShotTracker {
    rule: EndRule,
    timer_ran: bool,
    peak: f64,
    last_rise: Option<Duration>,
    grams: f64,
}

impl ShotTracker {
    pub fn new(rule: EndRule) -> Self {
        Self {
            rule,
            timer_ran: false,
            peak: 0.0,
            last_rise: None,
            grams: 0.0,
        }
    }

    /// A reading `at` this long after the knob press.
    pub fn reading(&mut self, at: Duration, r: Reading) -> Option<End> {
        if at < self.rule.settle {
            return None;
        }
        self.grams = r.grams;
        if r.timer_running {
            self.timer_ran = true;
        } else if self.timer_ran {
            return Some(End {
                time: at,
                grams: r.grams,
            });
        }
        if r.grams >= self.peak + self.rule.rise_g {
            self.peak = r.grams;
            self.last_rise = Some(at);
        }
        self.tick(at)
    }

    /// Checks for a stable weight without a new reading (the scale may only
    /// send when the weight changes).
    pub fn tick(&self, at: Duration) -> Option<End> {
        let rose = self.last_rise?;
        (self.peak >= self.rule.min_weight_g
            && rose >= self.rule.min_time
            && at.saturating_sub(rose) >= self.rule.stable_for)
            .then_some(End {
                time: rose,
                grams: self.grams,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn reading(grams: f64, timer_running: bool) -> Reading {
        Reading {
            grams,
            timer_running,
        }
    }

    /// Readings every 250 ms: nothing for 5 s, then 2 g/s until 40 g at 25 s.
    fn shot_weight(at_ms: u64) -> f64 {
        if at_ms <= 5_000 {
            0.0
        } else {
            ((at_ms - 5_000) as f64 / 500.0).min(40.0)
        }
    }

    #[test]
    fn ends_when_the_weight_stops_rising() {
        let mut t = ShotTracker::new(EndRule::default());
        let mut end = None;
        for i in 0..200 {
            let at = i * 250;
            if let Some(e) = t.reading(ms(at), reading(shot_weight(at), false)) {
                end = Some((at, e));
                break;
            }
        }
        let (seen_at, e) = end.expect("shot should end");
        assert_eq!(e, End { time: ms(25_000), grams: 40.0 });
        assert_eq!(seen_at, 28_000);
    }

    #[test]
    fn tick_ends_a_shot_when_the_scale_goes_quiet() {
        let mut t = ShotTracker::new(EndRule::default());
        for i in 0..=100 {
            assert_eq!(t.reading(ms(i * 250), reading(shot_weight(i * 250), false)), None);
        }
        // Last reading at 25 s; no more notifications.
        assert_eq!(t.tick(ms(27_000)), None);
        assert_eq!(t.tick(ms(28_000)), Some(End { time: ms(25_000), grams: 40.0 }));
    }

    #[test]
    fn a_few_grams_are_not_a_shot() {
        let mut t = ShotTracker::new(EndRule::default());
        for i in 4..40 {
            let grams = if i < 12 { i as f64 * 0.4 } else { 4.4 };
            assert_eq!(t.reading(ms(i * 250), reading(grams, false)), None);
        }
        assert_eq!(t.tick(ms(60_000)), None);
    }

    #[test]
    fn a_pause_before_the_minimum_time_is_not_the_end() {
        let mut t = ShotTracker::new(EndRule::default());
        // 2 g/s from 5 s, a pause at 12 g from 11 s to 16 s, then on to 40 g.
        let grams = |at: u64| -> f64 {
            let flowing = at.saturating_sub(5_000).min(6_000) + at.saturating_sub(16_000);
            (flowing as f64 / 500.0).min(40.0)
        };
        let mut end = None;
        for i in 0..200 {
            if let Some(e) = t.reading(ms(i * 250), reading(grams(i * 250), false)) {
                end = Some(e);
                break;
            }
        }
        // Not at 11 s (below 20 s), but after the flow stops at 30 s.
        assert_eq!(end, Some(End { time: ms(30_000), grams: 40.0 }));
    }

    #[test]
    fn no_minimum_time_allows_short_shots() {
        let rule = EndRule {
            min_time: Duration::ZERO,
            ..EndRule::default()
        };
        let mut t = ShotTracker::new(rule);
        for i in 4..=44 {
            // 2 g/s from 1 s to 6 s, then 10 g.
            let at = i * 250;
            let g = (at.saturating_sub(1_000) as f64 / 500.0).min(10.0);
            if let Some(e) = t.reading(ms(at), reading(g, false)) {
                assert_eq!(e, End { time: ms(6_000), grams: 10.0 });
                return;
            }
        }
        panic!("short shot should end without a minimum time");
    }

    #[test]
    fn stopping_the_timer_ends_the_shot() {
        let mut t = ShotTracker::new(EndRule::default());
        assert_eq!(t.reading(ms(2_000), reading(0.0, true)), None);
        assert_eq!(t.reading(ms(20_000), reading(30.0, true)), None);
        assert_eq!(
            t.reading(ms(27_500), reading(36.2, false)),
            Some(End { time: ms(27_500), grams: 36.2 })
        );
    }

    #[test]
    fn readings_before_the_tare_settles_are_ignored() {
        let mut t = ShotTracker::new(EndRule::default());
        // The cup, before the tare took effect.
        assert_eq!(t.reading(ms(200), reading(152.3, false)), None);
        for i in 5..40 {
            assert_eq!(t.reading(ms(i * 250), reading(0.0, false)), None);
        }
        assert_eq!(t.tick(ms(20_000)), None);
    }
}
