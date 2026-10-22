//! App names for menu bar items, the pure half: the tooltips in the Ice Bar
//! and Menu Bar Layout name each item by the app that owns it.
//! `platform::item_names` finds the apps.
//!
//! On macOS 26 every item's window belongs to Control Center and is titled
//! with the autosave name its app chose, such as "Item-0" or a UUID. The
//! owning app is found through Accessibility instead: each app's extras
//! menu bar lists its items, and an item and a window with the same center
//! are the same item (macOS 26.7.1, 2026-10-22: Shottr's item at x 4718,
//! width 24 in its window at x 4711, width 38).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::model::{Rect, WindowId};

/// How far apart an item's center and its window's center may be.
const CENTER_SLACK: f64 = 1.0;
/// How long a window that no app claimed waits before it is looked up
/// again.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The app that owns an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub name: String,
    /// One of Apple's apps (see `shown_name`).
    pub apple: bool,
}

/// Whether an app's item at `item` (Accessibility) fills the window at
/// `window` (window server). Both rectangles are in CG space.
pub fn same_item(window: &Rect, item: &Rect) -> bool {
    (window.mid_x() - item.mid_x()).abs() <= CENTER_SLACK
        && (window.mid_y() - item.mid_y()).abs() <= CENTER_SLACK
}

/// Whether an app is Apple's.
pub fn is_apple(bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|id| id.starts_with("com.apple."))
}

/// Whether an item's own name was made up for it: "Item-0" is what AppKit
/// calls an item whose app gave it no autosave name.
pub fn is_generated(name: &str) -> bool {
    match name.strip_prefix("Item") {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => name.is_empty(),
    }
}

/// The name an item shows, from its own name `own` and its app. Other apps'
/// items take the app's name: apps give them names like "Item-0" or a
/// UUID. Apple's keep their own (Clock, Wi-Fi, Battery), which say more
/// than the app's (Control Center), unless it was made up; one that is a
/// menu extra's identifier is spelled out (`menu_extra_name`).
pub fn shown_name(own: &str, owner: Option<&Owner>) -> String {
    match owner {
        Some(owner) if !owner.apple || is_generated(own) => owner.name.clone(),
        _ => menu_extra_name(own).unwrap_or_else(|| own.to_string()),
    }
}

/// "Time Machine" for SystemUIServer's item named
/// `com.apple.menuextra.TimeMachine` (macOS 26.7.1): the identifier's last
/// part with a space before each capital that follows a lowercase letter,
/// and a capital first letter.
pub fn menu_extra_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("com.apple.menuextra.")?;
    let mut out = String::with_capacity(rest.len() + 4);
    let mut prev_lower = false;
    for (i, c) in rest.chars().enumerate() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        prev_lower = c.is_lowercase();
    }
    (!out.is_empty()).then_some(out)
}

/// The owners found so far, by item.
#[derive(Debug, Default)]
pub struct NameBook {
    owners: HashMap<WindowId, Owner>,
    /// When each window still without an owner was last looked up.
    tried: HashMap<WindowId, Instant>,
}

impl NameBook {
    pub fn owner(&self, id: WindowId) -> Option<&Owner> {
        self.owners.get(&id)
    }

    pub fn record(&mut self, id: WindowId, owner: Owner) {
        self.tried.remove(&id);
        self.owners.insert(id, owner);
    }

