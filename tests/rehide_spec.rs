//! Spec tests for the pure smart and focused-app rehide policy.
//!
//! Every expected value comes from docs/clean-room/smart-rehide/SPEC.md (Part A
//! rules, Part B decisions D1 to D10) and docs/clean-room/smart-rehide/API.md.
//! Each test names its rule ID. Near-miss cases sit next to the triggering
//! case so a rule that fires too often or too rarely fails.

use std::cell::Cell;
use std::time::Duration;

use ice_rust::rehide_policy::*;

const DOCK_PID: Pid = 1;
const EDITOR_PID: Pid = 42;
const OTHER_PID: Pid = 43;
const ROGUE_PID: Pid = 9;
const CURSOR_LEVEL: i32 = 20;
const SPACE: SpaceId = 7;

fn settings(auto_rehide: bool, strategy: RehideStrategy) -> RehideSettings {
    RehideSettings {
        auto_rehide,
        strategy,
    }
}

fn bar(visible: DividerState, hidden: DividerState, always_hidden: DividerState) -> MenuBar {
    MenuBar {
        visible,
        hidden,
        always_hidden,
        ice_bar: None,
        use_ice_bar: false,
    }
}

fn all_shown() -> MenuBar {
    bar(DividerState::Show, DividerState::Show, DividerState::Show)
}

fn all_hidden() -> MenuBar {
    bar(DividerState::Hide, DividerState::Hide, DividerState::Hide)
}

fn app(bundle_id: Option<&str>, active: bool, policy: ActivationPolicy) -> RunningApp {
    RunningApp {
        bundle_id: bundle_id.map(str::to_owned),
        active,
        policy,
    }
}

fn lookup(apps: &[(Pid, RunningApp)]) -> impl Fn(Pid) -> Option<RunningApp> + '_ {
    move |pid| apps.iter().find(|(p, _)| *p == pid).map(|(_, a)| a.clone())
}

fn window(layer: i32, bounds: Rect, owner: Pid) -> Window {
    Window {
        layer,
        bounds,
        owner,
    }
}

/// A 500 by 500 window at the origin. The pointer used below, (100, 200), is inside it.
fn covering() -> Rect {
    Rect::new(0.0, 0.0, 500.0, 500.0)
}

const POINTER: (f64, f64) = (100.0, 200.0);

/// The editor app: an active regular app, so R16 hides everything.
fn editor_apps() -> Vec<(Pid, RunningApp)> {
    vec![(
        EDITOR_PID,
        app(Some("com.example.editor"), true, ActivationPolicy::Regular),
    )]
}

fn check<'a>(windows: &'a [Window]) -> SmartCheck<'a> {
    SmartCheck {
        snapshot_space: SPACE,
        current_space: SPACE,
        pointer: Some(POINTER),
        windows,
        cursor_level: CURSOR_LEVEL,
        dock: Some(DOCK_PID),
    }
}

fn smart_settings() -> RehideSettings {
    settings(true, RehideStrategy::Smart)
}

fn click() -> SmartClick {
    SmartClick {
        settings: smart_settings(),
        target: ClickTarget::Other,
        posted_by_app: false,
        revealed: true,
        pointer_in_menu_bar: false,
        space: SPACE,
    }
}

fn focus(frontmost: Option<Pid>, last_other: Option<Pid>) -> FocusChange {
    FocusChange {
        settings: settings(true, RehideStrategy::FocusedApp),
        frontmost,
        own: OWN_PID,
        last_other,
        pointer_in_menu_bar: false,
    }
}

const OWN_PID: Pid = 100;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// R8: the smart check waits 250 ms.
#[test]
fn r8_smart_check_delay_is_250_ms() {
    assert_eq!(SMART_CHECK_DELAY, Duration::from_millis(250));
}

// F4: the focused-app hide waits 0.1 s.
#[test]
fn f4_focus_hide_delay_is_100_ms() {
    assert_eq!(FOCUS_HIDE_DELAY, Duration::from_millis(100));
}

// R15: the Dock's bundle identifier.
#[test]
fn r15_dock_bundle_id_is_com_apple_dock() {
    assert_eq!(DOCK_BUNDLE_ID, "com.apple.dock");
}

// D5: normal app windows are on layer 0.
#[test]
fn d5_normal_window_layer_is_zero() {
    assert_eq!(NORMAL_WINDOW_LAYER, 0);
}

// R17: the hide order is visible, hidden, always-hidden.
#[test]
fn r17_hide_order_is_visible_hidden_always_hidden() {
    assert_eq!(
        HIDE_ORDER,
        [Section::Visible, Section::Hidden, Section::AlwaysHidden]
    );
    assert_eq!(
        Section::ALL,
        [Section::Visible, Section::Hidden, Section::AlwaysHidden]
    );
}

// ---------------------------------------------------------------------------
// Settings: S1 to S4, D1, D2, T1
// ---------------------------------------------------------------------------

// S1: the strategy defaults to Smart.
#[test]
fn s1_default_strategy_is_smart() {
    assert_eq!(RehideStrategy::default(), RehideStrategy::Smart);
}

