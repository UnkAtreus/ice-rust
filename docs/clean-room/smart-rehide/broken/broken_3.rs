// Broken version 3: violates H3. hide_section hides only the always-hidden divider even while the Ice Bar is in use, where every divider should hide.
//! Smart and focused-app rehide decisions, rebuilt from
//! `docs/clean-room/smart-rehide/SPEC.md`. Rule and decision IDs in the
//! comments (S2, R10, D5 and so on) refer to that spec. Every input is plain
//! data, so each rule can be checked without a window server;
//! `platform::app::rehide_policy` reads the system, calls these functions
//! and acts on what they return. The public API is listed in
//! `docs/clean-room/smart-rehide/API.md`.

use std::time::Duration;

/// R12, D5: window bounds (`x`, `y`, `width`, `height`), with the
/// half-open `contains` that `menu_stop` and `clicked_app` use.
pub use crate::model::Rect;
/// Terms: the visible, hidden and always-hidden sections.
pub use crate::sections::Section;

/// R7, R10: a window server Space ID (`CGSSpaceID`).
pub type SpaceId = u64;

/// R14, D3, D5: a process ID (`pid_t`).
pub type Pid = i32;

/// R8: how long smart rehide waits after a click before it checks.
pub const SMART_CHECK_DELAY: Duration = Duration::from_millis(250);

/// F4: how long focused-app rehide waits before it hides.
pub const FOCUS_HIDE_DELAY: Duration = Duration::from_millis(100);

/// R15, D5 step 2: the Dock's bundle identifier.
pub const DOCK_BUNDLE_ID: &str = "com.apple.dock";

/// D5: the layer of normal app windows (`kCGNormalWindowLevel`). Step 1
/// falls back to a window at it; step 3 stops above it.
pub const NORMAL_WINDOW_LAYER: i32 = 0;

/// R17: the order in which "hide everything" runs the hide action.
pub const HIDE_ORDER: [Section; 3] = [Section::Visible, Section::Hidden, Section::AlwaysHidden];

/// S1: how revealed items hide again. The default is Smart (S1, and D1 for
/// fresh installs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RehideStrategy {
    /// R1 to R17.
    #[default]
    Smart,
    /// T1: ice-rust's own interval checks (`crate::rehide`).
    Timed,
    /// F1 to F4.
    FocusedApp,
}

impl RehideStrategy {
    /// S4: the picker's order.
    pub const ALL: [RehideStrategy; 3] = [
        RehideStrategy::Smart,
        RehideStrategy::Timed,
        RehideStrategy::FocusedApp,
    ];

    /// S2: the strategy a stored integer names (Smart 0, Timed 1, Focused
    /// app 2), or `None` for any other value.
    pub fn from_raw(raw: i64) -> Option<RehideStrategy> {
        match raw {
            0 => Some(RehideStrategy::Smart),
            1 => Some(RehideStrategy::Timed),
            2 => Some(RehideStrategy::FocusedApp),
            _ => None,
        }
    }

    /// S2, D8: the integer stored under `RehideStrategy`.
    pub fn raw(self) -> i64 {
        match self {
            RehideStrategy::Smart => 0,
            RehideStrategy::Timed => 1,
            RehideStrategy::FocusedApp => 2,
        }
    }

    /// S4: the picker label.
    pub fn label(self) -> &'static str {
        match self {
            RehideStrategy::Smart => "Smart",
            RehideStrategy::Timed => "Timed",
            RehideStrategy::FocusedApp => "Focused app",
        }
    }

    /// S3: the line that describes the strategy under the picker.
    pub fn description(self) -> &'static str {
        match self {
            RehideStrategy::Smart => "Menu bar items are rehidden using a smart algorithm.",
            RehideStrategy::Timed => "Menu bar items are rehidden after a fixed amount of time.",
            RehideStrategy::FocusedApp => {
                "Menu bar items are rehidden when the focused app changes."
            }
        }
    }
}

