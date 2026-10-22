//! App names for menu bar items (see `crate::item_names`). On macOS 26 the
//! owning apps are found through Accessibility on a helper thread; on
//! macOS 27 MenuBarAgent reports each item's process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use objc2::rc::Retained;
use objc2_app_kit::{NSApplicationActivationPolicy, NSImage, NSRunningApplication, NSWorkspace};

use super::menu_bar_space;
use crate::item_names::{NameBook, Owner, is_apple, same_item, shown_name};
use crate::model::{Rect, WindowId};

static BOOK: LazyLock<Mutex<NameBook>> = LazyLock::new(Mutex::default);
/// Set while a lookup runs, so lookups do not pile up behind an app that
/// does not answer.
static LOOKING_UP: AtomicBool = AtomicBool::new(false);

fn book() -> MutexGuard<'static, NameBook> {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears `LOOKING_UP` when the lookup ends, also when it panics.
struct Gate;

impl Drop for Gate {
    fn drop(&mut self) {
        LOOKING_UP.store(false, Ordering::Release);
    }
}

/// The name item `id` shows, given its own name `own`
/// (`crate::item_names::shown_name`): mostly its app's, once found.
pub fn name(id: WindowId, own: &str) -> String {
    shown_name(own, book().owner(id))
}

/// The icon of the app that owns item `id`, once found (`look_up`,
/// `note_owners`), while it runs. Main thread only.
pub fn icon(id: WindowId) -> Option<Retained<NSImage>> {
    let pid = book().owner(id)?.pid;
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.icon()
}

/// `app` as an item owner; `None` for ours, whose items have names.
fn owner(app: &NSRunningApplication) -> Option<Owner> {
    let pid = app.processIdentifier();
    if pid == std::process::id() as i32 {
        return None;
    }
    let bundle = app.bundleIdentifier().map(|id| id.to_string());
    Some(Owner {
        apple: is_apple(bundle.as_deref()),
        name: app
            .localizedName()
            .map(|name| name.to_string())
            .or(bundle)?,
        pid,
    })
}

/// macOS 27: notes the owners of the items by the processes MenuBarAgent
/// reports for them. An item id there stands for one process
/// (`agent_bar::slot_ids`), so an owner once found stays. Main thread
/// only.
pub fn note_owners(owners: &[(WindowId, i32)]) {
    for &(id, pid) in owners {
        if book().owner(id).is_some() {
            continue;
        }
        let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
            continue;
        };
        if let Some(owner) = owner(&app) {
            book().record(id, owner);
        }
    }
}

/// macOS 26: finds on a helper thread the apps that own `windows` (item
/// window ids with their bounds in CG space) not found yet; `name` uses
/// them once it is done. Accessibility asks every app: the first lookup
/// after launch took 1.4 s for 79 apps on macOS 26.7.1, later ones 0.1 s,
/// and the one for the 29 items of Menu Bar Layout there 0.6 s. Main
/// thread only.
pub fn look_up(windows: &[(WindowId, Rect)]) {
    if LOOKING_UP.load(Ordering::Acquire) {
        return;
    }
    let wanted = book().wanted(windows, Instant::now());
    if wanted.is_empty() {
        return;
    }
    // Apps with a user interface, ours left out (`owner`): ours must not be
    // asked off the main thread. NSRunningApplication is read here. Other
    // apps go first: Apple's items mostly keep their names.
    let mut apps: Vec<(i32, Owner)> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() != NSApplicationActivationPolicy::Prohibited)
        .filter_map(|app| Some((app.processIdentifier(), owner(&app)?)))
        .collect();
    apps.sort_by_key(|(_, owner)| owner.apple);
    LOOKING_UP.store(true, Ordering::Release);
    let spawned = thread::Builder::new()
        .name("ice-rust item names".into())
        .spawn(move || {
            let _gate = Gate;
            let mut left = wanted;
            for (pid, owner) in apps {
                if left.is_empty() {
                    break;
                }
                for item in menu_bar_space::extras_item_frames(pid) {
                    left.retain(|(id, window)| {
                        let found = same_item(window, &item);
                        if found {
                            book().record(*id, owner.clone());
                        }
                        !found
                    });
                }
            }
        });
    if spawned.is_err() {
        LOOKING_UP.store(false, Ordering::Release);
    }
}
