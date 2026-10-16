//! The three status items: the icon, the hidden-section divider and the
//! always-hidden-section divider. On macOS 27 a fourth, the spacer, sits
//! directly left of the hidden divider (v1.8, `crate::agent_bar`).

use std::cell::{Cell, RefCell};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSAccessibility, NSEventMask, NSImage, NSMenu, NSStatusBar, NSStatusBarButton, NSStatusItem,
};
use objc2_foundation::{NSProcessInfo, NSString};

use super::menu_icon::IconImages;
use crate::agent_bar::{self, AgentLengths, Spacer};
use crate::model::{
    ALWAYS_HIDDEN_AUTOSAVE, ALWAYS_HIDDEN_SPACER_AUTOSAVE, DIVIDER_AUTOSAVE, HIDE_LENGTH,
    ICON_AUTOSAVE, Rect, SPACER_AUTOSAVE, VARIABLE_LENGTH,
};
use crate::sections::MenuBarState;
use crate::settings::{self, StatusKey};

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

/// The spacer's length while it is dragged back next to the divider.
const SPACER_HANDLE_LENGTH: f64 = 20.0;

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

/// Expanded to `length` (10000 pt before macOS 27, pushing items left of
/// it off screen), or collapsed with `None`: variable length with a
/// chevron (M1).
fn set_divider(
    mtm: MainThreadMarker,
    item: &NSStatusItem,
    length: Option<f64>,
    symbol_name: &str,
    fallback: &str,
) {
    item.setLength(length.unwrap_or(VARIABLE_LENGTH));
    if let Some(button) = item.button(mtm) {
        if length.is_some() {
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
    /// macOS 27 only, `Spacer::index` order: each grows with its divider,
    /// so the two together can fill more than half the display (v1.8).
    spacers: [Option<Retained<NSStatusItem>>; 2],
    state: Cell<MenuBarState>,
    /// The icon's images for the chosen style (v1.4); `None` shows a
    /// symbol or "...".
    icon_images: RefCell<Option<IconImages>>,
    icon_active: Cell<bool>,
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
    pub unsafe fn create(
        mtm: MainThreadMarker,
        target: &AnyObject,
        action: Sel,
        icon_images: Option<IconImages>,
    ) -> Self {
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
        let os = os_major();
        let agent = agent_bar::uses_agent_menu_bar(os);
        let seeds = settings::seed_positions(os);
        if settings::needs_reseed(stored_positions(), seeds) {
            for name in NAMES {
                settings::remove_key(&position_key(name));
            }
        }
        for (name, position) in NAMES.iter().zip(seeds) {
            seed_defaults(name, position);
        }
        if agent {
            // MenuBarAgent takes an app's seed only for an item it has not
            // placed yet; this puts a new spacer directly left of its
            // divider. The always-hidden divider has no seed, so neither has
            // its spacer: created right after it, the spacer lands left of
            // it at the far left like the divider.
            let hidden = settings::read_number(&position_key(DIVIDER_AUTOSAVE));
            seed_defaults(SPACER_AUTOSAVE, Some(agent_bar::spacer_seed(hidden)));
            let always = settings::read_number(&position_key(ALWAYS_HIDDEN_AUTOSAVE));
            seed_defaults(ALWAYS_HIDDEN_SPACER_AUTOSAVE, always.map(|p| p + 1.0));
        }

        let bar = NSStatusBar::systemStatusBar();
        let make = |name: &str| {
            let item = bar.statusItemWithLength(VARIABLE_LENGTH);
            let name = NSString::from_str(name);
            item.setAutosaveName(Some(&name));
            // On macOS 27 MenuBarAgent's Accessibility tree names each item
            // by this, which finds our items without waiting for AppKit's
            // frames to catch up (v1.8).
            if let Some(button) = item.button(mtm) {
                button.setAccessibilityIdentifier(Some(&name));
            }
            item
        };
        let icon = make(ICON_AUTOSAVE);
        let hidden = make(DIVIDER_AUTOSAVE);
        // Untitled at length 0 it is a 16 pt blank. It stays visible: a
        // hidden item keeps an old position value while MenuBarAgent
        // recomputes the others, and could end up right of the divider.
        let make_spacer = |name: &str| {
            agent.then(|| {
                let spacer = make(name);
                spacer.setLength(0.0);
                spacer
            })
        };
        let spacer = make_spacer(SPACER_AUTOSAVE);
        let always_hidden = make(ALWAYS_HIDDEN_AUTOSAVE);
        let always_spacer = make_spacer(ALWAYS_HIDDEN_SPACER_AUTOSAVE);
        let spacers = [spacer, always_spacer];
        // The seeds only fill missing keys, so a stored `Visible` false
        // would leave an item out of the menu bar, and every section needs
        // all three (issue #1).
        for item in [&icon, &hidden, &always_hidden]
            .into_iter()
            .chain(spacers.iter().flatten())
        {
            if !item.isVisible() {
                item.setVisible(true);
            }
        }

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
            spacers,
            state: Cell::new(MenuBarState::AllShown),
            icon_images: RefCell::new(icon_images),
            icon_active: Cell::new(true),
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
        self.frame_of(&self.icon)
    }

    /// Frame of the hidden divider's window in AppKit space.
    pub fn hidden_frame(&self) -> Option<Rect> {
        self.frame_of(&self.hidden)
    }

    /// Frame of the always-hidden divider's window in AppKit space.
    pub fn always_hidden_frame(&self) -> Option<Rect> {
        self.frame_of(&self.always_hidden)
    }

    /// Whether window number `number` belongs to one of our items.
    pub fn owns_window(&self, number: isize) -> bool {
        [&self.icon, &self.hidden, &self.always_hidden]
            .into_iter()
            .chain(self.spacers.iter().flatten())
            .filter_map(|item| item.button(self.mtm)?.window())
            .any(|window| window.windowNumber() == number)
    }

    /// Gives the spacer something to grab (on) or makes it a blank 16 pt
    /// slot again (off). A Command-drag does not move an untitled item of
    /// length 0 (macOS 27, v1.8).
    pub fn set_spacer_handle(&self, which: Spacer, on: bool) {
        let Some(spacer) = &self.spacers[which.index()] else {
            return;
        };
        spacer.setLength(if on { SPACER_HANDLE_LENGTH } else { 0.0 });
        if let Some(button) = spacer.button(self.mtm) {
            set_look(&button, None, if on { "|" } else { "" });
        }
    }

    /// Frame of the spacer's window in AppKit space (macOS 27).
    pub fn spacer_frame(&self, which: Spacer) -> Option<Rect> {
        self.frame_of(self.spacers[which.index()].as_ref()?)
    }

    fn frame_of(&self, item: &NSStatusItem) -> Option<Rect> {
        let frame = item.button(self.mtm)?.window()?.frame();
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
        let expanded = |on: bool| on.then_some(HIDE_LENGTH);
        self.set_lengths(
            state,
            expanded(lengths.hidden_expanded),
            [None, None],
            expanded(lengths.always_hidden_expanded),
        );
    }

    /// `set_state` for the macOS 27 menu bar, with lengths from
    /// `agent_bar::agent_lengths` (v1.8).
    pub fn set_agent_state(&self, state: MenuBarState, lengths: AgentLengths) {
        self.set_lengths(
            state,
            lengths.hidden,
            [Some(lengths.spacer), Some(lengths.always_spacer)],
            lengths.always_hidden,
        );
    }

    fn set_lengths(
        &self,
        state: MenuBarState,
        hidden: Option<f64>,
        spacers: [Option<f64>; 2],
        always_hidden: Option<f64>,
    ) {
        set_divider(self.mtm, &self.hidden, hidden, "chevron.left", "<");
        for (item, length) in self.spacers.iter().zip(spacers) {
            if let (Some(item), Some(length)) = (item, length) {
                item.setLength(length);
            }
        }
        set_divider(
            self.mtm,
            &self.always_hidden,
            always_hidden,
            "chevron.left.2",
            "<<",
        );
        self.state.set(state);
    }

    /// The shown image while a section is revealed or the Ice Bar is
    /// open, the hidden image otherwise.
    pub fn set_icon_active(&self, active: bool) {
        self.icon_active.set(active);
        self.show_icon();
    }

    /// Switches the icon to new images, keeping its state.
    pub fn set_icon_images(&self, images: Option<IconImages>) {
        *self.icon_images.borrow_mut() = images;
        self.show_icon();
    }

    fn show_icon(&self) {
        let Some(button) = self.icon.button(self.mtm) else {
            return;
        };
        let active = self.icon_active.get();
        if let Some(images) = &*self.icon_images.borrow() {
            button.setImage(Some(if active {
                &images.shown
            } else {
                &images.hidden
            }));
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
