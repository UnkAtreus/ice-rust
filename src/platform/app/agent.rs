//! The macOS 27 menu bar (v1.8). `crate::agent_bar` decides; this module
//! reads our items' order and the spacers' places (from MenuBarAgent's
//! Accessibility tree when it answers, where our items carry their
//! identifiers and folded items are left out, else from AppKit frames),
//! puts a spacer back next to its divider with one Command-drag, says when
//! macOS does not show ice-rust at all, and reads the occupied parts of
//! the menu bar for the empty-space test. See docs/macos27.md.

use std::cell::Cell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

use super::{Controller, MSG_ICON_LEFT_OF_DIVIDER};
use crate::agent_bar::{
    self, AgentLayout, Spacer, agent_lengths, collapsed_divider, collapsed_spacer, expanded_items,
    occupied, spacer_adjacent,
};
use crate::arrange::{Destination, drag_path, drop_point, travel};
use crate::geometry::{self, ControlOrder};
use crate::model::{MOVE_DRAG_STEPS, MOVE_EVENT_INTERVAL, Rect};
use crate::platform::{events, menu_bar_space, permissions, timer, windows};
use crate::sections::MenuBarState;

/// System Settings > Menu Bar on macOS 27, with Allow in the Menu Bar.
const MENU_BAR_PANE: &str = "x-apple.systempreferences:com.apple.ControlCenter-Settings.extension";
/// How often a hide waits for our items to show up in the menu bar.
const FRAME_POLL: Duration = Duration::from_secs(1);
/// Spacer repairs per run: one, and one more if the first drop missed.
const MAX_SPACER_REPAIRS: u32 = 2;
/// After a repair AppKit reports the spacer's new place late: reads every
/// `REPAIR_POLL`, up to `REPAIR_READS` times, until it sits next to the
/// divider.
const REPAIR_POLL: Duration = Duration::from_millis(250);
const REPAIR_READS: u32 = 8;

/// How long after a change MenuBarAgent has laid the items out again.
const LAYOUT_SETTLE: Duration = Duration::from_millis(400);
/// When to read again after a change: timers and `Instant` read different
/// clocks, so a timer may fire a little early.
const READ_AFTER: Duration = Duration::from_millis(500);
/// How long a read of our icon's slot in MenuBarAgent stays good. The rehide
/// tick and clicks ask for the icon; each fresh read walks MenuBarAgent's
/// tree.
const ICON_CACHE: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct Agent {
    /// Our items' order, read while the items concerned were laid out
    /// collapsed: the frame of a folded or just-changed item is not where
    /// it is.
    order: Cell<Option<ControlOrder>>,
    /// Each spacer (`Spacer::index`) sat directly left of its divider when
    /// last seen with both on the bar.
    spacer_usable: [Cell<Option<bool>>; 2],
    /// Polls made while waiting for our items to show up.
    waits: Cell<u32>,
    /// Whether the "not in the menu bar" alert was shown this run.
    warned: Cell<bool>,
    /// Command-drags made to put each spacer back next to its divider.
    spacer_repairs: [Cell<u32>; 2],
    /// The last length change. AppKit reports the new width at once but
    /// the old position until MenuBarAgent has laid the items out again.
    changed: Cell<Option<Instant>>,
    /// Our icon's slot in MenuBarAgent's live window and when it was read
    /// (`agent_icon`).
    icon: Cell<Option<(Instant, Option<Rect>)>>,
    /// Whether "the icon is on no display" was logged since the icon was
    /// last found.
    icon_missing_logged: Cell<bool>,
}

/// Seconds since the first call, for the macOS 27 log lines, so a report
/// shows how state changes follow clicks.
pub(super) fn stamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!(
        "{:.2} s",
        START.get_or_init(Instant::now).elapsed().as_secs_f64()
    )
}

impl Controller {
    /// Our icon's rectangle from MenuBarAgent's live window
    /// (`agent_bar::agent_icon_rect`), read at most once per `ICON_CACHE`.
    pub(super) fn agent_icon(&self) -> Option<Rect> {
        if let Some((at, icon)) = self.agent.icon.get()
            && at.elapsed() < ICON_CACHE
        {
            return icon;
        }
        let icon = menu_bar_space::agent_windows()
            .and_then(|w| agent_bar::agent_icon_rect(&w, std::process::id() as i32));
        self.agent.icon.set(Some((Instant::now(), icon)));
        icon
    }