// S2: stored integers 0, 1 and 2 map to Smart, Timed and Focused app.
#[test]
fn s2_from_raw_maps_0_1_2() {
    assert_eq!(RehideStrategy::from_raw(0), Some(RehideStrategy::Smart));
    assert_eq!(RehideStrategy::from_raw(1), Some(RehideStrategy::Timed));
    assert_eq!(
        RehideStrategy::from_raw(2),
        Some(RehideStrategy::FocusedApp)
    );
}

// S2: any integer outside 0 to 2 has no strategy.
#[test]
fn s2_from_raw_rejects_other_values() {
    for raw in [-1, 3, 4, 255, i64::MIN, i64::MAX] {
        assert_eq!(RehideStrategy::from_raw(raw), None, "raw {raw}");
    }
}

// S2, D8: raw() is the inverse of from_raw() and matches the stored values.
#[test]
fn s2_raw_is_inverse_of_from_raw_and_matches_stored_values() {
    assert_eq!(RehideStrategy::Smart.raw(), 0);
    assert_eq!(RehideStrategy::Timed.raw(), 1);
    assert_eq!(RehideStrategy::FocusedApp.raw(), 2);
    for strategy in RehideStrategy::ALL {
        assert_eq!(RehideStrategy::from_raw(strategy.raw()), Some(strategy));
    }
}

// S2: an invalid stored value leaves Smart in place, even where D1 would
// otherwise pick Timed for an existing install.
#[test]
fn s2_invalid_stored_value_keeps_smart_even_with_existing_install_keys() {
    assert_eq!(
        stored_strategy(Some(3), false, false),
        RehideStrategy::Smart
    );
    assert_eq!(
        stored_strategy(Some(-1), true, false),
        RehideStrategy::Smart
    );
    assert_eq!(stored_strategy(Some(9), false, true), RehideStrategy::Smart);
}

// S2: a stored valid value wins over the D1 default.
#[test]
fn s2_stored_valid_value_wins_over_d1_default() {
    assert_eq!(stored_strategy(Some(0), false, true), RehideStrategy::Smart);
    assert_eq!(
        stored_strategy(Some(2), true, true),
        RehideStrategy::FocusedApp
    );
    assert_eq!(
        stored_strategy(Some(1), false, false),
        RehideStrategy::Timed
    );
}

// S4: picker order and labels.
#[test]
fn s4_picker_order_and_labels() {
    assert_eq!(
        RehideStrategy::ALL,
        [
            RehideStrategy::Smart,
            RehideStrategy::Timed,
            RehideStrategy::FocusedApp
        ]
    );
    assert_eq!(RehideStrategy::Smart.label(), "Smart");
    assert_eq!(RehideStrategy::Timed.label(), "Timed");
    assert_eq!(RehideStrategy::FocusedApp.label(), "Focused app");
}

// S3: the one-line description under the picker, exact text from the spec.
#[test]
fn s3_descriptions_match_spec_text() {
    assert_eq!(
        RehideStrategy::Smart.description(),
        "Menu bar items are rehidden using a smart algorithm."
    );
    assert_eq!(
        RehideStrategy::Timed.description(),
        "Menu bar items are rehidden after a fixed amount of time."
    );
    assert_eq!(
        RehideStrategy::FocusedApp.description(),
        "Menu bar items are rehidden when the focused app changes."
    );
}

// S3: with the switch off, neither the picker nor the slider shows, whatever the strategy.
#[test]
fn s3_switch_off_hides_picker_and_interval_for_every_strategy() {
    for strategy in RehideStrategy::ALL {
        let controls = rehide_controls(settings(false, strategy));
        assert!(!controls.strategy_picker, "{strategy:?}");
        assert!(!controls.interval, "{strategy:?}");
    }
}

// S3: with the switch on, Smart and Focused app show the picker but not the slider.
#[test]
fn s3_smart_and_focused_app_show_picker_only() {
    for strategy in [RehideStrategy::Smart, RehideStrategy::FocusedApp] {
        let controls = rehide_controls(settings(true, strategy));
        assert!(controls.strategy_picker, "{strategy:?}");
        assert!(!controls.interval, "{strategy:?}");
    }
}

// S3: with the switch on and Timed selected, the slider shows.
#[test]
fn s3_timed_with_switch_on_shows_interval() {
    let controls = rehide_controls(settings(true, RehideStrategy::Timed));
    assert!(controls.strategy_picker);
    assert!(controls.interval);
}

// D1: a fresh install (no keys stored) defaults to Smart.
#[test]
fn d1_fresh_install_defaults_to_smart() {
    assert_eq!(default_strategy(false, false), RehideStrategy::Smart);
    assert_eq!(stored_strategy(None, false, false), RehideStrategy::Smart);
}

// D1: an install with AutoRehide stored but no RehideStrategy keeps Timed.
#[test]
fn d1_existing_auto_rehide_key_keeps_timed() {
    assert_eq!(default_strategy(true, false), RehideStrategy::Timed);
    assert_eq!(stored_strategy(None, true, false), RehideStrategy::Timed);
}

// D1: an install with RehideInterval stored but no RehideStrategy keeps Timed.
#[test]
fn d1_existing_interval_key_keeps_timed() {
    assert_eq!(default_strategy(false, true), RehideStrategy::Timed);
    assert_eq!(stored_strategy(None, false, true), RehideStrategy::Timed);
    assert_eq!(default_strategy(true, true), RehideStrategy::Timed);
}

