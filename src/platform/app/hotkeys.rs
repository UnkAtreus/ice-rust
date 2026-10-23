//! Global hotkeys (v1.9): registers the stored hotkeys, runs their actions
//! like the icon's clicks, and suspends them while one of our menus tracks,
//! as Ice does (`HotkeyRegistry.swift`).

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2_app_kit::{NSMenuDidBeginTrackingNotification, NSMenuDidEndTrackingNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};

use super::Controller;
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::platform::hotkeys::{self as carbon, Suspend};
use crate::sections::Section;

#[derive(Default)]
pub(super) struct HotkeyState {
    /// Actions whose combination macOS refused at the last registration.
    refused: RefCell<Vec<HotkeyAction>>,
}

impl Controller {
    pub(super) fn install_hotkeys(&self) {
        let me = self.me.clone();
        carbon::set_handler(move |action| {
            if let Some(c) = me.upgrade() {
                c.hotkey_pressed(action);
            }
        });
        let hotkeys = self.settings.borrow().hotkeys;
        self.register_hotkeys(&hotkeys);

        let center = NSNotificationCenter::defaultCenter();
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: both are immutable notification name constants.
        let names = unsafe {
            [
                (NSMenuDidBeginTrackingNotification, true),
                (NSMenuDidEndTrackingNotification, false),
            ]
        };
        for (name, tracking) in names {
            let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
                carbon::set_suspended(Suspend::Menu, tracking);
            });
            // SAFETY: no object filter; the main queue runs the block on the
            // main thread, where the hotkey registry lives.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&queue),
                    &block,
                )
            };
            self.observers.borrow_mut().push((center.clone(), token));
        }
    }

    pub(super) fn register_hotkeys(&self, hotkeys: &Hotkeys) {
        let refused = carbon::register(hotkeys);
        let count = hotkeys.assigned().count();
        if count > 0 {
            eprintln!(
                "ice-rust: {} hotkey(s) registered, {} refused",
                count - refused.len(),
                refused.len()
            );
        }
        *self.hotkeys.refused.borrow_mut() = refused;
    }

    /// Actions whose combination macOS refused, for the Hotkeys page.
    pub(super) fn refused_hotkeys(&self) -> Vec<HotkeyAction> {
        self.hotkeys.refused.borrow().clone()
    }

    fn hotkey_pressed(&self, action: HotkeyAction) {
        eprintln!("ice-rust: hotkey {}", action.key());
        match action {
            HotkeyAction::ToggleHiddenSection => self.toggle(Section::Hidden),
            HotkeyAction::ToggleAlwaysHiddenSection => self.toggle(Section::AlwaysHidden),
            HotkeyAction::SearchMenuBarItems => self.toggle_search(),
            HotkeyAction::EnableIceBar => self.toggle_ice_bar_mode(),
            HotkeyAction::ShowSectionDividers => {
                self.flip_setting(|s| s.show_section_dividers = !s.show_section_dividers);
            }
            HotkeyAction::ToggleApplicationMenus => self.toggle_app_menus(),
            HotkeyAction::EnableAutoRehide => self.flip_setting(|s| s.auto_rehide = !s.auto_rehide),
        }
    }
}