/// D1: the strategy when no `RehideStrategy` value is stored. An install
/// that already stored `AutoRehide` or `RehideInterval` keeps Timed, the
/// only behavior ice-rust had; a fresh install gets Smart.
pub fn default_strategy(auto_rehide_stored: bool, interval_stored: bool) -> RehideStrategy {
    if auto_rehide_stored || interval_stored {
        RehideStrategy::Timed
    } else {
        RehideStrategy::Smart
    }
}

/// S2 and D1: the strategy from the stored defaults. `stored` is the
/// integer under `RehideStrategy`, `None` when the key is absent. A stored
/// value outside 0 to 2 is ignored and Smart, the default, applies; D1's
/// Timed default applies only while the key is absent.
pub fn stored_strategy(
    stored: Option<i64>,
    auto_rehide_stored: bool,
    interval_stored: bool,
) -> RehideStrategy {
    match stored {
        None => default_strategy(auto_rehide_stored, interval_stored),
        Some(raw) => RehideStrategy::from_raw(raw).unwrap_or_default(),
    }
}

/// S1: the two settings the decisions read. The interval belongs to Timed
/// and stays in `crate::settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RehideSettings {
    /// "Automatically rehide".
    pub auto_rehide: bool,
    /// "Strategy".
    pub strategy: RehideStrategy,
}

/// R2: whether smart rehide is on: "Automatically rehide" is on and the
/// strategy is Smart.
pub fn smart_enabled(settings: RehideSettings) -> bool {
    settings.auto_rehide && settings.strategy == RehideStrategy::Smart
}

/// F2 as changed by D2: whether focused-app rehide is on. Unlike Ice, it
/// also needs "Automatically rehide".
pub fn focused_app_enabled(settings: RehideSettings) -> bool {
    settings.auto_rehide && settings.strategy == RehideStrategy::FocusedApp
}

/// T1: whether the Timed interval checks run: "Automatically rehide" is on
/// and the strategy is Timed.
pub fn timed_checks_enabled(settings: RehideSettings) -> bool {
    settings.auto_rehide && settings.strategy == RehideStrategy::Timed
}

/// S3 and D9: which rehide controls the Settings window shows besides the
/// "Automatically rehide" switch, which is always shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RehideControls {
    /// The Strategy picker and its description line.
    pub strategy_picker: bool,
    /// The interval slider.
    pub interval: bool,
}

/// S3: the picker shows while "Automatically rehide" is on; the interval
/// shows while the picker does and the strategy is Timed.
pub fn rehide_controls(settings: RehideSettings) -> RehideControls {
    RehideControls {
        strategy_picker: settings.auto_rehide,
        interval: timed_checks_enabled(settings),
    }
}

/// Terms: the state of a section's divider item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DividerState {
    /// The section's items are in the menu bar.
    Show,
    /// The section's items are pushed off screen.
    Hide,
}

/// Terms and H1 to H4: the three divider states and the Ice Bar, the input
/// and output of the hide action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuBar {
    /// The visible section's divider (the app's own icon).
    pub visible: DividerState,
    /// The hidden section's divider.
    pub hidden: DividerState,
    /// The always-hidden section's divider.
    pub always_hidden: DividerState,
    /// The section the Ice Bar panel displays, or `None` while it is
    /// closed.
    pub ice_bar: Option<Section>,
    /// H3: whether the Ice Bar is in use, meaning the "Use Ice Bar" setting
    /// as it applies on this system.
    pub use_ice_bar: bool,
}

impl MenuBar {
    /// The divider state of `section`.
    pub fn divider(&self, section: Section) -> DividerState {
        match section {
            Section::Visible => self.visible,
            Section::Hidden => self.hidden,
            Section::AlwaysHidden => self.always_hidden,
        }
    }

    /// Terms, "Shown section": the divider state is Show, or the Ice Bar
    /// displays the section. The visible section also counts as shown
    /// while the Ice Bar displays the hidden section.
    pub fn is_shown(&self, section: Section) -> bool {
        self.divider(section) == DividerState::Show
            || self.ice_bar == Some(section)
            || (section == Section::Visible && self.ice_bar == Some(Section::Hidden))
    }

    /// Terms, "Something is revealed" (R5): at least one section is shown.
    pub fn is_revealed(&self) -> bool {
        Section::ALL.iter().any(|&section| self.is_shown(section))
    }
}

