//! Hide application menus (v1.6). `crate::app_menus` decides; this module
//! measures whether the shown items reach the frontmost app's menus, hides
//! and restores ice-rust's own menus, switches the activation policy and
//! hands focus back.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSRunningApplication, NSWorkspace, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSOperationQueue, NSTimer};

use super::Controller;
use super::triggers::overlaps;
use crate::app_menus::{
    MenuAction, MenuSituation, items_reach_menus, menu_action, menus_take_item_room,
};
use crate::chain::{DividerLookup, section_chains};
use crate::model::{Rect, WindowRecord};
use crate::platform::{events, menu_bar_space, timer, windows};
use crate::sections::{MenuBarState, Section};

/// When to measure after a change: the items take a moment to move, so a
/// second look follows the first.
const CHECKS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(600)];

#[derive(Default)]
pub(super) struct AppMenus {
    /// ice-rust is the active app with its menus hidden.
    hiding: Cell<bool>,
    /// The app that was in front when hiding began; it gets focus back.
    previous: RefCell<Option<Retained<NSRunningApplication>>>,
    timers: RefCell<Vec<Retained<NSTimer>>>,
}

impl AppMenus {
    pub(super) fn is_hiding(&self) -> bool {
        self.hiding.get()
    }
}

/// Hides or shows every top-level item of ice-rust's main menu. With all of
/// them hidden, the menu bar shows only the Apple menu while ice-rust is
/// active.
fn set_menus_hidden(app: &NSApplication, hidden: bool) {
    if let Some(menu) = app.mainMenu() {
        for item in menu.itemArray().iter() {
            item.setHidden(hidden);
        }
    }
}

impl Controller {
    /// Measures again once the items have moved. Runs after every menu bar
    /// state change, the relevant setting changes and Settings closing.
    pub(super) fn schedule_app_menu_check(&self) {
        for old in self.app_menus.timers.borrow_mut().drain(..) {
            old.invalidate();
        }
        let timers = CHECKS
            .iter()
            .map(|&delay| {
                let me = self.me.clone();
                timer::after(self.mtm, delay, move || {
                    if let Some(c) = me.upgrade() {
                        c.check_app_menus();
                    }
                })
            })
            .collect();
        *self.app_menus.timers.borrow_mut() = timers;
    }

    fn check_app_menus(&self) {
        let enabled = self.settings.borrow().hide_app_menus;
        let use_ice_bar = self.use_ice_bar();
        let mut situation = MenuSituation {
            enabled,
            use_ice_bar,
            section_shown: self.status.state() != MenuBarState::AllHidden,
            settings_open: self
                .settings_window
                .borrow()
                .as_ref()
                .is_some_and(|w| w.is_visible()),
            hiding: self.app_menus.hiding.get(),
            overlap: None,
        };
        if !situation.hiding && situation.may_hide() {
            situation.overlap = self.items_reach_app_menus();
        }
        match menu_action(situation) {
            MenuAction::Hide => self.hide_app_menus(),
            MenuAction::Restore => self.restore_app_menus(true),
            MenuAction::Keep => {}
        }
    }

    /// Ice's test on the menu bar with our icon: the leftmost item at or
    /// left of the end of the frontmost app's menus. The items are the laid
    /// out positions from the full window list (the chain walk): macOS takes
    /// items that collide with the menus off screen (L14). On a display
    /// with a notch the menus must also continue right of it. `None` when
    /// the menus or the dividers cannot be read.
    fn items_reach_app_menus(&self) -> Option<bool> {
        let on_screen = windows::list_on_screen();
        let place = self.menu_bar_place(&on_screen, events::pointer_location())?;
        let b = place.display.bounds;
        let bar = Rect::new(b.x, b.y, b.width, place.menu_bar_height);
        let end = self.app_menus_end(&bar, true)?;
        let notch = menu_bar_space::notches(self.mtm, windows::primary_height())
            .into_iter()
            .find(|n| overlaps(n, &bar));
        if !menus_take_item_room(end, notch.map(|n| n.max_x())) {
            return Some(false);
        }
        let all = windows::list_windows();
        let frames = self.control_frames();
        let DividerLookup::Found(hidden) = self.find_hidden(&all, &frames) else {
            return None;
        };
        let always = match self.find_always_hidden(&all, &frames) {
            DividerLookup::Found(d) => Some(d),
            _ => None,
        };
        let chains = section_chains(&all, &hidden, always.as_ref());
        let state = self.status.state();
        let mut items: Vec<&WindowRecord> = chains.visible.iter().collect();
        if state.is_shown(Section::Hidden) {
            items.extend(&chains.hidden);
            items.push(&hidden);
        }
        if state.is_shown(Section::AlwaysHidden) {
            items.extend(&chains.always_hidden);
            items.extend(always.as_ref());
        }
        let edges = items
            .into_iter()
            .filter(|w| overlaps(&w.bounds, &bar))
            .map(|w| w.bounds.min_x());
        Some(items_reach_menus(edges, end))
    }

