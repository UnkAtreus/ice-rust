//! The Hotkeys page of the Settings window (v1.9): one row per action with
//! a field that records a shortcut and a button that clears it. While a
//! field records, our hotkeys are off and a local key-down monitor takes
//! the next key: Escape cancels, Delete clears, anything else is checked
//! (`crate::hotkeys::check_combo`) and saved.

use std::cell::{Cell, RefCell};
use std::ptr::{self, NonNull};
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSButton, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSView};
use objc2_foundation::NSString;

use super::hotkeys::{self as carbon, Suspend};
use super::settings_window::{MARGIN, SettingsHost, row_text, separator};
use super::status;
use super::ui::{
    ActionTarget, BoxStyle, Follow, RoundedBox, Weight, add, label, rect, symbol, wrapping_label,
};
use crate::hotkeys::{
    HotkeyAction, KEY_DELETE, KEY_ESCAPE, KeyCombo, check_combo, is_modifier_key,
};
use crate::triggers::Modifiers;

const ROW: f64 = 56.0;
const FIELD_WIDTH: f64 = 150.0;
const RECORD_PROMPT: &str = "Record Shortcut";
const RECORDING_PROMPT: &str = "Press a shortcut";

struct HotkeyRow {
    action: HotkeyAction,
    field: Retained<NSButton>,
    clear: Retained<NSButton>,
}

pub struct HotkeysPane {
    me: Weak<HotkeysPane>,
    host: Rc<dyn SettingsHost>,
    rows: Vec<HotkeyRow>,
    message: Retained<objc2_app_kit::NSTextField>,
    recording: Cell<Option<HotkeyAction>>,
    monitor: RefCell<Option<Retained<AnyObject>>>,
    _targets: Vec<Retained<ActionTarget>>,
}

fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        option: flags.contains(NSEventModifierFlags::Option),
        command: flags.contains(NSEventModifierFlags::Command),
    }
}

