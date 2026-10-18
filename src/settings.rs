//! User settings and the AppKit status item defaults keys (M3). Defaults and
//! key math are pure; reading and writing `NSUserDefaults` is macOS only.

use std::time::Duration;

use crate::hotkeys::Hotkeys;
use crate::menu_icon::IconStyle;
use crate::model::REHIDE_INTERVAL;
use crate::rehide_policy::{RehideSettings, RehideStrategy, stored_strategy};
use crate::triggers::{HOVER_DELAY, MAX_HOVER_DELAY};

pub const KEY_USE_ICE_BAR: &str = "UseIceBar";
pub const KEY_AUTO_REHIDE: &str = "AutoRehide";
pub const KEY_REHIDE_INTERVAL: &str = "RehideInterval";
/// Ice's key for the rehide strategy, an integer (SPEC S2, D8).
pub const KEY_REHIDE_STRATEGY: &str = "RehideStrategy";
pub const KEY_AUTO_CHECK_UPDATES: &str = "AutomaticallyCheckForUpdates";
pub const KEY_ICON_STYLE: &str = "IconStyle";
/// Ice's key names for the menu bar triggers (v1.5).
pub const KEY_SHOW_ON_CLICK: &str = "ShowOnClick";
pub const KEY_SHOW_ON_HOVER: &str = "ShowOnHover";
pub const KEY_SHOW_ON_HOVER_DELAY: &str = "ShowOnHoverDelay";
pub const KEY_SHOW_ON_SCROLL: &str = "ShowOnScroll";
/// Ice's key for hiding the app menus when shown items reach them (v1.6).
pub const KEY_HIDE_APP_MENUS: &str = "HideApplicationMenus";
/// Global hotkeys as JSON, `{action: [keyCode, modifiers]}` (v1.9).
pub const KEY_HOTKEYS: &str = "Hotkeys";
pub const KEY_TINT_CUSTOM_ICON: &str = "TintCustomIcon";
/// The custom icon as PNG data at 3x (v1.4). Not part of `Settings`: it is
/// written once when the user chooses an image.
pub const KEY_CUSTOM_ICON_IMAGE: &str = "CustomIconImage";

/// Longest accepted rehide interval, in seconds.
pub const MAX_REHIDE_SECONDS: f64 = 3600.0;
/// Rehide interval range offered in the Settings window, in seconds.
pub const UI_REHIDE_MIN_SECONDS: f64 = 5.0;
pub const UI_REHIDE_MAX_SECONDS: f64 = 60.0;

/// Seeded `Preferred Position` values: icon, hidden divider, always-hidden
/// divider. A larger value sits further left (M3). Like Ice, the
/// always-hidden divider gets none (v1.2, L1): AppKit places an item
/// without a stored position, expected at the far left, so existing items
/// stay in the hidden section.
pub const SEED_POSITIONS: [Option<f64>; 3] = [Some(0.0), Some(1.0), None];

/// The seeds for macOS `os_major`: MenuBarAgent reads the same keys with
/// different meaning from macOS 27 on (`agent_bar::AGENT_SEED_POSITIONS`).
pub fn seed_positions(os_major: u32) -> [Option<f64>; 3] {
    if crate::agent_bar::uses_agent_menu_bar(os_major) {
        crate::agent_bar::AGENT_SEED_POSITIONS
    } else {
        SEED_POSITIONS
    }
}

/// Defaults flag written once the v1.2 always-hidden placement migration
/// has run (L1).
pub const KEY_ALWAYS_HIDDEN_PLACEMENT: &str = "AlwaysHiddenPlacementV2";

/// What the one-time always-hidden placement migration does at launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementMigration {
    /// Remove the stored always-hidden `Preferred Position`.
    pub remove_position: bool,
    /// Write `KEY_ALWAYS_HIDDEN_PLACEMENT`.
    pub set_flag: bool,
}