    /// `icon_rect` on macOS 27 when AppKit's frame puts the icon on no
    /// display: MenuBarAgent's slot (`agent_icon`). While neither finds the
    /// icon, logs that once with the AppKit frame, for reports (issue #4).
    pub(super) fn agent_icon_fallback(&self, displays: usize) -> Option<Rect> {
        let icon = self.agent_icon();
        if icon.is_some() {
            self.agent.icon_missing_logged.set(false);
        } else if !self.agent.icon_missing_logged.replace(true) {
            eprintln!(
                "ice-rust: the icon is on no display: {displays} displays, AppKit frame {:?}",
                self.status.icon_frame()
            );
        }
        icon
    }

    /// `set_menu_bar_state` on macOS 27: the lengths that hide or show
    /// `state`, or everything shown when our order would hide the icon.
    /// Returns the state applied.
    pub(super) fn set_agent_state(&self, state: MenuBarState) -> MenuBarState {
        self.read_agent_layout();
        let lengths = state.lengths();
        let mut applied = state;
        if lengths.hidden_expanded || lengths.always_hidden_expanded {
            match self.agent.order.get() {
                Some(order) if order.allows(state) => {}
                Some(_) => {
                    applied = MenuBarState::AllShown;
                    self.show_message(MSG_ICON_LEFT_OF_DIVIDER);
                }
                None => {
                    applied = MenuBarState::AllShown;
                    self.retry_agent_state(state);
                }
            }
        }
        let widths: Vec<f64> = windows::displays().iter().map(|d| d.bounds.width).collect();
        let max = agent_bar::max_length(&widths).unwrap_or_else(|| {
            applied = MenuBarState::AllShown;
            0.0
        });
        // MenuBarAgent recomputes positions while our items are away (an
        // app switched off and on under Allow in the Menu Bar), which can
        // move a spacer away from its divider; one drag puts it back.
        if applied != MenuBarState::AllShown {
            for which in Spacer::ALL {
                if self.agent.spacer_usable[which.index()].get() == Some(false)
                    && self.start_spacer_repair(which, state)
                {
                    return self.status.state();
                }
            }
        }
        let usable = Spacer::ALL.map(|s| self.agent.spacer_usable[s.index()].get() == Some(true));
        let next = agent_lengths(applied, max, usable);
        eprintln!(
            "ice-rust: [{}] menu bar {applied:?} (hidden divider {:?}, spacer {}, always-hidden divider {:?}, spacer {})",
            stamp(),
            next.hidden,
            next.spacer,
            next.always_hidden,
            next.always_spacer
        );
        self.status.set_agent_state(applied, next);
        self.agent.changed.set(Some(Instant::now()));
        if applied != MenuBarState::AllHidden {
            let me = self.me.clone();
            timer::after(self.mtm, READ_AFTER, move || {
                if let Some(c) = me.upgrade() {
                    c.read_agent_layout();
                }
            });
        }
        applied
    }

    /// No reading of our order yet, for example right after launch: tries
    /// again once the items have been laid out. Items still missing after
    /// that mean macOS does not draw them: the app is switched off under
    /// Allow in the Menu Bar (System Settings > Menu Bar), which leaves
    /// ice-rust without an icon to click. The alert says so once, and the
    /// hide keeps waiting until the items appear.
    fn retry_agent_state(&self, state: MenuBarState) {
        let waits = self.agent.waits.get() + 1;
        self.agent.waits.set(waits);
        if waits >= 2 && !self.agent.warned.replace(true) {
            eprintln!("ice-rust: our items are not in the menu bar; is IceRust allowed there?");
            self.warn_not_in_menu_bar();
        }
        let delay = if waits == 1 { READ_AFTER } else { FRAME_POLL };
        let me = self.me.clone();
        timer::after(self.mtm, delay, move || {
            if let Some(c) = me.upgrade()
                && c.status.state() == MenuBarState::AllShown
            {
                c.apply_state(state);
            }
        });
    }

