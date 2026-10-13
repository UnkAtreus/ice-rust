//! The three status items: the icon, the hidden-section divider and the
//! always-hidden-section divider.

use std::cell::Cell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{NSEventMask, NSImage, NSMenu, NSStatusBar, NSStatusBarButton, NSStatusItem};
use objc2_foundation::{NSProcessInfo, NSString};

use super::brand;
use crate::model::{
    ALWAYS_HIDDEN_AUTOSAVE, DIVIDER_AUTOSAVE, HIDE_LENGTH, ICON_AUTOSAVE, Rect, VARIABLE_LENGTH,
};
use crate::sections::MenuBarState;
use crate::settings::{self, SEED_POSITIONS, StatusKey};

/// The macOS major version, or 0 when unknown.
pub fn os_major() -> u32 {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    u32::try_from(version.majorVersion).unwrap_or(0)
}

/// Writes the AppKit position and visibility keys that are still missing
/// (M3).
fn seed_defaults(autosave_name: &str, position: Option<f64>) {
    for (key, value) in
        settings::seed_plan(autosave_name, position, os_major(), settings::key_exists)
    {
        settings::write_seed(&key, &value);
    }
}

fn position_key(autosave_name: &str) -> String {
    settings::status_item_key(StatusKey::PreferredPosition, autosave_name)
}

/// Our autosave names, right to left.
const NAMES: [&str; 3] = [ICON_AUTOSAVE, DIVIDER_AUTOSAVE, ALWAYS_HIDDEN_AUTOSAVE];

/// Stored `Preferred Position` of the icon, the hidden divider and the
/// always-hidden divider (M3).
pub fn stored_positions() -> [Option<f64>; 3] {
    NAMES.map(|name| settings::read_number(&position_key(name)))
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )?;
    image.setTemplate(true);
    Some(image)
}

fn set_look(button: &NSStatusBarButton, symbol_name: Option<&str>, fallback_title: &str) {
    match symbol_name.and_then(|name| symbol(name, fallback_title)) {
        Some(image) => {
            button.setImage(Some(&image));
            button.setTitle(&NSString::new());
        }
        None => {
            button.setImage(None);
            button.setTitle(&NSString::from_str(fallback_title));
        }
    }
}

/// Expanded: 10000 pt, pushing items left of it off screen. Collapsed:
/// variable length with a chevron (M1).
fn set_divider(
    mtm: MainThreadMarker,
    item: &NSStatusItem,
    expanded: bool,
    symbol_name: &str,
    fallback: &str,
) {
    item.setLength(if expanded {
        HIDE_LENGTH
    } else {
        VARIABLE_LENGTH
    });
    if let Some(button) = item.button(mtm) {
        if expanded {
            button.setImage(None);
            button.setTitle(&NSString::new());
        } else {
            set_look(&button, Some(symbol_name), fallback);
        }
    }
}

pub struct StatusItems {
    mtm: MainThreadMarker,
    icon: Retained<NSStatusItem>,
    hidden: Retained<NSStatusItem>,
    always_hidden: Retained<NSStatusItem>,
    state: Cell<MenuBarState>,
}