// D2: Focused app needs "Automatically rehide" on (Q1 fix).
#[test]
fn d2_focused_app_needs_auto_rehide() {
    assert!(!focused_app_enabled(settings(
        false,
        RehideStrategy::FocusedApp
    )));
    assert!(focused_app_enabled(settings(
        true,
        RehideStrategy::FocusedApp
    )));
}

// R2: Smart is enabled only with the switch on and the strategy set to Smart.
#[test]
fn r2_smart_enabled_truth_table() {
    assert!(smart_enabled(settings(true, RehideStrategy::Smart)));
    assert!(!smart_enabled(settings(false, RehideStrategy::Smart)));
    assert!(!smart_enabled(settings(true, RehideStrategy::Timed)));
    assert!(!smart_enabled(settings(true, RehideStrategy::FocusedApp)));
}

// T1: Timed checks run only with the switch on and the strategy set to Timed.
#[test]
fn t1_timed_checks_enabled_truth_table() {
    assert!(timed_checks_enabled(settings(true, RehideStrategy::Timed)));
    assert!(!timed_checks_enabled(settings(
        false,
        RehideStrategy::Timed
    )));
    assert!(!timed_checks_enabled(settings(true, RehideStrategy::Smart)));
    assert!(!timed_checks_enabled(settings(
        true,
        RehideStrategy::FocusedApp
    )));
}

// ---------------------------------------------------------------------------
// Terms and hide action: H1 to H4, R5, R17
// ---------------------------------------------------------------------------

// Terms, "Shown section": a section is shown when its divider is Show.
#[test]
fn terms_divider_show_means_shown() {
    let menu = bar(DividerState::Hide, DividerState::Show, DividerState::Hide);
    assert!(!menu.is_shown(Section::Visible));
    assert!(menu.is_shown(Section::Hidden));
    assert!(!menu.is_shown(Section::AlwaysHidden));
}

// Terms: an Ice Bar showing the hidden section also shows the visible section.
#[test]
fn terms_ice_bar_hidden_section_also_shows_visible() {
    let menu = MenuBar {
        ice_bar: Some(Section::Hidden),
        ..all_hidden()
    };
    assert!(menu.is_shown(Section::Hidden));
    assert!(menu.is_shown(Section::Visible));
    assert!(!menu.is_shown(Section::AlwaysHidden));
}

// Terms: an Ice Bar showing the always-hidden section shows only that section.
#[test]
fn terms_ice_bar_always_hidden_section_shows_only_itself() {
    let menu = MenuBar {
        ice_bar: Some(Section::AlwaysHidden),
        ..all_hidden()
    };
    assert!(menu.is_shown(Section::AlwaysHidden));
    assert!(!menu.is_shown(Section::Visible));
    assert!(!menu.is_shown(Section::Hidden));
}

// Terms and R5: nothing revealed means no section is shown.
#[test]
fn r5_nothing_revealed_when_all_hidden_and_ice_bar_closed() {
    assert!(!all_hidden().is_revealed());
}

// R5: a single shown section counts as revealed.
#[test]
fn r5_any_shown_section_counts_as_revealed() {
    assert!(bar(DividerState::Hide, DividerState::Hide, DividerState::Show).is_revealed());
    assert!(bar(DividerState::Hide, DividerState::Show, DividerState::Hide).is_revealed());
    assert!(bar(DividerState::Show, DividerState::Hide, DividerState::Hide).is_revealed());
}

// H1: hiding a section that is not shown changes nothing.
#[test]
fn h1_hiding_unshown_section_is_no_op() {
    let menu = all_hidden();
    let outcome = hide_section(&menu, Section::Hidden);
    assert_eq!(outcome.menu_bar, menu);
    assert!(!outcome.close_ice_bar);
    assert!(outcome.stop_timed.is_empty());
    assert!(!outcome.changed());
}

// H1: hiding an unshown section leaves an open Ice Bar alone.
#[test]
fn h1_unshown_section_does_not_close_ice_bar() {
    let menu = MenuBar {
        ice_bar: Some(Section::Hidden),
        ..all_hidden()
    };
    let outcome = hide_section(&menu, Section::AlwaysHidden);
    assert_eq!(outcome.menu_bar.ice_bar, Some(Section::Hidden));
    assert!(!outcome.close_ice_bar);
    assert!(!outcome.changed());
}

// H2: hiding a shown section closes the Ice Bar.
#[test]
fn h2_hiding_shown_section_closes_ice_bar() {
    let menu = MenuBar {
        ice_bar: Some(Section::Hidden),
        ..all_hidden()
    };
    let outcome = hide_section(&menu, Section::Hidden);
    assert!(outcome.close_ice_bar);
    assert_eq!(outcome.menu_bar.ice_bar, None);
    assert!(outcome.changed());
}

// H3: hiding the visible section sets every divider to Hide.
#[test]
fn h3_hiding_visible_sets_all_dividers_to_hide() {
    let outcome = hide_section(&all_shown(), Section::Visible);
    assert_eq!(outcome.menu_bar.visible, DividerState::Hide);
    assert_eq!(outcome.menu_bar.hidden, DividerState::Hide);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
}