    fn warn_not_in_menu_bar(&self) {
        let app = NSApplication::sharedApplication(self.mtm);
        app.activate();
        if !app.isActive() {
            // Cooperative activation can refuse an accessory app (L2).
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(
            "macOS does not show ice-rust in the menu bar",
        ));
        alert.setInformativeText(&NSString::from_str(
            "In System Settings > Menu Bar, turn on IceRust under Allow in the Menu Bar. \
             Until then ice-rust has no icon to click, and nothing hides.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Open Menu Bar Settings"));
        alert.addButtonWithTitle(&NSString::from_str("Later"));
        if alert.runModal() == NSAlertFirstButtonReturn
            && let Some(url) = NSURL::URLWithString(&NSString::from_str(MENU_BAR_PANE))
        {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    /// Puts `which` back directly left of its divider, then applies `state`
    /// again: the spacer gets a handle (an untitled item of length 0 does
    /// not move under a Command-drag), is dragged once the bar has laid it
    /// out, and goes back to a blank slot. Each step waits for
    /// MenuBarAgent's layout. False when no drag can run now.
    fn start_spacer_repair(&self, which: Spacer, state: MenuBarState) -> bool {
        let repairs = &self.agent.spacer_repairs[which.index()];
        if repairs.get() >= MAX_SPACER_REPAIRS
            || !permissions::check().can_post_events()
            || events::any_button_down()
            || events::is_posting()
        {
            return false;
        }
        repairs.set(repairs.get() + 1);
        eprintln!(
            "ice-rust: moving {} back next to its divider",
            spacer_name(which)
        );
        self.status.set_spacer_handle(which, true);
        self.agent.changed.set(Some(Instant::now()));
        let me = self.me.clone();
        timer::after(self.mtm, READ_AFTER, move || {
            if let Some(c) = me.upgrade() {
                c.drag_spacer(which, state);
            }
        });
        true
    }

    /// The spacer's and its divider's frames on the bar: from MenuBarAgent,
    /// or for the hidden spacer from AppKit while the divider is collapsed.
    fn spacer_and_divider(&self, which: Spacer) -> Option<(Rect, Rect)> {
        if let Some(frames) = self
            .read_agent_bar()
            .and_then(|layout| layout.spacer_frames(which))
        {
            return Some(frames);
        }
        if which != Spacer::Hidden {
            return None;
        }
        let spacer = self
            .status
            .spacer_frame(which)
            .map(|f| geometry::appkit_to_cg(f, windows::primary_height()))?;
        let hidden = self.control_frames().hidden.filter(collapsed_divider)?;
        Some((spacer, hidden))
    }

    fn drag_spacer(&self, which: Spacer, state: MenuBarState) {
        let posted = self
            .spacer_and_divider(which)
            .is_some_and(|(spacer, divider)| {
                // The id is not used for the drop point.
                let to = drop_point(
                    Destination::LeftOf(0),
                    &divider,
                    travel(&spacer, &divider),
                    0,
                );
                let from = (spacer.mid_x(), spacer.mid_y());
                let me = self.me.clone();
                events::command_drag(
                    self.mtm,
                    from,
                    &drag_path(from, to, MOVE_DRAG_STEPS),
                    MOVE_EVENT_INTERVAL,
                    Box::new(move |_| {
                        if let Some(c) = me.upgrade() {
                            c.end_spacer_repair(which, state);
                        }
                    }),
                )
                .is_ok()
            });
        if !posted {
            self.end_spacer_repair(which, state);
        }
    }

    fn end_spacer_repair(&self, which: Spacer, state: MenuBarState) {
        self.status.set_spacer_handle(which, false);
        self.agent.changed.set(Some(Instant::now()));
        let me = self.me.clone();
        timer::after(self.mtm, READ_AFTER, move || {
            if let Some(c) = me.upgrade() {
                c.confirm_spacer_repair(which, state, REPAIR_READS);
            }
        });
    }

    fn confirm_spacer_repair(&self, which: Spacer, state: MenuBarState, reads_left: u32) {
        self.read_agent_layout();
        if self.agent.spacer_usable[which.index()].get() == Some(true) || reads_left <= 1 {
            self.apply_state(state);
            return;
        }
        let me = self.me.clone();
        timer::after(self.mtm, REPAIR_POLL, move || {
            if let Some(c) = me.upgrade() {
                c.confirm_spacer_repair(which, state, reads_left - 1);
            }
        });
    }

    /// Reads our order and the spacers' places, keeping the last reading
    /// for whatever this read cannot see. Nothing is read right after a
    /// change, while MenuBarAgent lays the items out again.
    fn read_agent_layout(&self) {
        if self
            .agent
            .changed
            .get()
            .is_some_and(|at| at.elapsed() < LAYOUT_SETTLE)
        {
            return;
        }
        match self.read_agent_bar() {
            Some(layout) => self.read_from_slots(&layout),
            None => self.read_from_frames(),
        }
    }

    /// MenuBarAgent's slots are current, and expanded items are where they
    /// are; folded items are not among them.
    fn read_from_slots(&self, layout: &AgentLayout) {
        let icon = layout.anchors.icon.and_then(|id| layout.frame(id));
        let hidden = layout.frame(layout.anchors.hidden_divider);
        let always = layout
            .anchors
            .always_hidden_divider
            .and_then(|id| layout.frame(id));
        if let (Some(icon), Some(hidden)) = (icon, hidden) {
            self.note_order(&icon, &hidden, always.as_ref());
        }
        for which in Spacer::ALL {
            if let Some(next) = layout.spacer_next_to_divider(which) {
                self.note_spacer(which, next);
            }
        }
    }

    /// Without Accessibility: AppKit frames, only while they show items
    /// laid out collapsed, and only the hidden spacer.
    fn read_from_frames(&self) {
        let frames = self.control_frames();
        let (Some(icon), Some(hidden)) = (frames.icon, frames.hidden) else {
            return;
        };
        if !collapsed_divider(&hidden) {
            return;
        }
        self.note_order(
            &icon,
            &hidden,
            frames.always_hidden.filter(collapsed_divider).as_ref(),
        );
        let (Some(spacer), Some(hidden)) = (
            self.status.spacer_frame(Spacer::Hidden),
            self.status.hidden_frame(),
        ) else {
            return;
        };
        if collapsed_spacer(&spacer) {
            self.note_spacer(Spacer::Hidden, spacer_adjacent(&spacer, &hidden));
        }
    }

    /// Without an always-hidden divider reading, its part keeps the last
    /// reading.
    fn note_order(&self, icon: &Rect, hidden: &Rect, always: Option<&Rect>) {
        let fresh = geometry::control_order(icon, hidden, always);
        let order = match (always, self.agent.order.get()) {
            (None, Some(old)) => ControlOrder {
                hidden_left_of_icon: fresh.hidden_left_of_icon,
                ..old
            },
            _ => fresh,
        };
        self.agent.order.set(Some(order));
    }

    fn note_spacer(&self, which: Spacer, usable: bool) {
        if self.agent.spacer_usable[which.index()].replace(Some(usable)) == Some(usable) {
            return;
        }
        if usable {
            eprintln!("ice-rust: {} sits next to its divider", spacer_name(which));
        } else {
            eprintln!(
                "ice-rust: {} is not directly left of its divider; the divider alone hides, so \
                 some items may stay on a wide menu bar",
                spacer_name(which)
            );
        }
    }

    /// The parts of `bar` an item occupies, for the empty-space test: every
    /// MenuBarAgent slot except our expanded items, which look empty.
    /// `None` without an answer from MenuBarAgent.
    pub(super) fn agent_occupied(&self, bar: &Rect) -> Option<Vec<Rect>> {
        let slots = menu_bar_space::agent_slots(bar)?;
        Some(occupied(
            &slots,
            std::process::id() as i32,
            expanded_items(self.status.state()),
        ))
    }
}

fn spacer_name(which: Spacer) -> &'static str {
    match which {
        Spacer::Hidden => "the hidden section's spacer",
        Spacer::AlwaysHidden => "the always-hidden section's spacer",
    }
}
