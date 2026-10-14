//! User settings and the AppKit status item defaults keys (M3). Defaults and
//! key math are pure; reading and writing `NSUserDefaults` is macOS only.

use std::time::Duration;

use crate::model::REHIDE_INTERVAL;

pub const KEY_USE_ICE_BAR: &str = "UseIceBar";
pub const KEY_AUTO_REHIDE: &str = "AutoRehide";
pub const KEY_REHIDE_INTERVAL: &str = "RehideInterval";
pub const KEY_AUTO_CHECK_UPDATES: &str = "AutomaticallyCheckForUpdates";

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
    /// Check GitHub for a new version once a day (v1.3).
    pub auto_check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_ice_bar: true,
            auto_rehide: true,
            rehide_interval: REHIDE_INTERVAL,
            auto_check_updates: true,
        }
    }
}

impl Settings {
    /// Builds settings from a key lookup, falling back to defaults for missing
    /// or invalid values.
    pub fn from_lookup(
        get_bool: impl Fn(&str) -> Option<bool>,
        get_f64: impl Fn(&str) -> Option<f64>,
    ) -> Self {
        let defaults = Self::default();
        let rehide_interval = get_f64(KEY_REHIDE_INTERVAL)
            .filter(|s| s.is_finite() && (0.0..=MAX_REHIDE_SECONDS).contains(s))
            .map(Duration::from_secs_f64)
            .unwrap_or(defaults.rehide_interval);
        Self {
            use_ice_bar: get_bool(KEY_USE_ICE_BAR).unwrap_or(defaults.use_ice_bar),
            auto_rehide: get_bool(KEY_AUTO_REHIDE).unwrap_or(defaults.auto_rehide),
            rehide_interval,
            auto_check_updates: get_bool(KEY_AUTO_CHECK_UPDATES)
                .unwrap_or(defaults.auto_check_updates),
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
pub fn positions_after_seed(stored: [Option<f64>; 3]) -> [Option<f64>; 3] {
    let mut out = SEED_POSITIONS;
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
pub fn needs_reseed(stored: [Option<f64>; 3]) -> bool {
    let [icon, hidden, always] = positions_after_seed(stored);
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
    use objc2_foundation::{NSNumber, NSString, NSUserDefaults};

    use super::{
        KEY_AUTO_CHECK_UPDATES, KEY_AUTO_REHIDE, KEY_REHIDE_INTERVAL, KEY_USE_ICE_BAR, SeedValue,
        Settings,
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
        defaults.setBool_forKey(
            settings.auto_check_updates,
            &NSString::from_str(KEY_AUTO_CHECK_UPDATES),
        );
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
    key_exists, load, read_number, read_string, remove_key, save, write_flag, write_number,
    write_seed, write_string,
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
        let s = Settings::from_lookup(|k| bools.get(k).copied(), |k| nums.get(k).copied());
        assert!(!s.use_ice_bar);
        assert!(!s.auto_rehide);
        assert_eq!(s.rehide_interval, Duration::from_secs_f64(7.5));
    }

    #[test]
    fn update_checks_default_on_and_follow_the_stored_value() {
        assert!(Settings::default().auto_check_updates);
        let off: HashMap<&str, bool> = [(KEY_AUTO_CHECK_UPDATES, false)].into();
        let s = Settings::from_lookup(|k| off.get(k).copied(), |_| None);
        assert!(!s.auto_check_updates);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let s = Settings::from_lookup(|_| None, |_| None);
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn invalid_interval_uses_default() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, MAX_REHIDE_SECONDS + 1.0] {
            let s = Settings::from_lookup(|_| None, |_| Some(bad));
            assert_eq!(s.rehide_interval, Duration::from_secs(15), "value {bad}");
        }
        let s = Settings::from_lookup(|_| None, |_| Some(0.0));
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
        assert_eq!(positions_after_seed([None, None, None]), SEED_POSITIONS);
        assert_eq!(
            positions_after_seed([Some(415.0), Some(496.0), None]),
            [Some(415.0), Some(496.0), None]
        );
        assert_eq!(
            positions_after_seed([None, Some(7.0), Some(9.0)]),
            [Some(0.0), Some(7.0), Some(9.0)]
        );
    }

    #[test]
    fn reseed_checks_the_order_after_seeding() {
        // A missing always-hidden position is accepted (L1): AppKit places
        // that divider.
        assert!(!needs_reseed([Some(415.0), Some(496.0), None]));
        assert!(!needs_reseed([Some(0.0), Some(1.0), None]));
        assert!(!needs_reseed([None, Some(496.0), None]));
        // Fresh install.
        assert!(!needs_reseed([None, None, None]));
        // Grown values in order, as AppKit wrote them for v1.1 on quit.
        assert!(!needs_reseed([Some(259.0), Some(373.0), Some(537.0)]));
        assert!(!needs_reseed([Some(415.0), Some(496.0), Some(5649.0)]));
        // Stored values that are wrong on their own.
        assert!(needs_reseed([Some(600.0), Some(496.0), Some(5649.0)]));
        assert!(needs_reseed([Some(0.0), Some(9.0), Some(5.0)]));
        // A stored always-hidden value right of a seeded hidden divider.
        assert!(needs_reseed([None, None, Some(0.5)]));
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
