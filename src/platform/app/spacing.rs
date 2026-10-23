//! Applying menu bar item spacing (v1.13, `crate::spacing`): finds the
//! apps with menu bar items, asks before quitting them, writes the keys,
//! quits and reopens the apps (`platform::spacing`), reports the ones that
//! did not come back, and reopens ice-rust last so its own items take the
//! new spacing too (the macOS 27 length cap depends on it).

use std::cell::Cell;
use std::time::Duration;

use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
use objc2_foundation::NSString;

use super::Controller;
use crate::platform::{installer, spacing, timer};
use crate::spacing::{ItemApp, offset_from_spacing};

/// Time for the reopened apps to put their items back before ice-rust
/// quits and reopens.
const SETTLE_BEFORE_RELAUNCH: Duration = Duration::from_millis(1500);

/// "A, B and C".
fn name_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl Controller {
    /// The offset the stored spacing stands for.
    pub(super) fn item_spacing(&self) -> i64 {
        offset_from_spacing(spacing::current_spacing())
    }

    /// Starts applying `offset`; `finished` runs with false when nothing
    /// was changed (cancelled, or ice-rust stays open after a failure to
    /// reopen itself).
    pub(super) fn apply_item_spacing(&self, offset: i64, finished: Box<dyn Fn(bool)>) {
        let me = self.me.clone();
        let finished = Cell::new(Some(finished));
        spacing::find_item_apps(self.mtm, move |apps| {
            let Some(finished) = finished.take() else {
                return;
            };
            match me.upgrade() {
                Some(c) => c.confirm_item_spacing(offset, apps, finished),
                None => finished(false),
            }
        });
    }

    /// Shows an alert; returns which button was pressed, from 0.
    fn alert(&self, message: &str, info: &str, buttons: &[&str]) -> isize {
        let app = NSApplication::sharedApplication(self.mtm);
        app.activate();
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        for button in buttons {
            alert.addButtonWithTitle(&NSString::from_str(button));
        }
        alert.runModal() - NSAlertFirstButtonReturn
    }

    fn confirm_item_spacing(&self, offset: i64, apps: Vec<ItemApp>, finished: Box<dyn Fn(bool)>) {
        let plan = spacing::plan(apps);
        let mut names: Vec<String> = plan.apps.iter().map(|a| a.name.clone()).collect();
        if plan.control_center.is_some() {
            names.push("Control Center".to_string());
        }
        names.push("ice-rust".to_string());
        let info = format!(
            "Apply and Relaunch quits and opens again {} so their items take the new \
             spacing. An app that does not quit within a second is forced to quit, so \
             save your work in them first, and VPN or display apps among them drop \
             their connections for a moment. Apply Only saves the spacing; each app \
             takes it when it next opens, or after you log out and back in.",
            name_list(&names)
        );
        let choice = self.alert(
            "Apply menu bar item spacing?",
            &info,
            &["Apply and Relaunch", "Apply Only", "Cancel"],
        );
        if choice > 1 {
            finished(false);
            return;
        }
        if !spacing::write_offset(offset) {
            let _ = self.alert(
                "The item spacing could not be saved",
                "macOS did not store the new spacing; nothing was changed.",
                &["OK"],
            );
            finished(false);
            return;
        }
        if choice == 1 {
            eprintln!("ice-rust: item spacing offset {offset} saved; apps take it when they open");
            finished(false);
            return;
        }
        eprintln!(
            "ice-rust: item spacing offset {offset}; relaunching {} apps",
            plan.apps.len()
        );
        let me = self.me.clone();
        let finished = Cell::new(Some(finished));
        spacing::relaunch(self.mtm, plan, move |failed| {
            let Some(c) = me.upgrade() else {
                return;
            };
            if !failed.is_empty() {
                let _ = c.alert(
                    "Some apps did not open again",
                    &format!(
                        "{} did not quit or could not be opened again. Quit and open \
                         them yourself, or log out and back in.",
                        name_list(&failed)
                    ),
                    &["OK"],
                );
            }
            let finished = finished.take();
            let me = c.me.clone();
            timer::after(c.mtm, SETTLE_BEFORE_RELAUNCH, move || {
                let Some(c) = me.upgrade() else {
                    return;
                };
                let relaunched = c.relaunch_self();
                if let Some(finished) = &finished {
                    finished(relaunched);
                }
            });
        });
    }

    /// Quits and reopens ice-rust, so its own items take the spacing.
    /// False when it cannot (not running from an app bundle).
    fn relaunch_self(&self) -> bool {
        let Some(bundle) = spacing::own_bundle() else {
            eprintln!(
                "ice-rust: not in an app bundle; quit and open ice-rust to apply the spacing to its own items"
            );
            return false;
        };
        match installer::spawn_relaunch_helper(&bundle) {
            Ok(()) => {
                NSApplication::sharedApplication(self.mtm).terminate(None);
                true
            }
            Err(err) => {
                eprintln!("ice-rust: {err}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::name_list;

    #[test]
    fn names_read_as_a_list() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(name_list(&names(&[])), "");
        assert_eq!(name_list(&names(&["A"])), "A");
        assert_eq!(name_list(&names(&["A", "B"])), "A and B");
        assert_eq!(name_list(&names(&["A", "B", "C"])), "A, B and C");
    }
}