impl HotkeysPane {
    pub fn build(
        mtm: MainThreadMarker,
        page: &NSView,
        width: f64,
        host: Rc<dyn SettingsHost>,
    ) -> Rc<HotkeysPane> {
        Rc::new_cyclic(|me: &Weak<HotkeysPane>| {
            add(
                page,
                &wrapping_label(
                    mtm,
                    "Press a shortcut in any app to run these actions. ice-rust has none by \
                     default. Hotkeys need no permission.",
                    rect(MARGIN + 4.0, 6.0, width - 8.0, 34.0),
                    12.0,
                    &NSColor::secondaryLabelColor(),
                ),
                Follow::Width,
            );
            let actions = HotkeyAction::ALL;
            let group = RoundedBox::new(
                mtm,
                rect(MARGIN, 48.0, width, actions.len() as f64 * ROW),
                BoxStyle::Group,
            );
            add(page, &group, Follow::Width);
            let mut rows = Vec::new();
            let mut targets = Vec::new();
            for (i, action) in actions.into_iter().enumerate() {
                let y = i as f64 * ROW;
                if i > 0 {
                    separator(mtm, &group, y, width);
                }
                row_text(
                    mtm,
                    &group,
                    action.title(),
                    action.detail(),
                    16.0,
                    y + 11.0,
                    width - FIELD_WIDTH - 80.0,
                );
                // SAFETY: no target or action yet; `attach` sets both.
                let field = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str(RECORD_PROMPT),
                        None,
                        None,
                        mtm,
                    )
                };
                field.setFrame(rect(
                    width - FIELD_WIDTH - 52.0,
                    y + 13.0,
                    FIELD_WIDTH,
                    30.0,
                ));
                add(&group, &field, Follow::Right);
                // SAFETY: as above.
                let clear = unsafe {
                    NSButton::buttonWithTitle_target_action(&NSString::new(), None, None, mtm)
                };
                if let Some(image) = symbol("xmark.circle.fill", 13.0, Weight::Regular) {
                    clear.setImage(Some(&image));
                }
                clear.setBordered(false);
                clear.setToolTip(Some(&NSString::from_str("Clear the shortcut")));
                clear.setFrame(rect(width - 46.0, y + 16.0, 24.0, 24.0));
                add(&group, &clear, Follow::Right);

                let pane = me.clone();
                let on_field = ActionTarget::new(mtm, move |_| {
                    if let Some(p) = pane.upgrade() {
                        p.field_clicked(action);
                    }
                });
                on_field.attach(&field);
                let pane = me.clone();
                let on_clear = ActionTarget::new(mtm, move |_| {
                    if let Some(p) = pane.upgrade() {
                        p.stop_recording();
                        p.save(action, None);
                    }
                });
                on_clear.attach(&clear);
                targets.push(on_field);
                targets.push(on_clear);
                rows.push(HotkeyRow {
                    action,
                    field,
                    clear,
                });
            }
            let below = 48.0 + actions.len() as f64 * ROW + 12.0;
            add(
                page,
                &wrapping_label(
                    mtm,
                    "Click a field, then press the keys. Use Command or Control in a shortcut, \
                     for example ⌃⌥H; Escape cancels and Delete clears.",
                    rect(MARGIN + 4.0, below, width - 8.0, 34.0),
                    12.0,
                    &NSColor::secondaryLabelColor(),
                ),
                Follow::Width,
            );
            let message = label(
                mtm,
                "",
                rect(MARGIN + 4.0, below + 40.0, width - 8.0, 17.0),
                12.0,
                Weight::Medium,
                &NSColor::systemRedColor(),
            );
            add(page, &message, Follow::Width);
            HotkeysPane {
                me: me.clone(),
                host,
                rows,
                message,
                recording: Cell::new(None),
                monitor: RefCell::new(None),
                _targets: targets,
            }
        })
    }

    /// Shows the stored shortcuts.
    pub fn sync(&self) {
        let hotkeys = self.host.settings().hotkeys;
        for row in &self.rows {
            let combo = hotkeys.get(row.action);
            let title = if self.recording.get() == Some(row.action) {
                RECORDING_PROMPT.to_string()
            } else {
                combo.map_or_else(
                    || RECORD_PROMPT.to_string(),
                    |c| c.display(&carbon::key_name(c.key_code)),
                )
            };
            row.field.setTitle(&NSString::from_str(&title));
            row.clear
                .setHidden(combo.is_none() || !row.field.isEnabled());
        }
    }

    fn set_message(&self, text: &str) {
        self.message.setStringValue(&NSString::from_str(text));
    }

    fn field_clicked(&self, action: HotkeyAction) {
        let again = self.recording.get() == Some(action);
        self.stop_recording();
        if !again {
            self.start_recording(action);
        }
    }

    fn start_recording(&self, action: HotkeyAction) {
        self.set_message("");
        self.recording.set(Some(action));
        carbon::set_suspended(Suspend::Recording, true);
        let me = self.me.clone();
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a live event for the duration of the call.
            let taken = me
                .upgrade()
                .is_some_and(|pane| pane.key_down(unsafe { event.as_ref() }));
            if taken {
                ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        // SAFETY: the handler returns either the event or null, which
        // swallows it, as the API allows.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
        };
        *self.monitor.borrow_mut() = monitor;
        self.sync();
    }

    /// Ends a recording without saving; hotkeys work again.
    pub fn stop_recording(&self) {
        if let Some(monitor) = self.monitor.borrow_mut().take() {
            // SAFETY: the monitor came from addLocalMonitor and is removed once.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if self.recording.take().is_some() {
            carbon::set_suspended(Suspend::Recording, false);
        }
        self.sync();
    }

    /// Handles a key while recording. Returns true when the key was taken.
    fn key_down(&self, event: &NSEvent) -> bool {
        let Some(action) = self.recording.get() else {
            return false;
        };
        let combo = KeyCombo {
            key_code: event.keyCode(),
            modifiers: modifiers(event.modifierFlags()),
        };
        let bare = combo.modifiers == Modifiers::default();
        if is_modifier_key(combo.key_code) {
            return true;
        }
        if bare && combo.key_code == KEY_ESCAPE {
            self.stop_recording();
            return true;
        }
        if bare && combo.key_code == KEY_DELETE {
            self.stop_recording();
            self.save(action, None);
            return true;
        }
        let hotkeys = self.host.settings().hotkeys;
        let checked = check_combo(
            combo,
            action,
            &hotkeys,
            status::os_major(),
            carbon::is_system_shortcut,
        );
        self.stop_recording();
        match checked {
            Ok(()) => self.save(action, Some(combo)),
            Err(problem) => self.set_message(&format!(
                "{}: {problem}",
                combo.display(&carbon::key_name(combo.key_code))
            )),
        }
        true
    }

    fn save(&self, action: HotkeyAction, combo: Option<KeyCombo>) {
        let mut settings = self.host.settings();
        settings.hotkeys.set(action, combo);
        self.host.apply_settings(&settings);
        if let Some(combo) = combo
            && self.host.refused_hotkeys().contains(&action)
        {
            self.set_message(&format!(
                "macOS did not accept {}; another app may use it",
                combo.display(&carbon::key_name(combo.key_code))
            ));
        } else {
            self.set_message("");
        }
        self.sync();
    }
}

impl Drop for HotkeysPane {
    fn drop(&mut self) {
        self.stop_recording();
    }
}