    /// The windows among `windows` to look up at `now`: those without an
    /// owner that were not looked up in the last `RETRY_AFTER`. They count
    /// as looked up from now on.
    pub fn wanted(&mut self, windows: &[(WindowId, Rect)], now: Instant) -> Vec<(WindowId, Rect)> {
        self.tried
            .retain(|_, at| now.saturating_duration_since(*at) < RETRY_AFTER);
        let wanted: Vec<(WindowId, Rect)> = windows
            .iter()
            .filter(|(id, _)| !self.owners.contains_key(id) && !self.tried.contains_key(id))
            .copied()
            .collect();
        for (id, _) in &wanted {
            self.tried.insert(*id, now);
        }
        wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> Owner {
        Owner {
            name: name.into(),
            apple: false,
        }
    }

    fn apple(name: &str) -> Owner {
        Owner {
            name: name.into(),
            apple: true,
        }
    }

    #[test]
    fn items_match_windows_by_center() {
        // macOS 26.7.1: Shottr, CodexBar and RunCat in their windows.
        let window = |x, w| Rect::new(x, 0.0, w, 30.0);
        let item = |x, w| Rect::new(x, 3.0, w, 24.0);
        assert!(same_item(&window(4711.0, 38.0), &item(4718.0, 24.0)));
        assert!(same_item(&window(4749.0, 34.0), &item(4748.0, 36.0)));
        assert!(same_item(&window(4783.0, 44.0), &item(4782.0, 46.0)));
        // Hidden, far left of the display.
        assert!(same_item(&window(-6095.0, 38.0), &item(-6096.0, 40.0)));
        // The neighbor, or the same item on another display.
        assert!(!same_item(&window(4711.0, 38.0), &item(4748.0, 36.0)));
        assert!(!same_item(
            &window(4711.0, 38.0),
            &Rect::new(4718.0, 1083.0, 24.0, 24.0)
        ));
    }

    #[test]
    fn apple_apps() {
        assert!(is_apple(Some("com.apple.controlcenter")));
        assert!(is_apple(Some("com.apple.TextInputMenuAgent")));
        assert!(!is_apple(Some("cc.ffitch.shottr")));
        assert!(!is_apple(Some("com.applesauce.app")));
        assert!(!is_apple(None));
    }

    #[test]
    fn generated_names() {
        for name in ["Item-0", "Item-12", "Item", ""] {
            assert!(is_generated(name), "{name}");
        }
        for name in [
            "Item-",
            "Item-a",
            "Items",
            "ItemX",
            "Battery",
            "codexbar-merged",
        ] {
            assert!(!is_generated(name), "{name}");
        }
    }

    #[test]
    fn shown_names() {
        // macOS 26.7.1, Menu Bar Layout tooltips.
        let uuid = "699e423d-73da-42ba-b94e-881ff8672467";
        assert_eq!(shown_name(uuid, Some(&app("Discord"))), "Discord");
        assert_eq!(shown_name("Item-0", Some(&app("Notion"))), "Notion");
        assert_eq!(shown_name("raycastIcon", Some(&app("Raycast"))), "Raycast");
        // Apple's items keep a name of their own.
        assert_eq!(shown_name("WiFi", Some(&apple("Control Center"))), "WiFi");
        assert_eq!(
            shown_name("Item-0", Some(&apple("TextInputMenuAgent"))),
            "TextInputMenuAgent"
        );
        // No owner found (yet).
        assert_eq!(shown_name("Item-0", None), "Item-0");
        // An identifier for a name.
        let ui_server = apple("SystemUIServer");
        assert_eq!(
            shown_name("com.apple.menuextra.TimeMachine", Some(&ui_server)),
            "Time Machine"
        );
        assert_eq!(
            shown_name("com.apple.menuextra.TimeMachine", None),
            "Time Machine"
        );
    }

    #[test]
    fn menu_extra_names() {
        let name = |id| menu_extra_name(id);
        assert_eq!(
            name("com.apple.menuextra.TimeMachine").as_deref(),
            Some("Time Machine")
        );
        assert_eq!(
            name("com.apple.menuextra.bluetooth").as_deref(),
            Some("Bluetooth")
        );
        assert_eq!(name("com.apple.menuextra.VPN").as_deref(), Some("VPN"));
        assert_eq!(name("com.apple.menuextra."), None);
        assert_eq!(name("WiFi"), None);
        assert_eq!(name("AirDrop"), None);
    }

    #[test]
    fn found_windows_are_not_looked_up_again() {
        let mut book = NameBook::default();
        let t0 = Instant::now();
        let a = (1, Rect::new(0.0, 0.0, 30.0, 30.0));
        let b = (2, Rect::new(30.0, 0.0, 30.0, 30.0));
        assert_eq!(book.wanted(&[a, b], t0), vec![a, b]);
        book.record(1, app("Shottr"));
        assert_eq!(book.owner(1), Some(&app("Shottr")));
        assert_eq!(book.owner(2), None);
        // Window 2 was looked up and not claimed: it waits.
        assert!(book.wanted(&[a, b], t0 + Duration::from_secs(5)).is_empty());
        assert_eq!(book.wanted(&[a, b], t0 + RETRY_AFTER), vec![b]);
        // A window that is new is looked up at once.
        let c = (3, Rect::new(60.0, 0.0, 30.0, 30.0));
        assert_eq!(book.wanted(&[a, b, c], t0 + RETRY_AFTER), vec![c]);
    }

    #[test]
    fn a_late_owner_ends_the_wait() {
        let mut book = NameBook::default();
        let t0 = Instant::now();
        let a = (1, Rect::new(0.0, 0.0, 30.0, 30.0));
        assert_eq!(book.wanted(&[a], t0), vec![a]);
        book.record(1, app("Notion"));
        assert!(book.wanted(&[a], t0 + RETRY_AFTER).is_empty());
    }
}