// H3: hiding the hidden section sets every divider to Hide.
#[test]
fn h3_hiding_hidden_sets_all_dividers_to_hide() {
    let outcome = hide_section(&all_shown(), Section::Hidden);
    assert_eq!(outcome.menu_bar.visible, DividerState::Hide);
    assert_eq!(outcome.menu_bar.hidden, DividerState::Hide);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
}

// H3: hiding always-hidden without the Ice Bar changes only its own divider.
#[test]
fn h3_always_hidden_without_ice_bar_sets_only_its_divider() {
    let outcome = hide_section(&all_shown(), Section::AlwaysHidden);
    assert_eq!(outcome.menu_bar.visible, DividerState::Show);
    assert_eq!(outcome.menu_bar.hidden, DividerState::Show);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
}

// H3: hiding always-hidden while the Ice Bar is in use sets every divider to Hide.
#[test]
fn h3_always_hidden_with_ice_bar_in_use_sets_all_dividers() {
    let menu = MenuBar {
        use_ice_bar: true,
        ..all_shown()
    };
    let outcome = hide_section(&menu, Section::AlwaysHidden);
    assert_eq!(outcome.menu_bar.visible, DividerState::Hide);
    assert_eq!(outcome.menu_bar.hidden, DividerState::Hide);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
    assert!(outcome.menu_bar.use_ice_bar);
}

// H3: hiding the hidden section while the Ice Bar is in use also sets every divider.
#[test]
fn h3_hidden_with_ice_bar_in_use_sets_all_dividers() {
    let menu = MenuBar {
        use_ice_bar: true,
        ..all_shown()
    };
    let outcome = hide_section(&menu, Section::Hidden);
    assert_eq!(outcome.menu_bar.visible, DividerState::Hide);
    assert_eq!(outcome.menu_bar.hidden, DividerState::Hide);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
}

// H4: hiding a shown section stops that section's Timed checks.
#[test]
fn h4_shown_section_stops_its_timed_checks() {
    let outcome = hide_section(&all_shown(), Section::Hidden);
    assert_eq!(outcome.stop_timed, vec![Section::Hidden]);
    assert!(outcome.changed());
}

// H4: an unshown section has no Timed checks to stop.
#[test]
fn h4_unshown_section_stops_nothing() {
    let outcome = hide_section(&all_hidden(), Section::Visible);
    assert!(outcome.stop_timed.is_empty());
}

// H1, R17: hide_sections on an empty slice returns the input unchanged.
#[test]
fn r17_hide_sections_empty_slice_is_no_op() {
    let menu = all_shown();
    let outcome = hide_sections(&menu, &[]);
    assert_eq!(outcome.menu_bar, menu);
    assert!(!outcome.close_ice_bar);
    assert!(outcome.stop_timed.is_empty());
}

// R17: each step sees the result of the one before. Hiding visible hides
// always-hidden too, so the always-hidden step that follows is a no-op.
#[test]
fn r17_later_step_sees_earlier_result() {
    let outcome = hide_sections(&all_shown(), &[Section::Visible, Section::AlwaysHidden]);
    assert_eq!(outcome.menu_bar, all_hidden());
    assert!(outcome.close_ice_bar);
    assert_eq!(outcome.stop_timed, vec![Section::Visible]);
}

// R17: stop_timed concatenates steps in the order they ran.
#[test]
fn r17_stop_timed_follows_step_order() {
    let menu = bar(DividerState::Hide, DividerState::Show, DividerState::Show);
    let outcome = hide_sections(&menu, &[Section::AlwaysHidden, Section::Hidden]);
    // Hiding always-hidden alone changes only its divider, so hidden is still shown afterward.
    assert_eq!(
        outcome.stop_timed,
        vec![Section::AlwaysHidden, Section::Hidden]
    );
    assert_eq!(outcome.menu_bar.hidden, DividerState::Hide);
}

// R17: hide_everything runs visible, hidden, always-hidden in that order.
#[test]
fn r17_hide_everything_runs_in_order_and_skips_unshown_steps() {
    // Only the hidden section shows. The visible step is a no-op, the hidden
    // step hides everything, and the always-hidden step then has nothing to do.
    let menu = bar(DividerState::Hide, DividerState::Show, DividerState::Hide);
    let outcome = hide_everything(&menu);
    assert_eq!(outcome.stop_timed, vec![Section::Hidden]);
    assert_eq!(outcome.menu_bar, all_hidden());
}

// R17: hide_everything with only always-hidden shown stops that section's Timed checks.
#[test]
fn r17_hide_everything_with_only_always_hidden_shown() {
    let menu = bar(DividerState::Hide, DividerState::Hide, DividerState::Show);
    let outcome = hide_everything(&menu);
    assert_eq!(outcome.stop_timed, vec![Section::AlwaysHidden]);
    assert_eq!(outcome.menu_bar.always_hidden, DividerState::Hide);
}

// R5, H1: hide_everything with nothing shown is a no-op.
#[test]
fn r5_hide_everything_with_nothing_shown_is_no_op() {
    let menu = all_hidden();
    let outcome = hide_everything(&menu);
    assert_eq!(outcome.menu_bar, menu);
    assert!(!outcome.changed());
    assert!(!outcome.close_ice_bar);
}

