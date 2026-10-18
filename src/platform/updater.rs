//! Automatic updates (v1.3), the app side: when to check, the prompt, and
//! driving `installer` on a worker thread. Everything here runs on the
//! main thread; network and file work runs on worker threads and reports
//! back through a channel that a main-thread timer drains.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSAutoresizingMaskOptions,
    NSBorderType, NSFont, NSScrollView, NSTextView, NSWorkspace,
};
use objc2_foundation::{
    NSDate, NSDateFormatter, NSDateFormatterStyle, NSPoint, NSRect, NSSize, NSString, NSTimer,
    NSURL,
};

use super::installer;
use super::permissions;
use super::timer;
use crate::settings;
use crate::update::{
    self, CheckOutcome, DUE_POLL_INTERVAL, FIRST_CHECK_DELAY, KEY_LAST_LAUNCHED_VERSION,
    KEY_LAST_UPDATE_CHECK, KEY_SKIPPED_VERSION, MSG_UP_TO_DATE, PERMISSIONS_NOTE, Update,
    UpdateStatus, Version, bundle_of_executable, check_due, epoch_seconds, from_epoch_seconds,
    permission_notice, precheck, prompt_title, should_offer, status_line,
};

/// How often the main thread looks for a worker's result.
const POLL: Duration = Duration::from_millis(250);
/// Wait after launch before the permission notice, so the system's own
/// permission prompts come first.
const PERMISSION_NOTICE_DELAY: Duration = Duration::from_secs(5);
const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

enum Message {
    Checked {
        manual: bool,
        result: Result<CheckOutcome, String>,
    },
    Installed {
        version: Version,
        bundle: PathBuf,
        result: Result<(), String>,
    },
}

pub struct Updater {
    mtm: MainThreadMarker,
    me: Weak<Updater>,
    /// The "Automatically check for updates" setting, read at each check.
    enabled: Box<dyn Fn() -> bool>,
    status: RefCell<UpdateStatus>,
    launched_at: SystemTime,
    last_attempt: Cell<Option<SystemTime>>,
    inbox: RefCell<Option<Receiver<Message>>>,
    poll: RefCell<Option<Retained<NSTimer>>>,
    timers: RefCell<Vec<Retained<NSTimer>>>,
    on_change: RefCell<Option<Box<dyn Fn()>>>,
}

fn open_url(url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// The bundle this process runs from, if any.
fn running_bundle() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundle_of_executable(&exe))
}

fn last_success() -> Option<SystemTime> {
    settings::read_number(KEY_LAST_UPDATE_CHECK).and_then(from_epoch_seconds)
}

fn skipped_version() -> Option<Version> {
    settings::read_string(KEY_SKIPPED_VERSION).and_then(|s| Version::parse(&s))
}