    fn hide_app_menus(&self) {
        let app = NSApplication::sharedApplication(self.mtm);
        let current = NSRunningApplication::currentApplication();
        *self.app_menus.previous.borrow_mut() = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .filter(|front| **front != *current);
        set_menus_hidden(&app, true);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        self.app_menus.hiding.set(true);
        eprintln!("ice-rust: hiding application menus to make room for shown items");
        // As for Settings (L2), activation follows the policy change on the
        // next pass. A click on a status item or a scroll over the menu bar
        // goes to the app in front, and since macOS 14 activation is
        // cooperative, so a plain request is refused. Like Ice, ice-rust
        // asks to be activated from the app in front (L14).
        let me = self.me.clone();
        timer::after(self.mtm, Duration::ZERO, move || {
            let Some(c) = me.upgrade() else {
                return;
            };
            if !c.app_menus.hiding.get() {
                return;
            }
            let current = NSRunningApplication::currentApplication();
            let front = c.app_menus.previous.borrow().clone();
            let sent = match front {
                Some(front) => current.activateFromApplication_options(
                    &front,
                    NSApplicationActivationOptions::empty(),
                ),
                None => current.activateWithOptions(NSApplicationActivationOptions::empty()),
            };
            if !sent {
                eprintln!("ice-rust: could not ask to become the active app");
            }
        });
    }

    /// Shows the app menus again: ice-rust's own menus come back, it returns
    /// to an accessory app and, with `refocus`, hands focus back to the app
    /// that was in front. Without `refocus` the user has already moved on.
    pub(super) fn restore_app_menus(&self, refocus: bool) {
        if !self.app_menus.hiding.replace(false) {
            return;
        }
        let app = NSApplication::sharedApplication(self.mtm);
        set_menus_hidden(&app, false);
        let previous = self.app_menus.previous.borrow_mut().take();
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        if refocus
            && let Some(previous) = previous
            && app.isActive()
            && !previous.isTerminated()
        {
            previous.activateWithOptions(NSApplicationActivationOptions::empty());
        }
        eprintln!("ice-rust: showing application menus again");
    }

    /// Settings is opening while the menus are hidden: ice-rust stays a
    /// regular app for the window, with its menus back, and Settings gives
    /// focus back to the app that was in front before the hiding began.
    pub(super) fn hand_app_menus_to_settings(&self) {
        if !self.app_menus.hiding.replace(false) {
            return;
        }
        set_menus_hidden(&NSApplication::sharedApplication(self.mtm), false);
        if let Some(previous) = self.app_menus.previous.borrow_mut().take() {
            *self.previous_app.borrow_mut() = Some(previous);
        }
    }

    /// The user switching to another app ends the hiding; that app's menus
    /// are back and focus stays where the user put it.
    pub(super) fn observe_app_activation(&self) {
        let me = self.me.clone();
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
            let Some(c) = me.upgrade() else {
                return;
            };
            if !c.app_menus.hiding.get() {
                return;
            }
            let current = NSRunningApplication::currentApplication();
            let other_in_front = NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .is_some_and(|front| *front != *current);
            if other_in_front {
                c.restore_app_menus(false);
            }
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: the name is an immutable notification name constant. No
        // object filter; the main queue runs the block on the main thread,
        // which the captured Weak<Controller> (not Send) and AppKit need.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                Some(&queue),
                &block,
            )
        };
        self.observers.borrow_mut().push((center, token));
    }
}
