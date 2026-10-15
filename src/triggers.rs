//! Showing hidden items from the menu bar itself (v1.5), like Ice's
//! ShowOnClick, ShowOnHover and ShowOnScroll: a click in empty menu bar
//! space, the pointer resting there, or a scroll or swipe over the menu
//! bar. Pure; time is passed in. `platform::app::triggers` installs the
//! event monitors and does the window server and Accessibility queries.

use std::time::{Duration, Instant};

use crate::model::Rect;
use crate::sections::Section;

/// Default wait before hovering shows or hides items (Ice's
/// ShowOnHoverDelay).
pub const HOVER_DELAY: Duration = Duration::from_millis(200);
/// Longest hover delay accepted and offered in the Settings window.
pub const MAX_HOVER_DELAY: Duration = Duration::from_secs(1);
/// How early a hover timer may fire and still find its check due; timers
/// and `Instant` read different clocks.
pub const HOVER_TIMER_SLACK: Duration = Duration::from_millis(10);
/// Margin around the Ice Bar that still counts as hovering it (Ice: 10 pt).
pub const ICE_BAR_HOVER_PAD: f64 = 10.0;
/// Extra height below the notch that still counts as the notch (Ice: 1 pt).
pub const NOTCH_PAD: f64 = 1.0;
/// Scroll amount, summed over one gesture, that shows or hides items.
pub const SCROLL_THRESHOLD: f64 = 10.0;
/// A pause this long between scroll events ends a gesture.
pub const SCROLL_GAP: Duration = Duration::from_millis(300);
/// Points per line for a wheel without precise deltas.
pub const SCROLL_LINE: f64 = 10.0;

/// The active menu bar as the empty-space test sees it, in CG space.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuBarSpace {
    /// The menu bar of the display that shows our icon.
    pub bar: Rect,
    /// Right edge of the frontmost app's menus. Everything left of it, to
    /// the display's left edge, belongs to the app menus.
    pub app_menus_end: f64,
    /// On-screen status item windows.
    pub items: Vec<Rect>,
    /// The display's notch, if it has one.
    pub notch: Option<Rect>,
}

/// Whether `pointer` is in empty menu bar space: inside the menu bar, right
/// of the app menus, outside every status item and outside the notch. An
/// expanded divider is not on screen, so it is not among `items`.
pub fn is_empty_space(pointer: (f64, f64), space: &MenuBarSpace) -> bool {
    let (x, y) = pointer;
    let in_notch = space
        .notch
        .is_some_and(|n| Rect::new(n.x, n.y, n.width, n.height + NOTCH_PAD).contains(x, y));
    space.bar.contains(x, y)
        && x >= space.app_menus_end
        && !space.items.iter().any(|item| item.contains(x, y))
        && !in_notch
}

/// Modifier keys held during a click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
}

/// The section a click in empty space shows or hides: a plain click the
/// hidden section, an Option-click the always-hidden section, like a click
/// on the icon. Any other modifier leaves the click alone.
pub fn click_section(modifiers: Modifiers) -> Option<Section> {
    let Modifiers {
        shift,
        control,
        option,
        command,
    } = modifiers;
    if shift || control || command {
        None
    } else if option {
        Some(Section::AlwaysHidden)
    } else {
        Some(Section::Hidden)
    }
}

/// One scroll event as a signed amount: positive when the fingers or the
/// wheel move down or right, whatever the natural scrolling setting.
/// `inverted` is `isDirectionInvertedFromDevice` (natural scrolling);
/// `precise` is `hasPreciseScrollingDeltas` (trackpad or Magic Mouse), and
/// a wheel's line deltas count `SCROLL_LINE` points each. Like Ice, the
/// horizontal and vertical deltas are averaged.
pub fn scroll_amount(dx: f64, dy: f64, inverted: bool, precise: bool) -> f64 {
    let scale = if precise { 1.0 } else { SCROLL_LINE };
    let direction = if inverted { 1.0 } else { -1.0 };
    direction * scale * (dx + dy) / 2.0
}

/// What a scroll gesture over the menu bar asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAction {
    /// Down or right: show the hidden items.
    Show,
    /// Up or left: hide them.
    Hide,
}

