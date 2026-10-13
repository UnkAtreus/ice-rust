//! Decides when a revealed hidden section should hide again. Time is passed
//! in, so the state machine is deterministic under test.

use std::time::{Duration, Instant};

/// What the controller observed at one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub pointer_in_menu_bar: bool,
    pub popup_menu_visible: bool,
    pub panel_visible: bool,
}

impl Activity {
    pub fn any(&self) -> bool {
        self.pointer_in_menu_bar || self.popup_menu_visible || self.panel_visible
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Rehide,
}

#[derive(Clone, Debug)]
pub struct Rehide {
    interval: Duration,
    revealed: bool,
    last_active: Instant,
}

impl Rehide {
    pub fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            revealed: false,
            last_active: now,
        }
    }

    pub fn is_revealed(&self) -> bool {
        self.revealed
    }

    /// Applies a new interval from the settings; the idle count continues.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// The section was revealed; start counting from `now`.
    pub fn reveal(&mut self, now: Instant) {
        self.revealed = true;
        self.last_active = now;
    }

    /// The section was hidden by something other than this state machine.
    pub fn hide(&mut self) {
        self.revealed = false;
    }

    /// A manual toggle happened; restart the idle count.
    pub fn reset(&mut self, now: Instant) {
        self.last_active = now;
    }

    /// Records activity and decides. Returning `Rehide` also moves the state
    /// to hidden, so the next tick returns `Keep` until the next reveal.
    pub fn tick(&mut self, activity: Activity, now: Instant) -> Decision {
        // Activity at this tick always keeps the section, even with a zero
        // interval.
        if activity.any() {
            self.last_active = now;
            return Decision::Keep;
        }
        if self.revealed && now.saturating_duration_since(self.last_active) >= self.interval {
            self.revealed = false;
            return Decision::Rehide;
        }
        Decision::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);
    const IDLE: Activity = Activity {
        pointer_in_menu_bar: false,
        popup_menu_visible: false,
        panel_visible: false,
    };

    #[test]
    fn hidden_state_never_rehides() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        assert!(!r.is_revealed());
        assert_eq!(r.tick(IDLE, t0 + 100 * SEC), Decision::Keep);
    }

    #[test]
    fn rehides_after_interval_of_idle() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        r.reveal(t0);
        assert_eq!(r.tick(IDLE, t0 + 14 * SEC), Decision::Keep);
        assert_eq!(r.tick(IDLE, t0 + 15 * SEC), Decision::Rehide);
        assert!(!r.is_revealed());
        assert_eq!(r.tick(IDLE, t0 + 16 * SEC), Decision::Keep);
    }

    #[test]
    fn each_activity_kind_postpones() {
        let kinds = [
            Activity {
                pointer_in_menu_bar: true,
                ..IDLE
            },
            Activity {
                popup_menu_visible: true,
                ..IDLE
            },
            Activity {
                panel_visible: true,
                ..IDLE
            },
        ];
        for kind in kinds {
            let t0 = Instant::now();
            let mut r = Rehide::new(15 * SEC, t0);
            r.reveal(t0);
            assert_eq!(r.tick(kind, t0 + 14 * SEC), Decision::Keep);
            assert_eq!(r.tick(IDLE, t0 + 20 * SEC), Decision::Keep);
            assert_eq!(r.tick(IDLE, t0 + 29 * SEC), Decision::Rehide);
        }
    }

    #[test]
    fn activity_at_deadline_keeps() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        r.reveal(t0);
        let busy = Activity {
            pointer_in_menu_bar: true,
            ..IDLE
        };
        assert_eq!(r.tick(busy, t0 + 30 * SEC), Decision::Keep);
    }

    #[test]
    fn reset_restarts_count() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        r.reveal(t0);
        r.reset(t0 + 10 * SEC);
        assert_eq!(r.tick(IDLE, t0 + 20 * SEC), Decision::Keep);
        assert_eq!(r.tick(IDLE, t0 + 25 * SEC), Decision::Rehide);
    }

    #[test]
    fn external_hide_stops_rehide() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        r.reveal(t0);
        r.hide();
        assert_eq!(r.tick(IDLE, t0 + 60 * SEC), Decision::Keep);
    }

    #[test]
    fn new_interval_applies_to_the_running_count() {
        let t0 = Instant::now();
        let mut r = Rehide::new(15 * SEC, t0);
        r.reveal(t0);
        assert_eq!(r.tick(IDLE, t0 + 6 * SEC), Decision::Keep);
        r.set_interval(5 * SEC);
        assert_eq!(r.tick(IDLE, t0 + 7 * SEC), Decision::Rehide);
    }

    #[test]
    fn zero_interval_rehides_on_first_idle_tick() {
        let t0 = Instant::now();
        let mut r = Rehide::new(Duration::ZERO, t0);
        r.reveal(t0);
        let busy = Activity {
            panel_visible: true,
            ..IDLE
        };
        assert_eq!(r.tick(busy, t0), Decision::Keep);
        assert_eq!(
            r.tick(IDLE, t0 + Duration::from_millis(500)),
            Decision::Rehide
        );
    }
}