/// The one-time migration for v1.1 installs (L1). v1.1 seeded the
/// always-hidden divider at 2, directly left of the hidden divider, which
/// put every existing item in the always-hidden section. AppKit rewrites
/// positions as distances when the app quits, so the old seed cannot be
/// recognized by its value: until the flag exists, a stored always-hidden
/// position is removed once.
pub fn placement_migration(flag_present: bool, position_present: bool) -> PlacementMigration {
    PlacementMigration {
        remove_position: !flag_present && position_present,
        set_flag: !flag_present,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub use_ice_bar: bool,
    pub auto_rehide: bool,
    pub rehide_interval: Duration,
    /// How revealed items hide again (smart rehide SPEC S1, D1).
    pub rehide_strategy: RehideStrategy,
    /// Check GitHub for a new version once a day (v1.3).
    pub auto_check_updates: bool,
    /// Look of the icon (v1.4).
    pub icon_style: IconStyle,
    /// Draw the custom icon in the menu bar's color, as a template image.
    pub tint_custom_icon: bool,
    /// A click in empty menu bar space shows or hides items (v1.5).
    pub show_on_click: bool,
    /// The pointer resting in empty menu bar space shows items.
    pub show_on_hover: bool,
    /// How long the pointer rests before hovering acts.
    pub hover_delay: Duration,
    /// Scrolling or swiping over the menu bar shows or hides items.
    pub show_on_scroll: bool,
    /// Hide the frontmost app's menus while shown items reach them (v1.6).
    pub hide_app_menus: bool,
    /// Global hotkeys; none by default, like Ice (v1.9).
    pub hotkeys: Hotkeys,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_ice_bar: true,
            auto_rehide: true,
            rehide_interval: REHIDE_INTERVAL,
            // A fresh install; `from_lookup` keeps Timed for earlier ones.
            rehide_strategy: RehideStrategy::Smart,
            auto_check_updates: true,
            icon_style: IconStyle::default(),
            tint_custom_icon: true,
            // Ice's defaults.
            show_on_click: true,
            show_on_hover: false,
            hover_delay: HOVER_DELAY,
            show_on_scroll: true,
            hide_app_menus: true,
            hotkeys: Hotkeys::default(),
        }
    }
}

impl Settings {
    /// Builds settings from a key lookup, falling back to defaults for missing
    /// or invalid values.
    pub fn from_lookup(
        get_bool: impl Fn(&str) -> Option<bool>,
        get_f64: impl Fn(&str) -> Option<f64>,
        get_string: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let defaults = Self::default();
        let rehide_interval = get_f64(KEY_REHIDE_INTERVAL)
            .filter(|s| s.is_finite() && (0.0..=MAX_REHIDE_SECONDS).contains(s))
            .map(Duration::from_secs_f64)
            .unwrap_or(defaults.rehide_interval);
        // An integer stored as a number; a fraction is not a strategy.
        let strategy = get_f64(KEY_REHIDE_STRATEGY)
            .filter(|n| n.is_finite() && n.fract() == 0.0)
            .map(|n| n as i64);
        let rehide_strategy = stored_strategy(
            strategy,
            get_bool(KEY_AUTO_REHIDE).is_some(),
            get_f64(KEY_REHIDE_INTERVAL).is_some(),
        );
        Self {
            use_ice_bar: get_bool(KEY_USE_ICE_BAR).unwrap_or(defaults.use_ice_bar),
            auto_rehide: get_bool(KEY_AUTO_REHIDE).unwrap_or(defaults.auto_rehide),
            rehide_interval,
            rehide_strategy,
            auto_check_updates: get_bool(KEY_AUTO_CHECK_UPDATES)
                .unwrap_or(defaults.auto_check_updates),
            icon_style: get_string(KEY_ICON_STYLE)
                .and_then(|key| IconStyle::from_key(&key))
                .unwrap_or(defaults.icon_style),
            tint_custom_icon: get_bool(KEY_TINT_CUSTOM_ICON).unwrap_or(defaults.tint_custom_icon),
            show_on_click: get_bool(KEY_SHOW_ON_CLICK).unwrap_or(defaults.show_on_click),
            show_on_hover: get_bool(KEY_SHOW_ON_HOVER).unwrap_or(defaults.show_on_hover),
            hover_delay: get_f64(KEY_SHOW_ON_HOVER_DELAY)
                .filter(|s| s.is_finite() && (0.0..=MAX_HOVER_DELAY.as_secs_f64()).contains(s))
                .map(Duration::from_secs_f64)
                .unwrap_or(defaults.hover_delay),
            show_on_scroll: get_bool(KEY_SHOW_ON_SCROLL).unwrap_or(defaults.show_on_scroll),
            hide_app_menus: get_bool(KEY_HIDE_APP_MENUS).unwrap_or(defaults.hide_app_menus),
            hotkeys: get_string(KEY_HOTKEYS)
                .map(|text| Hotkeys::decode(&text))
                .unwrap_or(defaults.hotkeys),
        }
    }