impl Updater {
    pub fn new(mtm: MainThreadMarker, enabled: impl Fn() -> bool + 'static) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            mtm,
            me: me.clone(),
            enabled: Box::new(enabled),
            status: RefCell::new(UpdateStatus::Idle),
            launched_at: SystemTime::now(),
            last_attempt: Cell::new(None),
            inbox: RefCell::new(None),
            poll: RefCell::new(None),
            timers: RefCell::new(Vec::new()),
            on_change: RefCell::new(None),
        })
    }

    /// Starts the automatic checks and, after an update, the permission
    /// notice. Nothing touches the network before `FIRST_CHECK_DELAY`.
    pub fn start(&self) {
        if let Some(bundle) = running_bundle() {
            installer::clean_up_after_update(&bundle);
        }
        let me = self.me.clone();
        let first = timer::after(self.mtm, FIRST_CHECK_DELAY, move || {
            if let Some(u) = me.upgrade() {
                u.maybe_check();
            }
        });
        let me = self.me.clone();
        let due = timer::schedule(self.mtm, DUE_POLL_INTERVAL, true, move |_| {
            if let Some(u) = me.upgrade() {
                u.maybe_check();
            }
        });
        let me = self.me.clone();
        let notice = timer::after(self.mtm, PERMISSION_NOTICE_DELAY, move || {
            if let Some(u) = me.upgrade() {
                u.permission_notice();
            }
        });
        self.timers.borrow_mut().extend([first, due, notice]);
    }

    /// Calls `f` whenever the status changes.
    pub fn set_on_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.borrow().clone()
    }

    /// The About page's status line.
    pub fn status_line(&self) -> String {
        let checked = last_success().map(|time| {
            let date = NSDate::dateWithTimeIntervalSince1970(epoch_seconds(time) as f64);
            NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
                &date,
                NSDateFormatterStyle::MediumStyle,
                NSDateFormatterStyle::ShortStyle,
            )
            .to_string()
        });
        status_line(&self.status.borrow(), checked.as_deref())
    }

    fn set_status(&self, status: UpdateStatus) {
        *self.status.borrow_mut() = status;
        if let Some(f) = &*self.on_change.borrow() {
            f();
        }
    }

    fn maybe_check(&self) {
        let due = check_due(
            SystemTime::now(),
            self.launched_at,
            last_success(),
            self.last_attempt.get(),
            (self.enabled)(),
            self.status.borrow().is_busy() || self.inbox.borrow().is_some(),
        );
        if due {
            self.check(false);
        }
    }

    /// "Check for Updates...": checks now, whatever the setting.
    pub fn check_now(&self) {
        self.check(true);
    }

    fn check(&self, manual: bool) {
        if self.status.borrow().is_busy() || self.inbox.borrow().is_some() {
            return;
        }
        self.last_attempt.set(Some(SystemTime::now()));
        self.set_status(UpdateStatus::Checking);
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("ice-rust-update-check".into())
            .spawn(move || {
                let result = installer::check(Version::current());
                let _ = tx.send(Message::Checked { manual, result });
            });
        if let Err(err) = spawned {
            self.set_status(UpdateStatus::CheckFailed(format!(
                "could not start the check ({err})"
            )));
            return;
        }
        self.wait_for(rx);
    }

    fn wait_for(&self, rx: Receiver<Message>) {
        *self.inbox.borrow_mut() = Some(rx);
        let me = self.me.clone();
        let poll = timer::schedule(self.mtm, POLL, true, move |_| {
            if let Some(u) = me.upgrade() {
                u.drain();
            }
        });
        if let Some(old) = self.poll.borrow_mut().replace(poll) {
            old.invalidate();
        }
    }

    fn drain(&self) {
        let received = match self.inbox.borrow().as_ref().map(Receiver::try_recv) {
            Some(Ok(message)) => Some(message),
            Some(Err(TryRecvError::Empty)) => return,
            Some(Err(TryRecvError::Disconnected)) | None => None,
        };
        self.inbox.borrow_mut().take();
        if let Some(poll) = self.poll.borrow_mut().take() {
            poll.invalidate();
        }
        match received {
            Some(Message::Checked { manual, result }) => self.checked(manual, result),
            Some(Message::Installed {
                version,
                bundle,
                result,
            }) => self.installed(version, bundle, result),
            None => self.set_status(UpdateStatus::CheckFailed(
                "the update worker stopped".into(),
            )),
        }
    }

    fn checked(&self, manual: bool, result: Result<CheckOutcome, String>) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.set_status(UpdateStatus::CheckFailed(err.clone()));
                if manual {
                    self.alert("Could not check for updates", &err, &["OK"]);
                }
                return;
            }
        };
        settings::write_number(
            KEY_LAST_UPDATE_CHECK,
            epoch_seconds(SystemTime::now()) as f64,
        );
        match outcome {
            CheckOutcome::UpToDate { .. } => {
                self.set_status(UpdateStatus::UpToDate);
                if manual {
                    let text = format!("You have version {}.", Version::current());
                    self.alert(MSG_UP_TO_DATE, &text, &["OK"]);
                }
            }
            CheckOutcome::NotInstallable { version, reason } => {
                self.set_status(UpdateStatus::NotInstallable(version, reason.clone()));
                if manual || should_offer(version, skipped_version(), false) {
                    let text = format!("ice-rust {version} cannot be installed here: {reason}.");
                    let choice = self.alert(
                        "A new version is available",
                        &text,
                        &["Open Release Page", "Later"],
                    );
                    if choice == 0 {
                        open_url(update::RELEASES_URL);
                    }
                }
            }
            CheckOutcome::Available(update) => {
                self.set_status(UpdateStatus::Available(update.version));
                if should_offer(update.version, skipped_version(), manual) {
                    self.offer(update);
                }
            }
        }
    }

    /// The Install / Later / Skip This Version prompt.
    fn offer(&self, update: Update) {
        let key = installer::trusted_key();
        let bundle = running_bundle();
        let target = precheck(bundle.as_deref(), &key, update.version, installer::writable);
        let title = prompt_title(update.version, Version::current());
        let notes = update.notes.as_str();
        match target {
            Ok(bundle) => {
                let choice = self.alert_with_notes(
                    &title,
                    PERMISSIONS_NOTE,
                    notes,
                    &["Install", "Later", "Skip This Version"],
                );
                match choice {
                    0 => self.install(update, bundle),
                    2 => settings::write_string(KEY_SKIPPED_VERSION, &update.version.to_string()),
                    _ => {}
                }
            }
            Err(err) => {
                let choice = self.alert_with_notes(
                    &title,
                    &err.to_string(),
                    notes,
                    &["Open Release Page", "Later", "Skip This Version"],
                );
                match choice {
                    0 => open_url(&update.page_url),
                    2 => settings::write_string(KEY_SKIPPED_VERSION, &update.version.to_string()),
                    _ => {}
                }
            }
        }
    }

    fn install(&self, update: Update, bundle: PathBuf) {
        self.set_status(UpdateStatus::Downloading(update.version));
        let key = installer::trusted_key();
        let (tx, rx) = mpsc::channel();
        let worker_bundle = bundle.clone();
        let spawned = thread::Builder::new()
            .name("ice-rust-update-install".into())
            .spawn(move || {
                let result = installer::download_and_install(
                    &worker_bundle,
                    &update.zip_url,
                    &update.sig_url,
                    update.version,
                    &key,
                );
                let _ = tx.send(Message::Installed {
                    version: update.version,
                    bundle: worker_bundle,
                    result,
                });
            });
        if let Err(err) = spawned {
            self.set_status(UpdateStatus::InstallFailed(format!(
                "could not start the install ({err})"
            )));
            return;
        }
        self.wait_for(rx);
    }

    fn installed(&self, version: Version, bundle: PathBuf, result: Result<(), String>) {
        if let Err(err) = result {
            self.set_status(UpdateStatus::InstallFailed(err.clone()));
            self.alert("The update failed", &err, &["OK"]);
            return;
        }
        self.set_status(UpdateStatus::Relaunching(version));
        match installer::spawn_relaunch_helper(&bundle) {
            Ok(()) => NSApplication::sharedApplication(self.mtm).terminate(None),
            Err(err) => {
                let text = format!(
                    "ice-rust {version} is installed, but {err}. Quit ice-rust and open it again."
                );
                self.set_status(UpdateStatus::InstallFailed(text.clone()));
                self.alert("Relaunch ice-rust", &text, &["OK"]);
            }
        }
    }

    /// After a version change, says how to grant permissions again if a
    /// grant no longer applies. Shown once per version.
    fn permission_notice(&self) {
        let current = update::CURRENT_VERSION;
        let last = settings::read_string(KEY_LAST_LAUNCHED_VERSION);
        settings::write_string(KEY_LAST_LAUNCHED_VERSION, current);
        let state = permissions::check();
        if let Some(notice) = permission_notice(
            last.as_deref(),
            current,
            state.screen_recording,
            state.can_post_events(),
        ) {
            let choice = self.alert(
                notice.title(),
                notice.text(),
                &["Open Privacy & Security", "Later"],
            );
            if choice == 0 {
                open_url(ACCESSIBILITY_PANE);
            }
        }
    }

    /// Shows a modal alert in front of other apps and returns the index of
    /// the button the user chose.
    fn alert(&self, title: &str, text: &str, buttons: &[&str]) -> usize {
        self.alert_with_notes(title, text, "", buttons)
    }

    /// `alert` with release notes in a fixed-size box that scrolls, so long
    /// notes cannot push the buttons off the screen. Empty notes add no box.
    fn alert_with_notes(&self, title: &str, text: &str, notes: &str, buttons: &[&str]) -> usize {
        let app = NSApplication::sharedApplication(self.mtm);
        app.activate();
        if !app.isActive() {
            // Cooperative activation can refuse an accessory app (L2).
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(text));
        if !notes.is_empty() {
            alert.setAccessoryView(Some(&notes_box(self.mtm, notes)));
        }
        for button in buttons {
            alert.addButtonWithTitle(&NSString::from_str(button));
        }
        let response = alert.runModal();
        usize::try_from(response - NSAlertFirstButtonReturn).unwrap_or(usize::MAX)
    }
}

