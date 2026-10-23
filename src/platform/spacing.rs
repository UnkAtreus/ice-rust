//! Menu bar item spacing (v1.13, `crate::spacing`): reads and writes the
//! spacing keys in the current host's global defaults, finds the apps with
//! menu bar items through Accessibility (their extras menu bars, off the
//! main thread), and quits and reopens them as Ice does: each app is asked
//! to quit, forced after 1 s, and opened again without activating unless it
//! came back on its own; Control Center quits last and reopens itself.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_core_foundation::{
    CFNumber, CFPreferencesCopyValue, CFPreferencesSetValue, CFPreferencesSynchronize, CFString,
    kCFPreferencesAnyApplication, kCFPreferencesCurrentHost, kCFPreferencesCurrentUser,
};
use objc2_foundation::{NSBundle, NSString, NSURL};

use super::{menu_bar_space, timer};
use crate::spacing::{
    ItemApp, KEY_PADDING, KEY_SPACING, RelaunchPlan, item_chrome, relaunch_plan, spacing_value,
};

/// How long an app may take to quit before it is forced to.
const FORCE_AFTER: Duration = Duration::from_secs(1);
/// How long an app may take to quit at all.
const GIVE_UP_AFTER: Duration = Duration::from_secs(3);
/// How often quitting is checked.
const POLL: Duration = Duration::from_millis(100);
/// How long an app that quit gets to come back on its own (launchd
/// restarts some agents) before it is opened.
const COME_BACK: Duration = Duration::from_millis(500);

/// The `NSStatusItemSpacing` stored now, `None` when not set.
pub fn current_spacing() -> Option<f64> {
    let key = CFString::from_static_str(KEY_SPACING);
    // SAFETY: immutable CFString constants exported by CoreFoundation.
    let (app, user, host) = unsafe {
        (
            kCFPreferencesAnyApplication,
            kCFPreferencesCurrentUser,
            kCFPreferencesCurrentHost,
        )
    };
    let value = CFPreferencesCopyValue(&key, app, user, host)?;
    value.downcast_ref::<CFNumber>()?.as_f64()
}

/// How much wider than its length a status item of this process is: the
/// spacing it read when it started, read here once.
pub fn launch_chrome() -> f64 {
    static CHROME: OnceLock<f64> = OnceLock::new();
    *CHROME.get_or_init(|| item_chrome(current_spacing()))
}

/// Writes both keys for `offset`, or removes them for 0.
pub fn write_offset(offset: i64) -> bool {
    // SAFETY: immutable CFString constants exported by CoreFoundation.
    let (app, user, host) = unsafe {
        (
            kCFPreferencesAnyApplication,
            kCFPreferencesCurrentUser,
            kCFPreferencesCurrentHost,
        )
    };
    let value = spacing_value(offset).map(CFNumber::new_i64);
    for key in [KEY_SPACING, KEY_PADDING] {
        let key = CFString::from_static_str(key);
        // SAFETY: a CFNumber is a property list; None removes the key.
        unsafe {
            CFPreferencesSetValue(&key, value.as_deref().map(|v| v.as_ref()), app, user, host);
        }
    }
    CFPreferencesSynchronize(app, user, host)
}

/// The running apps with menu bar items, found on a helper thread; `done`
/// runs on the main thread. Each app's extras menu bar lists its items,
/// folded ones too; ours is left out (it may not be asked off the main
/// thread) and added by the caller's plan anyway.
pub fn find_item_apps(mtm: MainThreadMarker, done: impl FnOnce(Vec<ItemApp>) + 'static) {
    let own = std::process::id() as i32;
    let candidates: Vec<ItemApp> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() != NSApplicationActivationPolicy::Prohibited)
        .filter(|app| app.processIdentifier() != own)
        .map(|app| ItemApp {
            pid: app.processIdentifier(),
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
            name: app
                .localizedName()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("process {}", app.processIdentifier())),
        })
        .collect();
    let found: Arc<Mutex<Option<Vec<ItemApp>>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&found);
    let spawned = thread::Builder::new()
        .name("ice-rust item spacing".into())
        .spawn(move || {
            let with_items = candidates
                .into_iter()
                .filter(|app| menu_bar_space::extras_item_count(app.pid).is_some_and(|n| n > 0))
                .collect();
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(with_items);
        });
    if spawned.is_err() {
        *found.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
    }
    let done = Cell::new(Some(done));
    timer::schedule(mtm, POLL, true, move |timer| {
        let result = found.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(apps) = result {
            timer.invalidate();
            if let Some(done) = done.take() {
                done(apps);
            }
        }
    });
}

