//! Applying menu bar item spacing (v1.13, `crate::spacing`): finds the
//! apps with menu bar items, asks before quitting them, writes the keys,
//! quits and reopens the apps (`platform::spacing`), reports the ones that
//! did not come back, and reopens ice-rust last so its own items take the
//! new spacing too (the macOS 27 length cap depends on it).

use std::cell::Cell;
use std::time::Duration;

use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSModalResponse,
};
use objc2_foundation::NSString;

use super::Controller;
use crate::platform::{installer, spacing, timer};
use crate::spacing::{ItemApp, SpacingChoice, offset_from_spacing, spacing_choice};

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

    /// Shows an alert and returns its response. The first button answers
    /// Return, a "Cancel" button Escape; `destructive` marks the second
    /// button, which then has no key.
    fn alert(
        &self,
        message: &str,
        info: &str,
        buttons: &[&str],
        destructive: bool,
    ) -> NSModalResponse {
        let app = NSApplication::sharedApplication(self.mtm);
        app.activate();
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        for (i, title) in buttons.iter().enumerate() {
            let button = alert.addButtonWithTitle(&NSString::from_str(title));
            if i == 1 && destructive {
                button.setHasDestructiveAction(true);
                button.setKeyEquivalent(&NSString::new());
            }
            if *title == "Cancel" {
                button.setKeyEquivalent(&NSString::from_str("\u{1b}"));
            }
        }
        alert.runModal()
    }

    fn confirm_item_spacing(&self, offset: i64, apps: Vec<ItemApp>, finished: Box<dyn Fn(bool)>) {
        let plan = spacing::plan(apps);
        let mut names: Vec<String> = plan.apps.iter().map(|a| a.name.clone()).collect();
        if plan.control_center.is_some() {
            names.push("Control Center".to_string());
        }
        names.push("ice-rust".to_string());
        let info = format!(
            "Apply Only saves the spacing; each app takes it when it next opens, or \
             after you log out and back in. Apply and Relaunch also quits and opens \
             again {} so their items take it now. An app that does not quit within a \
             second is forced to quit, so save your work in them first, and VPN or \
             display apps among them drop their connections for a moment.",
            name_list(&names)
        );
        let response = self.alert(
            "Apply menu bar item spacing?",
            &info,
            &["Apply Only", "Apply and Relaunch", "Cancel"],
            true,
        );
        let choice = spacing_choice(response, NSAlertFirstButtonReturn);
        eprintln!("ice-rust: item spacing dialog answered {response} ({choice:?})");
        if choice == SpacingChoice::Cancel {
            finished(false);
            return;
        }
        if !spacing::write_offset(offset) {
            self.alert(
                "The item spacing could not be saved",
                "macOS did not store the new spacing; nothing was changed.",
                &["OK"],
                false,
            );
            finished(false);
            return;
        }
        if choice == SpacingChoice::SaveOnly {
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
                c.alert(
                    "Some apps did not open again",
                    &format!(
                        "{} did not quit or could not be opened again. Quit and open \
                         them yourself, or log out and back in.",
                        name_list(&failed)
                    ),
                    &["OK"],
                    false,
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