/// Size of the release notes box in the update prompt, in points.
const NOTES_BOX: NSSize = NSSize::new(420.0, 220.0);

/// A read-only text view of `notes` inside a scroll view of
/// `NOTES_BOX` size.
fn notes_box(mtm: MainThreadMarker, notes: &str) -> Retained<NSScrollView> {
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NOTES_BOX),
    );
    scroll.setHasVerticalScroller(true);
    scroll.setBorderType(NSBorderType::BezelBorder);
    scroll.setDrawsBackground(false);
    let size = scroll.contentSize();
    let text = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), size),
    );
    text.setEditable(false);
    // Not selectable: in the alert on macOS 27, a selectable text view
    // opened scrolled halfway down.
    text.setSelectable(false);
    text.setDrawsBackground(false);
    text.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    text.setTextContainerInset(NSSize::new(4.0, 6.0));
    // Apple's setup for text in a scroll view: wrap at the box's width and
    // grow downward with the text. FLT_MAX as in Apple's sample.
    let unbounded = f64::from(f32::MAX);
    text.setMinSize(NSSize::new(0.0, size.height));
    text.setMaxSize(NSSize::new(unbounded, unbounded));
    text.setVerticallyResizable(true);
    text.setHorizontallyResizable(false);
    text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    // SAFETY: the text view was just created with its own text container.
    let container = unsafe { text.textContainer() };
    if let Some(container) = &container {
        container.setContainerSize(NSSize::new(size.width, unbounded));
        container.setWidthTracksTextView(true);
    }
    text.setString(&NSString::from_str(notes));
    // Lay out all of the text now, so the view has its final height before
    // it is shown. Asking for the layout manager moves the view to TextKit
    // 1, whose layout is complete rather than estimated.
    // SAFETY: as above.
    if let (Some(layout), Some(container)) = (unsafe { text.layoutManager() }, &container) {
        layout.ensureLayoutForTextContainer(container);
    }
    text.sizeToFit();
    scroll.setDocumentView(Some(&text));
    scroll
}
