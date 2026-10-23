//! Menu bar item spacing (v1.13), like Ice's (beta, `MenuBarItemSpacingManager`):
//! the space AppKit leaves around each status item comes from
//! `NSStatusItemSpacing` and `NSStatusItemSelectionPadding` in the current
//! host's global defaults, both 16 by default. Ice offers an offset from
//! -16 to +16 in steps of 2, writes 16 plus the offset to both keys (or
//! removes them for 0), and relaunches every app with a menu bar item.
//! Each app reads the keys when it starts: an item is its content plus
//! `NSStatusItemSpacing` wide (macOS 26.7.1 and 27.0.1, measured), and
//! relaunching the app is enough on both. Pure; `platform::spacing`
//! writes the keys and relaunches the apps.

/// The keys, in the current host's global domain.
pub const KEY_SPACING: &str = "NSStatusItemSpacing";
pub const KEY_PADDING: &str = "NSStatusItemSelectionPadding";
/// AppKit's value for both keys when they are not set.
pub const DEFAULT_SPACING: i64 = 16;
/// The offsets offered, Ice's range and step.
pub const MIN_OFFSET: i64 = -16;
pub const MAX_OFFSET: i64 = 16;
pub const OFFSET_STEP: i64 = 2;

/// Control Center relaunches itself after it quits, so it is quit last and
/// not opened again (Ice does the same).
pub const CONTROL_CENTER: &str = "com.apple.controlcenter";
/// MenuBarAgent draws the macOS 27 menu bar; its own items keep their
/// spacing even after it restarts (27.0.1), and a restart redraws the
/// whole bar, so it is left alone.
pub const MENU_BAR_AGENT: &str = "com.apple.MenuBarAgent";

/// The offset nearest `raw` that the slider offers.
pub fn offset_from_slider(raw: f64) -> i64 {
    if !raw.is_finite() {
        return 0;
    }
    let steps = (raw / OFFSET_STEP as f64).round() as i64;
    (steps * OFFSET_STEP).clamp(MIN_OFFSET, MAX_OFFSET)
}

/// The offset a stored `NSStatusItemSpacing` stands for, `None` when the
/// key is not set (offset 0).
pub fn offset_from_spacing(spacing: Option<f64>) -> i64 {
    spacing.map_or(0, |s| offset_from_slider(s - DEFAULT_SPACING as f64))
}

/// The value both keys get for `offset`, `None` to remove them.
pub fn spacing_value(offset: i64) -> Option<i64> {
    let offset = offset.clamp(MIN_OFFSET, MAX_OFFSET);
    (offset != 0).then_some(DEFAULT_SPACING + offset)
}

/// The slider's label, Ice's words at the ends and the middle.
pub fn offset_label(offset: i64) -> String {
    match offset {
        MIN_OFFSET => "None".to_string(),
        0 => "Default".to_string(),
        MAX_OFFSET => "Max".to_string(),
        n if n > 0 => format!("+{n}"),
        n => n.to_string(),
    }
}

/// How much wider an item is than its content in this process, from the
/// `NSStatusItemSpacing` this process started with (the macOS 27 length
/// cap depends on it).
pub fn item_chrome(spacing: Option<f64>) -> f64 {
    spacing
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(DEFAULT_SPACING as f64)
}

/// What the Apply dialog's answer means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpacingChoice {
    /// Apply Only, the first button, which Return presses: it quits
    /// nothing.
    SaveOnly,
    /// Apply and Relaunch, the second button, which only a click presses.
    Relaunch,
    Cancel,
}

/// The choice for the dialog's `response`, where `first` is the first
/// button's response code. Only the two buttons themselves act; anything
/// else (Cancel, Escape, or a modal session that ended without a button)
/// changes nothing.
pub fn spacing_choice(response: isize, first: isize) -> SpacingChoice {
    if response == first {
        SpacingChoice::SaveOnly
    } else if response == first + 1 {
        SpacingChoice::Relaunch
    } else {
        SpacingChoice::Cancel
    }
}