// ---------------------------------------------------------------------------
// Smart at click time: R1 to R8, D4, D6
// ---------------------------------------------------------------------------

// R2: a click is dropped when "Automatically rehide" is off.
#[test]
fn r2_click_dropped_when_switch_off() {
    let mut c = click();
    c.settings = settings(false, RehideStrategy::Smart);
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R2: a click is dropped when the strategy is Timed or Focused app.
#[test]
fn r2_click_dropped_for_timed_and_focused_strategies() {
    for strategy in [RehideStrategy::Timed, RehideStrategy::FocusedApp] {
        let mut c = click();
        c.settings = settings(true, strategy);
        assert_eq!(smart_click(&c), ClickDecision::Drop, "{strategy:?}");
    }
}

// R7, R8: a qualifying click starts a check with the active Space and a 250 ms wait.
#[test]
fn r8_qualifying_click_starts_check_after_250_ms() {
    assert_eq!(
        smart_click(&click()),
        ClickDecision::Check {
            space: SPACE,
            after: Duration::from_millis(250)
        }
    );
}

// R3: a click on ice-rust's own menu bar icon is dropped.
#[test]
fn r3_app_icon_click_dropped() {
    let mut c = click();
    c.target = ClickTarget::AppIcon;
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R4: a click on the Ice Bar is dropped.
#[test]
fn r4_ice_bar_click_dropped() {
    let mut c = click();
    c.target = ClickTarget::IceBar;
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R3, R4: a click on any other window is a candidate.
#[test]
fn r3_r4_other_window_click_is_candidate() {
    let mut c = click();
    c.target = ClickTarget::Other;
    assert!(matches!(smart_click(&c), ClickDecision::Check { .. }));
}

// D6: clicks ice-rust posts itself never trigger smart rehide.
#[test]
fn d6_app_posted_click_dropped() {
    let mut c = click();
    c.posted_by_app = true;
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R5: a click with nothing revealed is dropped.
#[test]
fn r5_click_with_nothing_revealed_dropped() {
    let mut c = click();
    c.revealed = false;
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R6, D4: a click with the pointer in the menu bar is dropped.
#[test]
fn r6_d4_click_in_menu_bar_dropped() {
    let mut c = click();
    c.pointer_in_menu_bar = true;
    assert_eq!(smart_click(&c), ClickDecision::Drop);
}

// R7: the snapshot is the Space active at the click, not a fixed value.
#[test]
fn r7_snapshot_is_the_space_at_the_click() {
    let mut c = click();
    c.space = 42;
    assert_eq!(
        smart_click(&c),
        ClickDecision::Check {
            space: 42,
            after: SMART_CHECK_DELAY
        }
    );
}

// R2 to R6: each drop condition is independent. Changing one flag at a time
// from a qualifying click must flip the decision.
#[test]
fn r2_to_r6_each_flag_alone_drops_the_click() {
    let mut off = click();
    off.settings = settings(false, RehideStrategy::Smart);
    let mut own = click();
    own.posted_by_app = true;
    let mut icon = click();
    icon.target = ClickTarget::AppIcon;
    let mut bar_click = click();
    bar_click.target = ClickTarget::IceBar;
    let mut hidden = click();
    hidden.revealed = false;
    let mut menu = click();
    menu.pointer_in_menu_bar = true;
    for c in [off, own, icon, bar_click, hidden, menu] {
        assert_eq!(smart_click(&c), ClickDecision::Drop);
    }
    assert!(matches!(smart_click(&click()), ClickDecision::Check { .. }));
}

// ---------------------------------------------------------------------------
// Smart after the wait: R10 to R17, D5
// ---------------------------------------------------------------------------

// R10: a changed Space hides everything, with no pointer, window or app lookup.
#[test]
fn r10_space_change_hides_everything_without_window_checks() {
    let windows: [Window; 0] = [];
    let calls = Cell::new(0);
    let mut c = check(&windows);
    c.current_space = 8;
    c.pointer = None;
    let decision = smart_check(&c, |_| {
        calls.set(calls.get() + 1);
        None
    });
    assert_eq!(decision, CheckDecision::HideEverything);
    assert_eq!(calls.get(), 0);
}

// R10: a Space change hides everything even when the pointer sits on a window
// that would not qualify.
#[test]
fn r10_space_change_wins_over_a_non_qualifying_window() {
    let windows = [window(19, covering(), ROGUE_PID)];
    let mut c = check(&windows);
    c.current_space = 8;
    assert_eq!(smart_check(&c, |_| None), CheckDecision::HideEverything);
}

// R10: the same Space keeps the check going.
#[test]
fn r10_same_space_does_not_hide_on_its_own() {
    let windows: [Window; 0] = [];
    assert_eq!(
        smart_check(&check(&windows), |_| None),
        CheckDecision::NoChange
    );
}

// R11: an unreadable pointer ends the check with no change, before any window or app lookup.
#[test]
fn r11_unreadable_pointer_no_change_without_lookup() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = editor_apps();
    let calls = Cell::new(0);
    let mut c = check(&windows);
    c.pointer = None;
    let decision = smart_check(&c, |pid| {
        calls.set(calls.get() + 1);
        lookup(&apps)(pid)
    });
    assert_eq!(decision, CheckDecision::NoChange);
    assert_eq!(calls.get(), 0);
}

// R11: a readable pointer on the same window does hide, which isolates the R11 cause.
#[test]
fn r11_readable_pointer_on_same_window_hides() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = editor_apps();
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// R12: a window at the cursor level is skipped, and the walk goes on to the next window.
#[test]
fn r12_window_at_cursor_level_is_skipped() {
    let windows = [
        window(CURSOR_LEVEL, covering(), ROGUE_PID),
        window(0, covering(), EDITOR_PID),
    ];
    let apps = editor_apps();
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// R12: a cursor-level window alone is skipped, leaving no candidate.
#[test]
fn r12_only_cursor_level_window_gives_no_change() {
    let windows = [window(CURSOR_LEVEL, covering(), EDITOR_PID)];
    let apps = editor_apps();
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R13 (as changed by D5): a window whose bounds do not contain the pointer is skipped.
#[test]
fn r13_window_not_covering_pointer_is_skipped() {
    let elsewhere = Rect::new(600.0, 0.0, 300.0, 300.0);
    let windows = [
        window(0, elsewhere, ROGUE_PID),
        window(0, covering(), EDITOR_PID),
    ];
    let apps = editor_apps();
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// R13: the first covering window in front-to-back order decides the click.
#[test]
fn r13_first_covering_window_in_front_to_back_order_wins() {
    let windows = [
        window(0, covering(), OTHER_PID),
        window(0, covering(), EDITOR_PID),
    ];
    let picked = clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID));
    assert_eq!(picked.map(|w| w.owner), Some(OTHER_PID));
}

// R13: no covering window gives no change.
#[test]
fn r13_no_covering_window_gives_no_change() {
    let windows = [window(0, Rect::new(600.0, 0.0, 300.0, 300.0), EDITOR_PID)];
    assert_eq!(
        smart_check(&check(&windows), lookup(&editor_apps())),
        CheckDecision::NoChange
    );
    assert_eq!(
        clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID)),
        None
    );
}

// Terms and D5: the containment edge is half-open, so the max edges are outside.
#[test]
fn d5_rect_contains_is_half_open() {
    let rect = Rect::new(10.0, 20.0, 100.0, 50.0);
    assert!(rect.contains(10.0, 20.0), "min corner is inside");
    assert!(rect.contains(109.9, 69.9), "just under max is inside");
    assert!(!rect.contains(110.0, 20.0), "max x edge is outside");
    assert!(!rect.contains(10.0, 70.0), "max y edge is outside");
    assert!(!rect.contains(9.9, 30.0), "just under min x is outside");
}

// D5: a window on layer 0 that covers the pointer is the clicked window.
#[test]
fn d5_layer_zero_window_is_clicked_window() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let picked = clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID));
    assert_eq!(picked.map(|w| w.owner), Some(EDITOR_PID));
}