/// What the hide action did (H1 to H4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HideOutcome {
    /// The divider states and the Ice Bar afterwards.
    pub menu_bar: MenuBar,
    /// H2: close the Ice Bar panel and allow show-on-hover again. True when
    /// at least one shown section was hidden.
    pub close_ice_bar: bool,
    /// H4: the sections whose Timed checks stop, in the order they were
    /// hidden. Empty when nothing was shown (H1).
    pub stop_timed: Vec<Section>,
}

impl HideOutcome {
    /// Whether the action changed anything; false is H1's no-op.
    pub fn changed(&self) -> bool {
        !self.stop_timed.is_empty()
    }
}

/// The hide action on one section. H1: a section that is not shown is left
/// alone and nothing changes. Otherwise H2 closes the Ice Bar, H3 sets
/// every divider to Hide (or only the always-hidden divider, when that
/// section is hidden with the Ice Bar not in use), and H4 stops the
/// section's Timed checks.
pub fn hide_section(menu_bar: &MenuBar, section: Section) -> HideOutcome {
    if !menu_bar.is_shown(section) {
        return HideOutcome {
            menu_bar: *menu_bar,
            close_ice_bar: false,
            stop_timed: Vec::new(),
        };
    }
    let mut next = *menu_bar;
    next.ice_bar = None;
    if section == Section::AlwaysHidden {
        next.always_hidden = DividerState::Hide;
    } else {
        next.visible = DividerState::Hide;
        next.hidden = DividerState::Hide;
        next.always_hidden = DividerState::Hide;
    }
    HideOutcome {
        menu_bar: next,
        close_ice_bar: true,
        stop_timed: vec![section],
    }
}

/// Runs `hide_section` on each of `sections` in order, each step on the
/// result of the one before. `close_ice_bar` is true when any step closed
/// it, and `stop_timed` joins the steps' lists.
pub fn hide_sections(menu_bar: &MenuBar, sections: &[Section]) -> HideOutcome {
    let mut outcome = HideOutcome {
        menu_bar: *menu_bar,
        close_ice_bar: false,
        stop_timed: Vec::new(),
    };
    for &section in sections {
        let step = hide_section(&outcome.menu_bar, section);
        outcome.menu_bar = step.menu_bar;
        outcome.close_ice_bar |= step.close_ice_bar;
        outcome.stop_timed.extend(step.stop_timed);
    }
    outcome
}

/// Terms "Hide everything" and R17: the hide action on the visible, hidden
/// and always-hidden sections, in that order (`HIDE_ORDER`).
pub fn hide_everything(menu_bar: &MenuBar) -> HideOutcome {
    hide_sections(menu_bar, &HIDE_ORDER)
}

/// R3 and R4: which of ice-rust's windows received the click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickTarget {
    /// Any other window, in ice-rust or in another app.
    Other,
    /// The window of ice-rust's own menu bar icon (R3).
    AppIcon,
    /// The Ice Bar panel (R4).
    IceBar,
}

/// A left mouse-down (R1), with what smart rehide reads at the click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmartClick {
    /// R2.
    pub settings: RehideSettings,
    /// R3, R4.
    pub target: ClickTarget,
    /// D6: ice-rust posted the click itself.
    pub posted_by_app: bool,
    /// R5: something is revealed (`MenuBar::is_revealed`).
    pub revealed: bool,
    /// R6 and D4: the pointer is in the menu bar of the display under it.
    pub pointer_in_menu_bar: bool,
    /// R7: the active Space ID at the click.
    pub space: SpaceId,
}

/// What smart rehide does with a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickDecision {
    /// The click is not a candidate.
    Drop,
    /// R7 and R8: run `smart_check` once `after` has passed, with `space`
    /// as the snapshot. Each click starts its own check; none replaces or
    /// cancels another, and the check does not read the settings again
    /// (R9).
    Check { space: SpaceId, after: Duration },
}