impl StatusItems {
    /// Creates the three items with every section shown; the controller
    /// hides them through its order check. Left mouse down and right mouse
    /// up on the icon send `action` to `target`.
    ///
    /// # Safety
    ///
    /// `target` must stay alive as long as the status items, and must
    /// implement `action` with the signature `-(void)action:(id)sender`.
    pub unsafe fn create(mtm: MainThreadMarker, target: &AnyObject, action: Sel) -> Self {
        // v1.1 seeded the always-hidden divider directly left of the hidden
        // divider (L1). Its stored position is removed once, before the
        // items exist, so AppKit places that divider instead.
        let always_key = position_key(ALWAYS_HIDDEN_AUTOSAVE);
        let migration = settings::placement_migration(
            settings::key_exists(settings::KEY_ALWAYS_HIDDEN_PLACEMENT),
            settings::key_exists(&always_key),
        );
        if migration.remove_position {
            settings::remove_key(&always_key);
        }
        if migration.set_flag {
            settings::write_flag(settings::KEY_ALWAYS_HIDDEN_PLACEMENT);
        }
        // An order other than icon, hidden divider, always-hidden divider
        // from right to left would hide the icon or mix the sections, so all
        // three keys are removed and the icon and hidden divider reseeded.
        // The check covers the seeds about to be written for missing keys.
        if settings::needs_reseed(stored_positions()) {
            for name in NAMES {
                settings::remove_key(&position_key(name));
            }
        }
        for (name, position) in NAMES.iter().zip(SEED_POSITIONS) {
            seed_defaults(name, position);
        }

        let bar = NSStatusBar::systemStatusBar();
        let make = |name: &str| {
            let item = bar.statusItemWithLength(VARIABLE_LENGTH);
            item.setAutosaveName(Some(&NSString::from_str(name)));
            item
        };
        let icon = make(ICON_AUTOSAVE);
        let hidden = make(DIVIDER_AUTOSAVE);
        let always_hidden = make(ALWAYS_HIDDEN_AUTOSAVE);

        if let Some(button) = icon.button(mtm) {
            // SAFETY: the caller guarantees the target's lifetime and the
            // action's signature.
            unsafe {
                button.setTarget(Some(target));
                button.setAction(Some(action));
            }
            button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseUp);
            button.setToolTip(Some(&NSString::from_str(
                "ice-rust: click for hidden items, Option-click for always-hidden items",
            )));
        }

        let items = Self {
            mtm,
            icon,
            hidden,
            always_hidden,
            state: Cell::new(MenuBarState::AllShown),
        };
        // Collapsed: nothing expands before the live order is known.
        items.set_state(MenuBarState::AllShown);
        items.set_icon_active(true);
        items
    }

    pub fn state(&self) -> MenuBarState {
        self.state.get()
    }

    /// Whether everything is hidden.
    pub fn is_hidden(&self) -> bool {
        self.state.get() == MenuBarState::AllHidden
    }

    /// Frame of the icon button's window in AppKit space. Unlike the CG
    /// window title, reading it needs no TCC grant.
    pub fn icon_frame(&self) -> Option<Rect> {
        let frame = self.icon.button(self.mtm)?.window()?.frame();
        Some(Rect::new(
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        ))
    }

    /// Sets both divider lengths for `state`. The hidden divider collapses
    /// to a single chevron, the always-hidden divider to a double chevron.
    /// No order check here; the controller's `set_menu_bar_state` makes it.
    pub fn set_state(&self, state: MenuBarState) {
        let lengths = state.lengths();
        set_divider(
            self.mtm,
            &self.hidden,
            lengths.hidden_expanded,
            "chevron.left",
            "<",
        );
        set_divider(
            self.mtm,
            &self.always_hidden,
            lengths.always_hidden_expanded,
            "chevron.left.2",
            "<<",
        );
        self.state.set(state);
    }

    /// Filled icon while a section is revealed or the Ice Bar is open.
    pub fn set_icon_active(&self, active: bool) {
        if let Some(button) = self.icon.button(self.mtm) {
            if let Some(glyph) = brand::menu_glyph(active) {
                button.setImage(Some(&glyph));
                button.setTitle(&NSString::new());
                return;
            }
            let name = if active {
                "ellipsis.circle.fill"
            } else {
                "ellipsis.circle"
            };
            set_look(&button, Some(name), "...");
        }
    }

    /// Pops up `menu` from the icon.
    pub fn show_menu(&self, menu: &NSMenu) {
        self.icon.setMenu(Some(menu));
        if let Some(button) = self.icon.button(self.mtm) {
            // SAFETY: a nil sender is allowed.
            unsafe { button.performClick(None) };
        }
        self.icon.setMenu(None);
    }
}