    /// The settings the rehide decisions read.
    pub fn rehide(&self) -> RehideSettings {
        RehideSettings {
            auto_rehide: self.auto_rehide,
            strategy: self.rehide_strategy,
        }
    }
}

/// Which AppKit-owned status item key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKey {
    PreferredPosition,
    Visible,
    VisibleCc,
}

/// `NSStatusItem <Key> <autosaveName>`.
pub fn status_item_key(key: StatusKey, autosave_name: &str) -> String {
    let key = match key {
        StatusKey::PreferredPosition => "Preferred Position",
        StatusKey::Visible => "Visible",
        StatusKey::VisibleCc => "VisibleCC",
    };
    format!("NSStatusItem {key} {autosave_name}")
}

/// A defaults value to write before creating a status item.
#[derive(Clone, Debug, PartialEq)]
pub enum SeedValue {
    Number(f64),
    Bool(bool),
}

/// Keys to seed before creating a status item, skipping keys that already
/// exist. `position` is 0 for the icon and 1 for the divider, which puts the
/// divider directly left of the icon; `None` seeds no position. `VisibleCC`
/// exists only on macOS 26 and later.
pub fn seed_plan(
    autosave_name: &str,
    position: Option<f64>,
    os_major: u32,
    exists: impl Fn(&str) -> bool,
) -> Vec<(String, SeedValue)> {
    let mut wanted = Vec::new();
    if let Some(position) = position {
        wanted.push((StatusKey::PreferredPosition, SeedValue::Number(position)));
    }
    wanted.push((StatusKey::Visible, SeedValue::Bool(true)));
    if os_major >= 26 {
        wanted.push((StatusKey::VisibleCc, SeedValue::Bool(true)));
    }
    wanted
        .into_iter()
        .map(|(key, value)| (status_item_key(key, autosave_name), value))
        .filter(|(key, _)| !exists(key))
        .collect()
}

/// Whether stored `Preferred Position` values put the icon left of, or level
/// with, the divider. The value grows leftward from the menu bar's right end
/// (M3), so the icon must have the smaller one. Missing values say nothing.
pub fn positions_inverted(icon: Option<f64>, divider: Option<f64>) -> bool {
    matches!((icon, divider), (Some(i), Some(d)) if i >= d)
}

/// Whether stored positions break the order icon, hidden divider,
/// always-hidden divider from right to left. Each pair with both values
/// present is checked; when any pair is wrong all three keys are reseeded.
pub fn positions_out_of_order(icon: Option<f64>, hidden: Option<f64>, always: Option<f64>) -> bool {
    positions_inverted(icon, hidden)
        || positions_inverted(hidden, always)
        || positions_inverted(icon, always)
}

/// The positions the icon, the hidden divider and the always-hidden divider
/// have once seeding is done: the stored value, or the seed for a missing
/// key (`seed_plan` writes only missing keys). The always-hidden divider
/// has no seed, so a missing value stays missing.
pub fn positions_after_seed(stored: [Option<f64>; 3], seeds: [Option<f64>; 3]) -> [Option<f64>; 3] {
    let mut out = seeds;
    for (slot, value) in out.iter_mut().zip(stored) {
        if value.is_some() {
            *slot = value;
        }
    }
    out
}

/// Whether the launch check must remove all three position keys: the
/// positions as they will be after seeding break the order. Checking the
/// stored values alone would miss a fresh seed next to grown values, such
/// as a seeded icon at 0 beside a stored hidden divider. A missing
/// always-hidden position is accepted.
pub fn needs_reseed(stored: [Option<f64>; 3], seeds: [Option<f64>; 3]) -> bool {
    let [icon, hidden, always] = positions_after_seed(stored, seeds);
    positions_out_of_order(icon, hidden, always)
}

/// Clamps a rehide interval to the range the Settings window offers.
pub fn ui_rehide_seconds(interval: Duration) -> f64 {
    interval
        .as_secs_f64()
        .clamp(UI_REHIDE_MIN_SECONDS, UI_REHIDE_MAX_SECONDS)
}