/// Sums the scroll events of one gesture and acts once per gesture, so a
/// slow swipe still counts and momentum events do not act again. A gesture
/// counts only if it starts over the menu bar.
#[derive(Clone, Debug, Default)]
pub struct ScrollGesture {
    last: Option<Instant>,
    sum: f64,
    /// Started over the menu bar and has not acted yet.
    armed: bool,
}

impl ScrollGesture {
    /// Feeds one event of `amount` (`scroll_amount`) at `now`.
    /// `over_menu_bar` is asked only for the first event of a gesture.
    pub fn feed(
        &mut self,
        amount: f64,
        now: Instant,
        over_menu_bar: impl FnOnce() -> bool,
    ) -> Option<ScrollAction> {
        let starts = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= SCROLL_GAP);
        self.last = Some(now);
        if starts {
            self.sum = 0.0;
            self.armed = over_menu_bar();
        }
        if !self.armed || !amount.is_finite() {
            return None;
        }
        self.sum += amount;
        let action = if self.sum >= SCROLL_THRESHOLD {
            ScrollAction::Show
        } else if self.sum <= -SCROLL_THRESHOLD {
            ScrollAction::Hide
        } else {
            return None;
        };
        self.armed = false;
        Some(action)
    }
}

/// A check that hovering scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverCheck {
    /// The pointer rested in the menu bar while items were hidden.
    Show,
    /// The pointer left the menu bar and the Ice Bar after hovering showed
    /// the items.
    Hide,
}

/// Where the pointer is, from the cheap test on every mouse move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoverPointer {
    pub in_menu_bar: bool,
    /// Within `ICE_BAR_HOVER_PAD` of the open Ice Bar.
    pub near_ice_bar: bool,
}

/// Show on hover. A mouse move starts a check `delay` later; the check
/// re-tests the pointer, with the precise empty-space test for Show, before
/// anything changes. Only items that hovering showed hide again when the
/// pointer leaves, and a click in the menu bar takes them over (`release`),
/// like Ice's `showOnHoverAllowed` latch, so moving into an item's menu
/// keeps them.
#[derive(Clone, Debug, Default)]
pub struct Hover {
    shown: bool,
    pending: Option<(HoverCheck, Instant)>,
}

impl Hover {
    /// A mouse move at `now`. Returns the delay of a check that starts now;
    /// a check of the same kind already waiting keeps its time. A move that
    /// no longer wants the waiting check cancels it.
    pub fn moved(
        &mut self,
        pointer: HoverPointer,
        items_hidden: bool,
        delay: Duration,
        now: Instant,
    ) -> Option<Duration> {
        let wanted = if self.shown {
            (!pointer.in_menu_bar && !pointer.near_ice_bar).then_some(HoverCheck::Hide)
        } else {
            (items_hidden && pointer.in_menu_bar).then_some(HoverCheck::Show)
        };
        match (wanted, self.pending) {
            (None, _) => {
                self.pending = None;
                None
            }
            (Some(check), Some((waiting, _))) if waiting == check => None,
            (Some(check), _) => {
                self.pending = Some((check, now + delay));
                Some(delay)
            }
        }
    }

    /// The check due at `now` (within `HOVER_TIMER_SLACK`), taken off the
    /// schedule.
    pub fn due(&mut self, now: Instant) -> Option<HoverCheck> {
        match self.pending {
            Some((check, at)) if now + HOVER_TIMER_SLACK >= at => {
                self.pending = None;
                Some(check)
            }
            _ => None,
        }
    }

    /// Hovering showed the items.
    pub fn showed(&mut self) {
        self.shown = true;
        self.pending = None;
    }

    /// The items were shown or hidden some other way, or a click in the
    /// menu bar took them over: leaving no longer hides them.
    pub fn release(&mut self) {
        self.shown = false;
        self.pending = None;
    }