/// Smart rehide at click time. The click is dropped when smart rehide is
/// off (R2), ice-rust posted it (D6), it landed on ice-rust's icon (R3) or
/// on the Ice Bar (R4), nothing is revealed (R5), or the pointer is in the
/// menu bar (R6, D4). Otherwise a check starts after `SMART_CHECK_DELAY`
/// with the click's Space ID (R7, R8).
pub fn smart_click(click: &SmartClick) -> ClickDecision {
    let dropped = !smart_enabled(click.settings)
        || click.posted_by_app
        || click.target != ClickTarget::Other
        || !click.revealed
        || click.pointer_in_menu_bar;
    if dropped {
        ClickDecision::Drop
    } else {
        ClickDecision::Check {
            space: click.space,
            after: SMART_CHECK_DELAY,
        }
    }
}

/// An on-screen window from the window server's list (R12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    /// The window layer (`kCGWindowLayer`).
    pub layer: i32,
    /// The bounds in global coordinates, origin at the top left of the
    /// main display, y growing down.
    pub bounds: Rect,
    /// The owner's process ID (`kCGWindowOwnerPID`).
    pub owner: Pid,
}

/// R16: a running app's activation policy (`NSApplicationActivationPolicy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationPolicy {
    /// A Dock icon and menus (raw value 0).
    Regular,
    /// Raw value 1.
    Accessory,
    /// Raw value 2.
    Prohibited,
}

/// R14 to R16 and D5: what the lookup from a process ID returns about the
/// running app with that ID, read for the clicked app only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningApp {
    /// R15, D5 step 2.
    pub bundle_id: Option<String>,
    /// R16: the app is active (frontmost).
    pub active: bool,
    /// R16.
    pub policy: ActivationPolicy,
}

/// What the smart check reads once the wait is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartCheck<'a> {
    /// R7: the Space ID recorded at the click.
    pub snapshot_space: SpaceId,
    /// R10: the active Space ID now.
    pub current_space: SpaceId,
    /// R11: the pointer location now, in the windows' coordinates, or
    /// `None` when it cannot be read.
    pub pointer: Option<(f64, f64)>,
    /// R12: the on-screen windows, front to back.
    pub windows: &'a [Window],
    /// R12: the cursor window level (`CGWindowLevelForKey(19)`).
    pub cursor_level: i32,
    /// D5 step 1: the process ID that the accessibility hit test at the
    /// pointer returned after the wait, `None` when it gave nothing (no
    /// Accessibility permission, an error or a timeout).
    pub hit_test: Option<Pid>,
}

/// What the smart check decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckDecision {
    /// Run `hide_everything` (R10, R15, R16).
    HideEverything,
    /// Leave the menu bar as it is (R11, R14, R16, D5).
    NoChange,
}

/// D5 step 1: the clicked app's process ID, or `None` for no change. It is
/// `hit_test` when that is `Some`. Otherwise it is the owner of the first
/// window, front to back, at `NORMAL_WINDOW_LAYER` and below
/// `cursor_level` (R12) whose bounds contain `pointer` (`Rect::contains`:
/// the minimum edges are inside, the maximum edges are not).
pub fn clicked_app(
    windows: &[Window],
    pointer: (f64, f64),
    cursor_level: i32,
    hit_test: Option<Pid>,
) -> Option<Pid> {
    let (x, y) = pointer;
    hit_test.or_else(|| {
        windows
            .iter()
            .find(|window| {
                window.layer == NORMAL_WINDOW_LAYER
                    && window.layer < cursor_level
                    && window.bounds.contains(x, y)
            })
            .map(|window| window.owner)
    })
}

/// D5 step 3, the menu stop (Q6): whether the check ends with no change.
/// Of the windows with a layer below `cursor_level` (R12) whose bounds
/// contain `pointer`, the first one front to back that `clicked` owns
/// decides: true above `NORMAL_WINDOW_LAYER` (a menu, a pop-up or a panel
/// of the clicked app), false at or below it. False when the clicked app
/// owns no such window. Other processes' windows play no part, and titles
/// play none either.
pub fn menu_stop(windows: &[Window], pointer: (f64, f64), cursor_level: i32, clicked: Pid) -> bool {
    let (x, y) = pointer;
    windows
        .iter()
        .find(|window| {
            window.owner == clicked && window.layer < cursor_level && window.bounds.contains(x, y)
        })
        .is_some_and(|window| window.layer > NORMAL_WINDOW_LAYER)
}