// D5: a non-Dock window above layer 0 (menu, pop-up, status item) stops the walk.
// The layer 0 window behind it must not be used.
#[test]
fn d5_non_dock_window_above_layer_zero_stops_walk() {
    let windows = [
        window(8, covering(), ROGUE_PID),
        window(0, covering(), EDITOR_PID),
    ];
    assert_eq!(
        clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID)),
        None
    );
    assert_eq!(
        smart_check(&check(&windows), lookup(&editor_apps())),
        CheckDecision::NoChange
    );
}

// D5: a non-Dock window below layer 0 (another app's desktop-level window) is passed over.
#[test]
fn d5_other_app_desktop_window_is_passed_over() {
    let windows = [
        window(-1, covering(), ROGUE_PID),
        window(0, covering(), EDITOR_PID),
    ];
    let picked = clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID));
    assert_eq!(picked.map(|w| w.owner), Some(EDITOR_PID));
}

// D5, R15: a click on the desktop reaches the Dock's wallpaper window, which hides everything.
// The layer is a desktop-level value; the real layer is a macOS check (D10), not a pure-module fact.
#[test]
fn d5_r15_dock_wallpaper_click_hides_everything() {
    let windows = [window(-1000, covering(), DOCK_PID)];
    let apps = [(
        DOCK_PID,
        app(Some("com.apple.dock"), false, ActivationPolicy::Accessory),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// D5: the Dock's windows are the clicked window at any layer, not just layer 0.
#[test]
fn d5_dock_window_is_clicked_window_at_any_layer() {
    let windows = [window(8, covering(), DOCK_PID)];
    let picked = clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID));
    assert_eq!(picked.map(|w| w.owner), Some(DOCK_PID));
}

// D5: with the Dock not running (dock None), a desktop-level window is passed over, not taken.
#[test]
fn d5_desktop_window_passed_over_when_dock_not_running() {
    let windows = [window(-20, covering(), DOCK_PID)];
    assert_eq!(clicked_window(&windows, POINTER, CURSOR_LEVEL, None), None);
}

// D5: a Dock window that is also above the cursor level is still skipped (R12).
#[test]
fn d5_dock_window_at_cursor_level_is_skipped() {
    let windows = [
        window(CURSOR_LEVEL, covering(), DOCK_PID),
        window(0, covering(), EDITOR_PID),
    ];
    let picked = clicked_window(&windows, POINTER, CURSOR_LEVEL, Some(DOCK_PID));
    assert_eq!(picked.map(|w| w.owner), Some(EDITOR_PID));
}

// R14: an owner process with no running app ends the check with no change.
#[test]
fn r14_unknown_owner_gives_no_change() {
    let windows = [window(0, covering(), 999)];
    let apps: [(Pid, RunningApp); 0] = [];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R14: the app lookup is called with the clicked window's owner process ID.
#[test]
fn r14_lookup_uses_window_owner_pid() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let seen = Cell::new(None);
    let apps = editor_apps();
    let decision = smart_check(&check(&windows), |pid| {
        seen.set(Some(pid));
        lookup(&apps)(pid)
    });
    assert_eq!(decision, CheckDecision::HideEverything);
    assert_eq!(seen.get(), Some(EDITOR_PID));
}

// R15: a click on the Dock hides everything even when the Dock is inactive and an accessory.
#[test]
fn r15_dock_click_hides_everything() {
    let windows = [window(0, covering(), DOCK_PID)];
    let apps = [(
        DOCK_PID,
        app(Some("com.apple.dock"), false, ActivationPolicy::Prohibited),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// R15 near miss: a bundle identifier that only looks like the Dock's is not exempt,
// so the R16 test applies (inactive here, so no change).
#[test]
fn r15_near_miss_bundle_id_is_not_the_dock() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = [(
        EDITOR_PID,
        app(
            Some("com.apple.dock.helper"),
            false,
            ActivationPolicy::Accessory,
        ),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R16: an active regular app hides everything.
#[test]
fn r16_active_regular_app_hides_everything() {
    let windows = [window(0, covering(), EDITOR_PID)];
    assert_eq!(
        smart_check(&check(&windows), lookup(&editor_apps())),
        CheckDecision::HideEverything
    );
}

// R16: an inactive regular app (failing the active half) gives no change.
#[test]
fn r16_inactive_regular_app_gives_no_change() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = [(
        EDITOR_PID,
        app(Some("com.example.editor"), false, ActivationPolicy::Regular),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R16: an active accessory app (failing the policy half) gives no change.
#[test]
fn r16_active_accessory_app_gives_no_change() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = [(
        EDITOR_PID,
        app(Some("com.example.panel"), true, ActivationPolicy::Accessory),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R16: an active prohibited app (failing the policy half) gives no change.
#[test]
fn r16_active_prohibited_app_gives_no_change() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = [(
        EDITOR_PID,
        app(
            Some("com.example.agent"),
            true,
            ActivationPolicy::Prohibited,
        ),
    )];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::NoChange
    );
}

// R15, R16: an app with no bundle identifier is not the Dock, so R16 applies.
#[test]
fn r16_app_without_bundle_id_uses_active_and_regular_rule() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let apps = [(EDITOR_PID, app(None, true, ActivationPolicy::Regular))];
    assert_eq!(
        smart_check(&check(&windows), lookup(&apps)),
        CheckDecision::HideEverything
    );
}

// R16 check order: the app lookup happens for a clicked window with a running owner.
#[test]
fn r16_lookup_called_once_for_clicked_window_owner() {
    let windows = [window(0, covering(), EDITOR_PID)];
    let calls = Cell::new(0);
    let apps = editor_apps();
    let _ = smart_check(&check(&windows), |pid| {
        calls.set(calls.get() + 1);
        lookup(&apps)(pid)
    });
    assert_eq!(calls.get(), 1);
}

// ---------------------------------------------------------------------------
// Focused app: F1 to F4, D2, D3
// ---------------------------------------------------------------------------

// F1, F4: a qualifying focus change hides the hidden section after 0.1 s.
#[test]
fn f4_focus_change_hides_hidden_section_after_100_ms() {
    let outcome = focus_change(&focus(Some(EDITOR_PID), None));
    assert_eq!(
        outcome.decision,
        FocusDecision::Hide {
            section: Section::Hidden,
            after: Duration::from_millis(100)
        }
    );
}

// F4: the wait is the focus delay constant, not the smart delay.
#[test]
fn f4_focus_delay_is_not_smart_delay() {
    let outcome = focus_change(&focus(Some(EDITOR_PID), None));
    match outcome.decision {
        FocusDecision::Hide { after, .. } => {
            assert_eq!(after, FOCUS_HIDE_DELAY);
            assert_ne!(after, SMART_CHECK_DELAY);
        }
        FocusDecision::Drop => panic!("expected a hide"),
    }
}

// F2, D2: with "Automatically rehide" off, focus changes are dropped,
// but the last other app is still tracked.
#[test]
fn f2_switch_off_drops_change_and_still_tracks_last_app() {
    let mut change = focus(Some(EDITOR_PID), None);
    change.settings = settings(false, RehideStrategy::FocusedApp);
    let outcome = focus_change(&change);
    assert_eq!(outcome.decision, FocusDecision::Drop);
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// F2: a non-Focused strategy drops focus changes.
#[test]
fn f2_other_strategies_drop_focus_changes() {
    for strategy in [RehideStrategy::Smart, RehideStrategy::Timed] {
        let mut change = focus(Some(EDITOR_PID), None);
        change.settings = settings(true, strategy);
        assert_eq!(
            focus_change(&change).decision,
            FocusDecision::Drop,
            "{strategy:?}"
        );
    }
}

// F3: a focus change with the pointer in the menu bar is dropped.
#[test]
fn f3_pointer_in_menu_bar_drops_focus_change() {
    let mut change = focus(Some(EDITOR_PID), None);
    change.pointer_in_menu_bar = true;
    assert_eq!(focus_change(&change).decision, FocusDecision::Drop);
}

// F3, D3: a dropped change still records the new last other app.
#[test]
fn d3_dropped_change_still_updates_last_other() {
    let mut change = focus(Some(EDITOR_PID), Some(OTHER_PID));
    change.pointer_in_menu_bar = true;
    let outcome = focus_change(&change);
    assert_eq!(outcome.decision, FocusDecision::Drop);
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// D3: ice-rust becoming frontmost is not a focus change.
#[test]
fn d3_own_process_becoming_frontmost_is_dropped() {
    let outcome = focus_change(&focus(Some(OWN_PID), Some(EDITOR_PID)));
    assert_eq!(outcome.decision, FocusDecision::Drop);
}

// D3: when ice-rust is frontmost, the last other app is kept, not replaced by ice-rust.
#[test]
fn d3_own_focus_keeps_previous_last_other() {
    let outcome = focus_change(&focus(Some(OWN_PID), Some(EDITOR_PID)));
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// D3: returning to the last app other than ice-rust is dropped.
#[test]
fn d3_return_to_last_other_app_is_dropped() {
    let outcome = focus_change(&focus(Some(EDITOR_PID), Some(EDITOR_PID)));
    assert_eq!(outcome.decision, FocusDecision::Drop);
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// D3 near miss: a switch to an app other than the last one is a real focus change.
#[test]
fn d3_switch_to_different_app_hides() {
    let outcome = focus_change(&focus(Some(OTHER_PID), Some(EDITOR_PID)));
    assert_eq!(
        outcome.decision,
        FocusDecision::Hide {
            section: Section::Hidden,
            after: FOCUS_HIDE_DELAY
        }
    );
    assert_eq!(outcome.last_other, Some(OTHER_PID));
}

// D3: a focus change whose frontmost is None is neither ice-rust nor a return.
#[test]
fn d3_none_frontmost_is_not_own_or_return() {
    let outcome = focus_change(&focus(None, Some(EDITOR_PID)));
    assert_eq!(
        outcome.decision,
        FocusDecision::Hide {
            section: Section::Hidden,
            after: FOCUS_HIDE_DELAY
        }
    );
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// D3, F1: the first delivery with no history hides, and sets the last other app.
#[test]
fn d3_first_delivery_with_no_history_hides_and_records_app() {
    let outcome = focus_change(&focus(Some(EDITOR_PID), None));
    assert!(matches!(outcome.decision, FocusDecision::Hide { .. }));
    assert_eq!(outcome.last_other, Some(EDITOR_PID));
}

// D3: the first delivery being ice-rust hides nothing and records no app.
#[test]
fn d3_first_delivery_of_own_process_drops_with_no_record() {
    let outcome = focus_change(&focus(Some(OWN_PID), None));
    assert_eq!(outcome.decision, FocusDecision::Drop);
    assert_eq!(outcome.last_other, None);
}

// D3: a sequence of focus changes updates the last process ID as the spec describes.
#[test]
fn d3_sequence_tracks_last_other_across_changes() {
    let mut last = None;

    let first = focus_change(&focus(Some(EDITOR_PID), last));
    assert!(matches!(first.decision, FocusDecision::Hide { .. }));
    last = first.last_other;
    assert_eq!(last, Some(EDITOR_PID));

    let to_ice_rust = focus_change(&focus(Some(OWN_PID), last));
    assert_eq!(to_ice_rust.decision, FocusDecision::Drop);
    last = to_ice_rust.last_other;
    assert_eq!(last, Some(EDITOR_PID));

    let back = focus_change(&focus(Some(EDITOR_PID), last));
    assert_eq!(back.decision, FocusDecision::Drop);
    last = back.last_other;
    assert_eq!(last, Some(EDITOR_PID));

    let other = focus_change(&focus(Some(OTHER_PID), last));
    assert!(matches!(other.decision, FocusDecision::Hide { .. }));
    assert_eq!(other.last_other, Some(OTHER_PID));
}

// F4, H3: the focused-app hide, applied to the hidden section, hides every divider.
#[test]
fn f4_focused_app_hide_clears_every_divider() {
    let outcome = hide_section(&all_shown(), Section::Hidden);
    assert_eq!(outcome.menu_bar, all_hidden());
    assert!(
        outcome.close_ice_bar,
        "a shown section was hidden, so H2 closes the Ice Bar"
    );
}

// F4: the focused-app hide does nothing when the hidden section is not shown.
#[test]
fn f4_focused_app_hide_is_no_op_when_hidden_section_not_shown() {
    let outcome = hide_section(&all_hidden(), Section::Hidden);
    assert!(!outcome.changed());
}