    /// Whether hovering showed the items and still owns them.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// Whether a check is waiting.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> MenuBarSpace {
        MenuBarSpace {
            bar: Rect::new(0.0, 0.0, 1512.0, 37.0),
            app_menus_end: 400.0,
            items: vec![
                Rect::new(1100.0, 0.0, 34.0, 37.0),
                Rect::new(1134.0, 0.0, 40.0, 37.0),
            ],
            notch: Some(Rect::new(665.0, 0.0, 185.0, 32.0)),
        }
    }

    #[test]
    fn empty_space_is_between_the_app_menus_and_the_items() {
        let s = space();
        assert!(is_empty_space((500.0, 10.0), &s));
        assert!(is_empty_space((1000.0, 36.0), &s));
        assert!(is_empty_space((400.0, 0.0), &s));
    }

    #[test]
    fn app_menus_items_notch_and_outside_are_not_empty() {
        let s = space();
        // App menus, to the display's left edge.
        assert!(!is_empty_space((399.0, 10.0), &s));
        assert!(!is_empty_space((0.0, 10.0), &s));
        // Status items.
        assert!(!is_empty_space((1100.0, 10.0), &s));
        assert!(!is_empty_space((1173.0, 10.0), &s));
        // The notch, and 1 pt below it.
        assert!(!is_empty_space((700.0, 5.0), &s));
        assert!(!is_empty_space((700.0, 32.5), &s));
        assert!(is_empty_space((700.0, 33.5), &s));
        // Below the menu bar or on another display.
        assert!(!is_empty_space((500.0, 37.0), &s));
        assert!(!is_empty_space((1600.0, 10.0), &s));
        assert!(!is_empty_space((500.0, -5.0), &s));
    }

    #[test]
    fn right_of_the_items_is_empty_too() {
        // Space right of the last item is empty space, as in Ice.
        assert!(is_empty_space((1300.0, 10.0), &space()));
    }

    #[test]
    fn no_notch() {
        let mut s = space();
        s.notch = None;
        assert!(is_empty_space((700.0, 5.0), &s));
    }

    #[test]
    fn click_modifiers() {
        let m = Modifiers::default();
        assert_eq!(click_section(m), Some(Section::Hidden));
        let option = Modifiers { option: true, ..m };
        assert_eq!(click_section(option), Some(Section::AlwaysHidden));
        for other in [
            Modifiers { shift: true, ..m },
            Modifiers { control: true, ..m },
            Modifiers { command: true, ..m },
            Modifiers {
                command: true,
                option: true,
                ..m
            },
        ] {
            assert_eq!(click_section(other), None, "{other:?}");
        }
    }

    #[test]
    fn scroll_amount_follows_the_fingers() {
        // Natural scrolling: fingers down give a positive delta.
        assert_eq!(scroll_amount(0.0, 12.0, true, true), 6.0);
        // Without natural scrolling the same gesture gives a negative delta.
        assert_eq!(scroll_amount(0.0, -12.0, false, true), 6.0);
        // Fingers up.
        assert_eq!(scroll_amount(0.0, -12.0, true, true), -6.0);
        // Right counts like down, as Ice adds the deltas.
        assert_eq!(scroll_amount(8.0, 0.0, true, true), 4.0);
        // A wheel line is SCROLL_LINE points.
        assert_eq!(scroll_amount(0.0, 1.0, true, false), 5.0);
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_gesture_acts_once_after_the_threshold() {
        let t = Instant::now();
        let mut g = ScrollGesture::default();
        assert_eq!(g.feed(4.0, t, || true), None);
        assert_eq!(g.feed(4.0, at(t, 16), || unreachable!()), None);
        assert_eq!(
            g.feed(4.0, at(t, 32), || unreachable!()),
            Some(ScrollAction::Show)
        );
        // Momentum events of the same gesture do nothing.
        assert_eq!(g.feed(20.0, at(t, 48), || unreachable!()), None);
        assert_eq!(g.feed(-40.0, at(t, 64), || unreachable!()), None);
        // A pause starts a new gesture.
        assert_eq!(g.feed(-12.0, at(t, 400), || true), Some(ScrollAction::Hide));
    }

    #[test]
    fn a_gesture_that_starts_elsewhere_never_acts() {
        let t = Instant::now();
        let mut g = ScrollGesture::default();
        assert_eq!(g.feed(30.0, t, || false), None);
        // Moving into the menu bar mid-gesture does not arm it.
        assert_eq!(g.feed(30.0, at(t, 20), || unreachable!()), None);
        assert_eq!(g.feed(30.0, at(t, 500), || true), Some(ScrollAction::Show));
    }

    #[test]
    fn small_back_and_forth_scrolls_do_not_act() {
        let t = Instant::now();
        let mut g = ScrollGesture::default();
        for i in 0..20 {
            let amount = if i % 2 == 0 { 6.0 } else { -6.0 };
            assert_eq!(g.feed(amount, at(t, i * 10), || true), None);
        }
        assert_eq!(g.feed(f64::NAN, at(t, 300), || true), None);
    }

    const IN_BAR: HoverPointer = HoverPointer {
        in_menu_bar: true,
        near_ice_bar: false,
    };
    const AWAY: HoverPointer = HoverPointer {
        in_menu_bar: false,
        near_ice_bar: false,
    };
    const NEAR_BAR: HoverPointer = HoverPointer {
        in_menu_bar: false,
        near_ice_bar: true,
    };

    #[test]
    fn resting_in_the_menu_bar_schedules_one_show_check() {
        let t = Instant::now();
        let mut h = Hover::default();
        assert_eq!(h.moved(IN_BAR, true, HOVER_DELAY, t), Some(HOVER_DELAY));
        // More moves keep the first deadline.
        assert_eq!(h.moved(IN_BAR, true, HOVER_DELAY, at(t, 100)), None);
        assert_eq!(h.due(at(t, 150)), None);
        // A timer that fires a moment early still finds the check.
        assert_eq!(h.due(at(t, 195)), Some(HoverCheck::Show));
        assert!(!h.is_pending());
    }

    #[test]
    fn leaving_before_the_delay_cancels_the_show() {
        let t = Instant::now();
        let mut h = Hover::default();
        h.moved(IN_BAR, true, HOVER_DELAY, t);
        assert_eq!(h.moved(AWAY, true, HOVER_DELAY, at(t, 50)), None);
        assert_eq!(h.due(at(t, 300)), None);
    }

    #[test]
    fn nothing_to_show_while_items_are_shown() {
        let t = Instant::now();
        let mut h = Hover::default();
        assert_eq!(h.moved(IN_BAR, false, HOVER_DELAY, t), None);
        // Shown some other way: leaving does not hide them either.
        assert_eq!(h.moved(AWAY, false, HOVER_DELAY, t), None);
    }

    #[test]
    fn hover_shown_items_hide_after_leaving() {
        let t = Instant::now();
        let mut h = Hover::default();
        h.moved(IN_BAR, true, HOVER_DELAY, t);
        assert_eq!(h.due(at(t, 200)), Some(HoverCheck::Show));
        h.showed();
        assert!(h.is_shown());
        // In the menu bar or near the Ice Bar: nothing to do.
        assert_eq!(h.moved(IN_BAR, false, HOVER_DELAY, at(t, 300)), None);
        assert_eq!(h.moved(NEAR_BAR, false, HOVER_DELAY, at(t, 350)), None);
        // Away: a hide check.
        assert_eq!(
            h.moved(AWAY, false, HOVER_DELAY, at(t, 400)),
            Some(HOVER_DELAY)
        );
        // Back before it is due: cancelled.
        assert_eq!(h.moved(IN_BAR, false, HOVER_DELAY, at(t, 450)), None);
        assert_eq!(h.due(at(t, 700)), None);
        h.moved(AWAY, false, HOVER_DELAY, at(t, 800));
        assert_eq!(h.due(at(t, 1000)), Some(HoverCheck::Hide));
    }

    #[test]
    fn a_click_takes_hover_shown_items_over() {
        let t = Instant::now();
        let mut h = Hover::default();
        h.showed();
        h.moved(AWAY, false, HOVER_DELAY, t);
        h.release();
        assert!(!h.is_shown());
        assert_eq!(h.due(at(t, 500)), None);
        assert_eq!(h.moved(AWAY, false, HOVER_DELAY, at(t, 600)), None);
    }

    #[test]
    fn zero_delay_is_due_at_once() {
        let t = Instant::now();
        let mut h = Hover::default();
        assert_eq!(
            h.moved(IN_BAR, true, Duration::ZERO, t),
            Some(Duration::ZERO)
        );
        assert_eq!(h.due(t), Some(HoverCheck::Show));
    }
}