/// An app with a menu bar item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemApp {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub name: String,
}

/// Which apps an apply relaunches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelaunchPlan {
    /// Quit, then opened again.
    pub apps: Vec<ItemApp>,
    /// Quit last; it opens again on its own.
    pub control_center: Option<ItemApp>,
}

/// Splits `apps` for an apply: ice-rust (`own_pid`, relaunched on its
/// own at the end) and MenuBarAgent are left out, Control Center goes
/// last, the rest are sorted by name.
pub fn relaunch_plan(apps: Vec<ItemApp>, own_pid: i32) -> RelaunchPlan {
    let mut plan = RelaunchPlan::default();
    for app in apps {
        match app.bundle_id.as_deref() {
            _ if app.pid == own_pid => {}
            Some(MENU_BAR_AGENT) => {}
            Some(CONTROL_CENTER) => plan.control_center = Some(app),
            _ => plan.apps.push(app),
        }
    }
    plan.apps
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_offsets_snap_to_ices_steps() {
        assert_eq!(offset_from_slider(0.0), 0);
        assert_eq!(offset_from_slider(3.1), 4);
        assert_eq!(offset_from_slider(-2.9), -2);
        assert_eq!(offset_from_slider(40.0), 16);
        assert_eq!(offset_from_slider(-40.0), -16);
        assert_eq!(offset_from_slider(f64::NAN), 0);
    }

    #[test]
    fn keys_hold_sixteen_plus_the_offset() {
        assert_eq!(spacing_value(0), None);
        assert_eq!(spacing_value(-8), Some(8));
        assert_eq!(spacing_value(-16), Some(0));
        assert_eq!(spacing_value(16), Some(32));
        assert_eq!(spacing_value(99), Some(32));
        assert_eq!(offset_from_spacing(None), 0);
        assert_eq!(offset_from_spacing(Some(8.0)), -8);
        assert_eq!(offset_from_spacing(Some(5.0)), -12);
        assert_eq!(offset_from_spacing(Some(100.0)), 16);
    }

    #[test]
    fn only_the_two_buttons_act() {
        // NSAlertFirstButtonReturn is 1000; NSModalResponseStop -1000 and
        // NSModalResponseAbort -1001 end a session without a button.
        assert_eq!(spacing_choice(1000, 1000), SpacingChoice::SaveOnly);
        assert_eq!(spacing_choice(1001, 1000), SpacingChoice::Relaunch);
        for response in [1002, -1000, -1001, -1002, 0] {
            assert_eq!(spacing_choice(response, 1000), SpacingChoice::Cancel);
        }
    }

    #[test]
    fn labels() {
        assert_eq!(offset_label(-16), "None");
        assert_eq!(offset_label(0), "Default");
        assert_eq!(offset_label(16), "Max");
        assert_eq!(offset_label(4), "+4");
        assert_eq!(offset_label(-6), "-6");
    }

    #[test]
    fn chrome_follows_the_spacing_at_launch() {
        assert_eq!(item_chrome(None), 16.0);
        assert_eq!(item_chrome(Some(8.0)), 8.0);
        assert_eq!(item_chrome(Some(32.0)), 32.0);
        assert_eq!(item_chrome(Some(-1.0)), 16.0);
    }

    #[test]
    fn the_plan_leaves_out_ice_rust_and_menubaragent() {
        let app = |pid, id: Option<&str>, name: &str| ItemApp {
            pid,
            bundle_id: id.map(str::to_string),
            name: name.to_string(),
        };
        let plan = relaunch_plan(
            vec![
                app(10, Some("com.example.zeta"), "Zeta"),
                app(11, Some(CONTROL_CENTER), "Control Center"),
                app(12, Some("local.icerust.IceRust"), "IceRust"),
                app(13, Some(MENU_BAR_AGENT), "MenuBarAgent"),
                app(14, None, "alpha"),
            ],
            12,
        );
        let names: Vec<&str> = plan.apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Zeta"]);
        assert_eq!(plan.control_center.map(|a| a.pid), Some(11));
    }
}