#[cfg(target_os = "macos")]
mod store {
    use objc2_foundation::{NSData, NSNumber, NSString, NSUserDefaults};

    use super::{
        KEY_AUTO_CHECK_UPDATES, KEY_AUTO_REHIDE, KEY_HIDE_APP_MENUS, KEY_HOTKEYS, KEY_ICON_STYLE,
        KEY_REHIDE_INTERVAL, KEY_REHIDE_STRATEGY, KEY_SHOW_ON_CLICK, KEY_SHOW_ON_HOVER,
        KEY_SHOW_ON_HOVER_DELAY, KEY_SHOW_ON_SCROLL, KEY_TINT_CUSTOM_ICON, KEY_USE_ICE_BAR,
        SeedValue, Settings,
    };

    fn has_key(defaults: &NSUserDefaults, key: &str) -> bool {
        defaults.objectForKey(&NSString::from_str(key)).is_some()
    }

    /// Reads settings from the standard user defaults.
    pub fn load() -> Settings {
        let defaults = NSUserDefaults::standardUserDefaults();
        Settings::from_lookup(
            |k| has_key(&defaults, k).then(|| defaults.boolForKey(&NSString::from_str(k))),
            |k| has_key(&defaults, k).then(|| defaults.doubleForKey(&NSString::from_str(k))),
            |k| {
                defaults
                    .stringForKey(&NSString::from_str(k))
                    .map(|s| s.to_string())
            },
        )
    }

    /// Writes settings to the standard user defaults.
    pub fn save(settings: &Settings) {
        let defaults = NSUserDefaults::standardUserDefaults();
        defaults.setBool_forKey(settings.use_ice_bar, &NSString::from_str(KEY_USE_ICE_BAR));
        defaults.setBool_forKey(settings.auto_rehide, &NSString::from_str(KEY_AUTO_REHIDE));
        defaults.setDouble_forKey(
            settings.rehide_interval.as_secs_f64(),
            &NSString::from_str(KEY_REHIDE_INTERVAL),
        );
        defaults.setInteger_forKey(
            settings.rehide_strategy.raw() as isize,
            &NSString::from_str(KEY_REHIDE_STRATEGY),
        );
        defaults.setBool_forKey(
            settings.auto_check_updates,
            &NSString::from_str(KEY_AUTO_CHECK_UPDATES),
        );
        write_string(KEY_ICON_STYLE, settings.icon_style.key());
        defaults.setBool_forKey(
            settings.tint_custom_icon,
            &NSString::from_str(KEY_TINT_CUSTOM_ICON),
        );
        for (key, on) in [
            (KEY_SHOW_ON_CLICK, settings.show_on_click),
            (KEY_SHOW_ON_HOVER, settings.show_on_hover),
            (KEY_SHOW_ON_SCROLL, settings.show_on_scroll),
            (KEY_HIDE_APP_MENUS, settings.hide_app_menus),
        ] {
            defaults.setBool_forKey(on, &NSString::from_str(key));
        }
        defaults.setDouble_forKey(
            settings.hover_delay.as_secs_f64(),
            &NSString::from_str(KEY_SHOW_ON_HOVER_DELAY),
        );
        write_string(KEY_HOTKEYS, &settings.hotkeys.encode());
    }

    pub fn key_exists(key: &str) -> bool {
        has_key(&NSUserDefaults::standardUserDefaults(), key)
    }

    pub fn write_seed(key: &str, value: &SeedValue) {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str(key);
        match value {
            SeedValue::Number(n) => {
                let number = NSNumber::new_f64(*n);
                // SAFETY: NSNumber is a property list type, which is what
                // NSUserDefaults accepts.
                unsafe { defaults.setObject_forKey(Some(&number), &key) };
            }
            SeedValue::Bool(b) => defaults.setBool_forKey(*b, &key),
        }
    }

    /// Writes `true` for `key`.
    pub fn write_flag(key: &str) {
        NSUserDefaults::standardUserDefaults().setBool_forKey(true, &NSString::from_str(key));
    }

    pub fn read_number(key: &str) -> Option<f64> {
        let defaults = NSUserDefaults::standardUserDefaults();
        has_key(&defaults, key).then(|| defaults.doubleForKey(&NSString::from_str(key)))
    }

    pub fn remove_key(key: &str) {
        NSUserDefaults::standardUserDefaults().removeObjectForKey(&NSString::from_str(key));
    }