/// Smart rehide after the wait. A changed Space hides everything without
/// further checks (R10). Otherwise there is no change when the pointer
/// cannot be read (R11), there is no clicked app (`clicked_app`, D5 step
/// 1), or `app` finds no running app with its process ID (R14). The Dock
/// hides everything (R15, D5 step 2). Any other app is no change when the
/// menu stop ends the check (`menu_stop`, D5 step 3), and otherwise hides
/// everything only when it is active with the regular policy (R16, D5 step
/// 4). `app` is the lookup from process ID to running app; it is called at
/// most once, with the clicked app's process ID.
pub fn smart_check(
    check: &SmartCheck<'_>,
    app: impl Fn(Pid) -> Option<RunningApp>,
) -> CheckDecision {
    if check.current_space != check.snapshot_space {
        return CheckDecision::HideEverything;
    }
    let Some(pointer) = check.pointer else {
        return CheckDecision::NoChange;
    };
    let Some(pid) = clicked_app(check.windows, pointer, check.cursor_level, check.hit_test) else {
        return CheckDecision::NoChange;
    };
    let Some(clicked) = app(pid) else {
        return CheckDecision::NoChange;
    };
    if clicked.bundle_id.as_deref() == Some(DOCK_BUNDLE_ID) {
        return CheckDecision::HideEverything;
    }
    if menu_stop(check.windows, pointer, check.cursor_level, pid) {
        return CheckDecision::NoChange;
    }
    if clicked.active && clicked.policy == ActivationPolicy::Regular {
        CheckDecision::HideEverything
    } else {
        CheckDecision::NoChange
    }
}

/// A change of the frontmost app (F1), with what focused-app rehide reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusChange {
    /// F2, D2.
    pub settings: RehideSettings,
    /// The new frontmost app's process ID, `None` when there is none.
    pub frontmost: Option<Pid>,
    /// D3: ice-rust's own process ID.
    pub own: Pid,
    /// D3: the last frontmost process other than ice-rust, from the
    /// previous `FocusOutcome`; `None` before the first one.
    pub last_other: Option<Pid>,
    /// F3 and D4: the pointer is in the menu bar of the display under it.
    pub pointer_in_menu_bar: bool,
}

/// What focused-app rehide does with a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDecision {
    /// The change is not a candidate.
    Drop,
    /// F4: once `after` has passed, run `hide_section` on `section` (the
    /// hidden section, which by H3 hides every section).
    Hide { section: Section, after: Duration },
}

/// The decision and the D3 bookkeeping for the next change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusOutcome {
    /// F2 to F4, D2, D3.
    pub decision: FocusDecision,
    /// D3: the new frontmost process when it is not ice-rust, otherwise
    /// `last_other` unchanged. Updated whatever the decision.
    pub last_other: Option<Pid>,
}

/// Focused-app rehide. The change is dropped when focused-app rehide is off
/// (F2, D2), ice-rust itself became frontmost (D3), the new frontmost app
/// is the last one other than ice-rust, so focus is coming back to it
/// (D3), or the pointer is in the menu bar (F3, D4). Otherwise the hidden
/// section hides after `FOCUS_HIDE_DELAY` (F4). A `None` frontmost is
/// neither ice-rust nor a return.
pub fn focus_change(change: &FocusChange) -> FocusOutcome {
    let own = change.frontmost == Some(change.own);
    let back = change.frontmost.is_some() && change.frontmost == change.last_other;
    let last_other = match change.frontmost {
        Some(pid) if pid != change.own => Some(pid),
        _ => change.last_other,
    };
    let dropped =
        !focused_app_enabled(change.settings) || own || back || change.pointer_in_menu_bar;
    let decision = if dropped {
        FocusDecision::Drop
    } else {
        FocusDecision::Hide {
            section: Section::Hidden,
            after: FOCUS_HIDE_DELAY,
        }
    };
    FocusOutcome {
        decision,
        last_other,
    }
}