/// One app being quit and opened again.
struct Quitting {
    app: ItemApp,
    running: Retained<NSRunningApplication>,
    url: Option<Retained<NSURL>>,
    /// When it quit, once it has.
    quit_at: Option<Instant>,
    forced: bool,
    finished: bool,
    failed: bool,
}

fn running_with_bundle(bundle_id: &str) -> bool {
    !NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .is_empty()
}

fn open_again(url: &NSURL) {
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    configuration.setActivates(false);
    configuration.setAddsToRecentItems(false);
    configuration.setCreatesNewApplicationInstance(false);
    configuration.setPromptsUserIfNeeded(false);
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        url,
        &configuration,
        None,
    );
}

/// Quits every app of `plan` and opens it again, then quits Control
/// Center. `done` gets the names of the apps that did not quit, or quit
/// and could not be opened again.
pub fn relaunch(
    mtm: MainThreadMarker,
    plan: RelaunchPlan,
    done: impl FnOnce(Vec<String>) + 'static,
) {
    let quitting: Vec<Quitting> = plan
        .apps
        .into_iter()
        .filter_map(|app| {
            let running = NSRunningApplication::runningApplicationWithProcessIdentifier(app.pid)?;
            let url = running.bundleURL();
            eprintln!("ice-rust: quitting {} to apply the item spacing", app.name);
            running.terminate();
            Some(Quitting {
                app,
                running,
                url,
                quit_at: None,
                forced: false,
                finished: false,
                failed: false,
            })
        })
        .collect();
    let state = Rc::new(RefCell::new(quitting));
    let started = Instant::now();
    let control_center = plan.control_center;
    let done = Cell::new(Some(done));
    timer::schedule(mtm, POLL, true, move |timer| {
        let mut all = state.borrow_mut();
        for q in all.iter_mut().filter(|q| !q.finished) {
            if !q.running.isTerminated() {
                if started.elapsed() >= GIVE_UP_AFTER {
                    q.finished = true;
                    q.failed = true;
                } else if started.elapsed() >= FORCE_AFTER && !q.forced {
                    q.forced = true;
                    q.running.forceTerminate();
                }
                continue;
            }
            let quit_at = *q.quit_at.get_or_insert_with(Instant::now);
            if quit_at.elapsed() < COME_BACK {
                continue;
            }
            q.finished = true;
            let back = q.app.bundle_id.as_deref().is_some_and(running_with_bundle);
            match (&q.url, back) {
                (_, true) => {}
                (Some(url), false) => open_again(url),
                (None, false) => q.failed = true,
            }
        }
        if !all.iter().all(|q| q.finished) {
            return;
        }
        timer.invalidate();
        let failed: Vec<String> = all
            .iter()
            .filter(|q| q.failed)
            .map(|q| q.app.name.clone())
            .collect();
        if let Some(cc) = &control_center
            && let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(cc.pid)
        {
            eprintln!("ice-rust: quitting Control Center; it opens again on its own");
            if !app.terminate() {
                app.forceTerminate();
            }
        }
        if let Some(done) = done.take() {
            done(failed);
        }
    });
}

/// The plan for the apps found, ice-rust left out.
pub fn plan(apps: Vec<ItemApp>) -> RelaunchPlan {
    relaunch_plan(apps, std::process::id() as i32)
}

/// This app's bundle, when it runs from one.
pub fn own_bundle() -> Option<PathBuf> {
    let path = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
    path.extension()
        .is_some_and(|ext| ext == "app")
        .then_some(path)
}