    pub fn write_number(key: &str, value: f64) {
        NSUserDefaults::standardUserDefaults().setDouble_forKey(value, &NSString::from_str(key));
    }

    pub fn read_string(key: &str) -> Option<String> {
        NSUserDefaults::standardUserDefaults()
            .stringForKey(&NSString::from_str(key))
            .map(|s| s.to_string())
    }

    pub fn read_data(key: &str) -> Option<Vec<u8>> {
        NSUserDefaults::standardUserDefaults()
            .dataForKey(&NSString::from_str(key))
            .map(|data| data.to_vec())
    }

    pub fn write_data(key: &str, value: &[u8]) {
        let data = NSData::with_bytes(value);
        // SAFETY: NSData is a property list type, which is what
        // NSUserDefaults accepts.
        unsafe {
            NSUserDefaults::standardUserDefaults()
                .setObject_forKey(Some(&data), &NSString::from_str(key))
        };
    }

    pub fn write_string(key: &str, value: &str) {
        let string = NSString::from_str(value);
        // SAFETY: NSString is a property list type, which is what
        // NSUserDefaults accepts.
        unsafe {
            NSUserDefaults::standardUserDefaults()
                .setObject_forKey(Some(&string), &NSString::from_str(key))
        };
    }
}

#[cfg(target_os = "macos")]
pub use store::{
    key_exists, load, read_data, read_number, read_string, remove_key, save, write_data,
    write_flag, write_number, write_seed, write_string,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert!(s.use_ice_bar);
        assert!(s.auto_rehide);
        assert_eq!(s.rehide_interval, Duration::from_secs(15));
    }

    #[test]
    fn lookup_overrides_defaults() {
        let bools: HashMap<&str, bool> =
            [(KEY_USE_ICE_BAR, false), (KEY_AUTO_REHIDE, false)].into();
        let nums: HashMap<&str, f64> = [(KEY_REHIDE_INTERVAL, 7.5)].into();
        let s = Settings::from_lookup(
            |k| bools.get(k).copied(),
            |k| nums.get(k).copied(),
            |_| None,
        );
        assert!(!s.use_ice_bar);
        assert!(!s.auto_rehide);
        assert_eq!(s.rehide_interval, Duration::from_secs_f64(7.5));
    }

    #[test]
    fn rehide_strategy_is_read_from_its_key() {
        assert_eq!(KEY_REHIDE_STRATEGY, "RehideStrategy");
        assert_eq!(Settings::default().rehide_strategy, RehideStrategy::Smart);
        let nums: HashMap<&str, f64> = [(KEY_REHIDE_STRATEGY, 2.0)].into();
        let s = Settings::from_lookup(|_| None, |k| nums.get(k).copied(), |_| None);
        assert_eq!(s.rehide_strategy, RehideStrategy::FocusedApp);
        // A fraction is not an integer and leaves the default.
        let nums: HashMap<&str, f64> = [(KEY_REHIDE_STRATEGY, 1.5)].into();
        let s = Settings::from_lookup(|_| None, |k| nums.get(k).copied(), |_| None);
        assert_eq!(s.rehide_strategy, RehideStrategy::Smart);
    }

    #[test]
    fn earlier_installs_keep_timed_rehide() {
        // An install that ran without the strategy key has its other
        // rehide keys stored (D1).
        let bools: HashMap<&str, bool> = [(KEY_AUTO_REHIDE, true)].into();
        let s = Settings::from_lookup(|k| bools.get(k).copied(), |_| None, |_| None);
        assert_eq!(s.rehide_strategy, RehideStrategy::Timed);
        let nums: HashMap<&str, f64> = [(KEY_REHIDE_INTERVAL, 15.0)].into();
        let s = Settings::from_lookup(|_| None, |k| nums.get(k).copied(), |_| None);
        assert_eq!(s.rehide_strategy, RehideStrategy::Timed);
        assert_eq!(
            Settings::from_lookup(|_| None, |_| None, |_| None).rehide_strategy,
            RehideStrategy::Smart
        );
    }

    #[test]
    fn update_checks_default_on_and_follow_the_stored_value() {
        assert!(Settings::default().auto_check_updates);
        let off: HashMap<&str, bool> = [(KEY_AUTO_CHECK_UPDATES, false)].into();
        let s = Settings::from_lookup(|k| off.get(k).copied(), |_| None, |_| None);
        assert!(!s.auto_check_updates);
    }

    #[test]
    fn icon_style_defaults_to_the_logo_and_follows_the_stored_value() {
        let s = Settings::default();
        assert_eq!(s.icon_style, IconStyle::Ice);
        assert!(s.tint_custom_icon);
        let strings: HashMap<&str, String> = [(KEY_ICON_STYLE, "eye".to_string())].into();
        let bools: HashMap<&str, bool> = [(KEY_TINT_CUSTOM_ICON, false)].into();
        let s = Settings::from_lookup(
            |k| bools.get(k).copied(),
            |_| None,
            |k| strings.get(k).cloned(),
        );
        assert_eq!(s.icon_style, IconStyle::Eye);
        assert!(!s.tint_custom_icon);
    }

    #[test]
    fn triggers_default_like_ice_and_follow_the_stored_values() {
        let s = Settings::default();
        assert!(s.show_on_click);
        assert!(!s.show_on_hover);
        assert!(s.show_on_scroll);
        assert_eq!(s.hover_delay, Duration::from_millis(200));
        let bools: HashMap<&str, bool> = [
            (KEY_SHOW_ON_CLICK, false),
            (KEY_SHOW_ON_HOVER, true),
            (KEY_SHOW_ON_SCROLL, false),
        ]
        .into();
        let nums: HashMap<&str, f64> = [(KEY_SHOW_ON_HOVER_DELAY, 0.5)].into();
        let s = Settings::from_lookup(
            |k| bools.get(k).copied(),
            |k| nums.get(k).copied(),
            |_| None,
        );
        assert!(!s.show_on_click);
        assert!(s.show_on_hover);
        assert!(!s.show_on_scroll);
        assert_eq!(s.hover_delay, Duration::from_millis(500));
    }

    #[test]
    fn hide_app_menus_defaults_on_like_ice() {
        assert!(Settings::default().hide_app_menus);
        let off: HashMap<&str, bool> = [(KEY_HIDE_APP_MENUS, false)].into();
        let s = Settings::from_lookup(|k| off.get(k).copied(), |_| None, |_| None);
        assert!(!s.hide_app_menus);
    }

    #[test]
    fn invalid_hover_delay_uses_default() {
        for bad in [-0.1, 1.5, f64::NAN, f64::INFINITY] {
            let nums: HashMap<&str, f64> = [(KEY_SHOW_ON_HOVER_DELAY, bad)].into();
            let s = Settings::from_lookup(|_| None, |k| nums.get(k).copied(), |_| None);
            assert_eq!(s.hover_delay, HOVER_DELAY, "value {bad}");
        }
        let nums: HashMap<&str, f64> = [(KEY_SHOW_ON_HOVER_DELAY, 0.0)].into();
        let s = Settings::from_lookup(|_| None, |k| nums.get(k).copied(), |_| None);
        assert_eq!(s.hover_delay, Duration::ZERO);
    }

    #[test]
    fn unknown_icon_style_uses_the_logo() {
        for bad in ["", "Eye", "rocket"] {
            let s = Settings::from_lookup(|_| None, |_| None, |_| Some(bad.to_string()));
            assert_eq!(s.icon_style, IconStyle::Ice, "value {bad:?}");
        }
    }

    #[test]
    fn hotkeys_load_from_their_key() {
        use crate::hotkeys::HotkeyAction;
        let s = Settings::from_lookup(
            |_| None,
            |_| None,
            |k| (k == KEY_HOTKEYS).then(|| r#"{"ToggleHiddenSection":[4,3]}"#.to_string()),
        );
        let combo = s.hotkeys.get(HotkeyAction::ToggleHiddenSection).unwrap();
        assert_eq!(combo.key_code, 4);
        assert!(combo.modifiers.control && combo.modifiers.option);
        assert_eq!(s.hotkeys.get(HotkeyAction::EnableIceBar), None);
        // None by default, like Ice.
        assert_eq!(Settings::default().hotkeys.assigned().count(), 0);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let s = Settings::from_lookup(|_| None, |_| None, |_| None);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn invalid_interval_uses_default() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, MAX_REHIDE_SECONDS + 1.0] {
            let s = Settings::from_lookup(|_| None, |_| Some(bad), |_| None);
            assert_eq!(s.rehide_interval, Duration::from_secs(15), "value {bad}");
        }
        let s = Settings::from_lookup(|_| None, |_| Some(0.0), |_| None);
        assert_eq!(s.rehide_interval, Duration::ZERO);
    }

    #[test]
    fn status_item_key_format() {
        assert_eq!(
            status_item_key(StatusKey::PreferredPosition, "IceRust.ControlItem.Hidden"),
            "NSStatusItem Preferred Position IceRust.ControlItem.Hidden"
        );
        assert_eq!(
            status_item_key(StatusKey::Visible, "IceRust.ControlItem.Visible"),
            "NSStatusItem Visible IceRust.ControlItem.Visible"
        );
        assert_eq!(
            status_item_key(StatusKey::VisibleCc, "IceRust.ControlItem.Visible"),
            "NSStatusItem VisibleCC IceRust.ControlItem.Visible"
        );
    }

    #[test]
    fn seed_plan_on_macos_26() {
        let plan = seed_plan("IceRust.ControlItem.Hidden", Some(1.0), 26, |_| false);
        assert_eq!(
            plan,
            vec![
                (
                    "NSStatusItem Preferred Position IceRust.ControlItem.Hidden".to_string(),
                    SeedValue::Number(1.0)
                ),
                (
                    "NSStatusItem Visible IceRust.ControlItem.Hidden".to_string(),
                    SeedValue::Bool(true)
                ),
                (
                    "NSStatusItem VisibleCC IceRust.ControlItem.Hidden".to_string(),
                    SeedValue::Bool(true)
                ),
            ]
        );
    }

    #[test]
    fn seed_plan_before_macos_26_skips_visible_cc() {
        let plan = seed_plan("IceRust.ControlItem.Visible", Some(0.0), 15, |_| false);
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|(k, _)| !k.contains("VisibleCC")));
        assert_eq!(plan[0].1, SeedValue::Number(0.0));
    }

    #[test]
    fn always_hidden_position_is_not_seeded() {
        // L1: only the icon and the hidden divider get a position, like Ice.
        assert_eq!(SEED_POSITIONS, [Some(0.0), Some(1.0), None]);
        let plan = seed_plan("IceRust.ControlItem.AlwaysHidden", None, 26, |_| false);
        assert_eq!(
            plan,
            vec![
                (
                    "NSStatusItem Visible IceRust.ControlItem.AlwaysHidden".to_string(),
                    SeedValue::Bool(true)
                ),
                (
                    "NSStatusItem VisibleCC IceRust.ControlItem.AlwaysHidden".to_string(),
                    SeedValue::Bool(true)
                ),
            ]
        );
    }

    #[test]
    fn placement_migration_runs_once() {
        // v1.1 install: no flag, the old always-hidden position is stored.
        assert_eq!(
            placement_migration(false, true),
            PlacementMigration {
                remove_position: true,
                set_flag: true
            }
        );
        // Fresh install or no stored position: only the flag is written.
        assert_eq!(
            placement_migration(false, false),
            PlacementMigration {
                remove_position: false,
                set_flag: true
            }
        );
        // Already migrated: a position AppKit stored since then stays.
        assert_eq!(
            placement_migration(true, true),
            PlacementMigration {
                remove_position: false,
                set_flag: false
            }
        );
        assert_eq!(
            placement_migration(true, false),
            PlacementMigration {
                remove_position: false,
                set_flag: false
            }
        );
        assert_eq!(KEY_ALWAYS_HIDDEN_PLACEMENT, "AlwaysHiddenPlacementV2");
    }

    #[test]
    fn inverted_positions() {
        // Seeded order: icon 0, divider 1. Observed Ice values: 415 and 496.
        assert!(!positions_inverted(Some(0.0), Some(1.0)));
        assert!(!positions_inverted(Some(415.0), Some(496.0)));
        // The icon was dragged left of the divider.
        assert!(positions_inverted(Some(600.0), Some(496.0)));
        assert!(positions_inverted(Some(1.0), Some(1.0)));
        assert!(!positions_inverted(None, Some(1.0)));
        assert!(!positions_inverted(Some(5.0), None));
    }

    #[test]
    fn three_positions_in_order() {
        let [i, h, a] = SEED_POSITIONS;
        assert!(!positions_out_of_order(i, h, a));
        // Ice's stored values on the evidence host: 415, 496, 5649.
        assert!(!positions_out_of_order(
            Some(415.0),
            Some(496.0),
            Some(5649.0)
        ));
        // A stored always-hidden value of 2 next to grown values is wrong.
        assert!(positions_out_of_order(Some(415.0), Some(496.0), Some(2.0)));
        // Dividers swapped.
        assert!(positions_out_of_order(Some(0.0), Some(2.0), Some(1.0)));
        // Icon left of the always-hidden divider.
        assert!(positions_out_of_order(Some(3.0), None, Some(2.0)));
        // Icon left of the hidden divider.
        assert!(positions_out_of_order(Some(1.0), Some(1.0), None));
        // Missing values say nothing.
        assert!(!positions_out_of_order(None, None, None));
        assert!(!positions_out_of_order(Some(0.0), None, None));
        assert!(!positions_out_of_order(None, Some(1.0), Some(2.0)));
    }

    #[test]
    fn seeds_fill_missing_positions() {
        let seeds = SEED_POSITIONS;
        assert_eq!(positions_after_seed([None, None, None], seeds), seeds);
        assert_eq!(
            positions_after_seed([Some(415.0), Some(496.0), None], seeds),
            [Some(415.0), Some(496.0), None]
        );
        assert_eq!(
            positions_after_seed([None, Some(7.0), Some(9.0)], seeds),
            [Some(0.0), Some(7.0), Some(9.0)]
        );
    }

    #[test]
    fn reseed_checks_the_order_after_seeding() {
        // A missing always-hidden position is accepted (L1): AppKit places
        // that divider.
        assert!(!needs_reseed(
            [Some(415.0), Some(496.0), None],
            SEED_POSITIONS
        ));
        assert!(!needs_reseed([Some(0.0), Some(1.0), None], SEED_POSITIONS));
        assert!(!needs_reseed([None, Some(496.0), None], SEED_POSITIONS));
        // Fresh install.
        assert!(!needs_reseed([None, None, None], SEED_POSITIONS));
        // Grown values in order, as AppKit wrote them for v1.1 on quit.
        assert!(!needs_reseed(
            [Some(259.0), Some(373.0), Some(537.0)],
            SEED_POSITIONS
        ));
        assert!(!needs_reseed(
            [Some(415.0), Some(496.0), Some(5649.0)],
            SEED_POSITIONS
        ));
        // Stored values that are wrong on their own.
        assert!(needs_reseed(
            [Some(600.0), Some(496.0), Some(5649.0)],
            SEED_POSITIONS
        ));
        assert!(needs_reseed(
            [Some(0.0), Some(9.0), Some(5.0)],
            SEED_POSITIONS
        ));
        // A stored always-hidden value right of a seeded hidden divider.
        assert!(needs_reseed([None, None, Some(0.5)], SEED_POSITIONS));
    }

    #[test]
    fn macos_27_seeds_keep_the_order() {
        assert_eq!(seed_positions(26), SEED_POSITIONS);
        let seeds = seed_positions(27);
        assert_eq!(seeds, crate::agent_bar::AGENT_SEED_POSITIONS);
        // Fresh install, and an install that ran an earlier version on
        // macOS 27 with the macOS 26 seeds still stored.
        assert!(!needs_reseed([None, None, None], seeds));
        assert!(!needs_reseed([Some(0.0), Some(1.0), None], seeds));
        assert_eq!(
            positions_after_seed([None, None, None], seeds),
            [Some(1.0), Some(2.0), None]
        );
    }

    #[test]
    fn ui_interval_is_clamped() {
        assert_eq!(ui_rehide_seconds(Duration::from_secs(15)), 15.0);
        assert_eq!(ui_rehide_seconds(Duration::ZERO), UI_REHIDE_MIN_SECONDS);
        assert_eq!(
            ui_rehide_seconds(Duration::from_secs(600)),
            UI_REHIDE_MAX_SECONDS
        );
    }

    #[test]
    fn seed_plan_keeps_existing_keys() {
        let plan = seed_plan("IceRust.ControlItem.Visible", Some(0.0), 26, |k| {
            k.contains("Preferred Position")
        });
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|(k, _)| !k.contains("Preferred Position")));
        let none = seed_plan("IceRust.ControlItem.Visible", Some(0.0), 26, |_| true);
        assert!(none.is_empty());
    }
}
