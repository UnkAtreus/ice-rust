//! Smart and focused-app rehide (`docs/clean-room/smart-rehide/SPEC.md`;
//! rule and decision IDs refer to it). `crate::rehide_policy` decides; this
//! module watches left clicks and the frontmost app, reads the active
//! Space, the pointer, the window list, the accessibility hit test and the
//! running apps, and runs the hide action.

use std::cell::{Cell, RefCell};
use std::ptr::{self, NonNull};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSEvent, NSEventMask, NSRunningApplication, NSWorkspace,
    NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_core_foundation::{CFRetained, CFType};
use objc2_core_graphics::{CGEvent, CGEventField, CGWindowLevelForKey, CGWindowLevelKey};
use objc2_foundation::{NSNotification, NSOperationQueue};

use super::Controller;
use super::triggers::near_menu_bar;
use crate::geometry;
use crate::input_tag::is_ours;
use crate::model::{FIELD_EVENT_SOURCE_USER_DATA, Rect};
use crate::platform::{events, timer, windows};
use crate::rehide_policy::{
    self, ActivationPolicy, CheckDecision, ClickDecision, ClickTarget, DividerState, FocusChange,
    FocusDecision, HIDE_ORDER, MenuBar, Pid, RunningApp, SmartCheck, SmartClick, SpaceId,
};
use crate::sections::{MenuBarState, Section};

// Private CoreGraphics functions for R7. CoreGraphics exports both (the
// SDK's CoreGraphics.tbd lists them), but no header declares them; these
// are their widely used declarations:
//   CGSConnectionID CGSMainConnectionID(void);
//   CGSSpaceID CGSGetActiveSpace(CGSConnectionID cid);
// CGSConnectionID is a 32-bit integer and CGSSpaceID a 64-bit one.
// SAFETY: the declarations below match those signatures.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSMainConnectionID() -> i32;
    fn CGSGetActiveSpace(connection: i32) -> u64;
}

// HIServices/AXUIElement.h, for the D5 hit test:
//   extern AXUIElementRef AXUIElementCreateSystemWide(void);
//   extern AXError AXUIElementSetMessagingTimeout(AXUIElementRef element,
//       float timeoutInSeconds);
//   extern AXError AXUIElementCopyElementAtPosition(
//       AXUIElementRef application, float x, float y,
//       AXUIElementRef *element);
//   extern AXError AXUIElementGetPid(AXUIElementRef element, pid_t *pid);
// AXUIElementRef is a CFTypeRef and AXError an int32_t.
// SAFETY: the declarations below match that header.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateSystemWide() -> *mut CFType;
    fn AXUIElementSetMessagingTimeout(element: &CFType, timeout: f32) -> i32;
    fn AXUIElementCopyElementAtPosition(
        application: &CFType,
        x: f32,
        y: f32,
        element: *mut *mut CFType,
    ) -> i32;
    fn AXUIElementGetPid(element: &CFType, pid: *mut i32) -> i32;
}

const AX_SUCCESS: i32 = 0;
/// D5: the longest wait for the hit test's answer, so a hung app stalls
/// the main thread for a moment at most.
const HIT_TEST_TIMEOUT: f32 = 0.25;

/// R7, R10: the window server's active Space for ice-rust's main
/// connection.
fn active_space() -> SpaceId {
    // SAFETY: neither function takes a pointer, and the connection is the
    // one the window server opened for this process.
    unsafe { CGSGetActiveSpace(CGSMainConnectionID()) }
}

/// D5 step 1: the process of the app that owns the accessibility element
/// at `pointer` (CG space, the coordinates the hit test takes), from a hit
/// test on the system-wide element. A timeout set on that element applies
/// to the whole process, so it is set for this call only and then reset to
/// the default (0), leaving ice-rust's other Accessibility reads as they
/// were. Any failure (no Accessibility permission, an error or a timeout)
/// is `None`.
fn hit_test_pid(pointer: (f64, f64)) -> Option<Pid> {
    // SAFETY: the Create rule makes us the owner; a null result is handled.
    let system = NonNull::new(unsafe { AXUIElementCreateSystemWide() })
        .map(|system| unsafe { CFRetained::from_raw(system) })?;
    let mut element: *mut CFType = ptr::null_mut();
    // SAFETY: `system` is a live AXUIElement and `element` a valid out
    // pointer, which receives a +1 reference on success.
    let err = unsafe {
        AXUIElementSetMessagingTimeout(&system, HIT_TEST_TIMEOUT);
        let err = AXUIElementCopyElementAtPosition(
            &system,
            pointer.0 as f32,
            pointer.1 as f32,
            &mut element,
        );
        AXUIElementSetMessagingTimeout(&system, 0.0);
        err
    };
    // SAFETY: the Copy rule makes us the owner of the returned reference.
    let element = NonNull::new(element).map(|element| unsafe { CFRetained::from_raw(element) });
    let element = element.filter(|_| err == AX_SUCCESS)?;
    let mut pid = 0;
    // SAFETY: `element` is a live AXUIElement and `pid` a valid out pointer.
    let err = unsafe { AXUIElementGetPid(&element, &mut pid) };
    (err == AX_SUCCESS).then_some(pid)
}

/// D6: whether ice-rust posted `event`. Every event it posts carries a tag
/// in field 42 (`crate::input_tag`).
fn posted_by_app(event: &NSEvent) -> bool {
    event.CGEvent().is_some_and(|cg| {
        let tag =
            CGEvent::integer_value_field(Some(&cg), CGEventField(FIELD_EVENT_SOURCE_USER_DATA));
        is_ours(tag)
    })
}

/// R14 to R16: the lookup from process ID to running app, `None` when no
/// app runs with `pid`. A policy other than regular or accessory counts as
/// prohibited.
fn running_app(pid: Pid) -> Option<RunningApp> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let policy = match app.activationPolicy() {
        p if p == NSApplicationActivationPolicy::Regular => ActivationPolicy::Regular,
        p if p == NSApplicationActivationPolicy::Accessory => ActivationPolicy::Accessory,
        _ => ActivationPolicy::Prohibited,
    };
    Some(RunningApp {
        bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        active: app.isActive(),
        policy,
    })
}

/// The process of the app an activation notification names.
fn activated_pid(note: &NSNotification) -> Option<Pid> {
    let info = note.userInfo()?;
    // SAFETY: an immutable NSString constant exported by AppKit.
    let key = unsafe { NSWorkspaceApplicationKey };
    let app = info
        .objectForKey(key)?
        .downcast::<NSRunningApplication>()
        .ok()?;
    Some(app.processIdentifier())
}

fn divider(shown: bool) -> DividerState {
    if shown {
        DividerState::Show
    } else {
        DividerState::Hide
    }
}

/// ice-rust's state for the divider states the hide action leaves. The
/// visible section has no divider of its own here, so only the other two
/// count.
fn menu_bar_state(menu_bar: &MenuBar) -> MenuBarState {
    if menu_bar.always_hidden == DividerState::Show {
        MenuBarState::AllShown
    } else if menu_bar.hidden == DividerState::Show {
        MenuBarState::HiddenShown
    } else {
        MenuBarState::AllHidden
    }
}

/// Rehide state owned by the controller.
#[derive(Default)]
pub(super) struct RehideWatch {
    /// The left mouse-down monitors, installed while smart rehide is on.
    monitors: RefCell<Vec<Retained<AnyObject>>>,
    /// D3: the last frontmost process other than ice-rust.
    last_other: Cell<Option<Pid>>,
}

impl Controller {
    /// R1: a local and a global monitor for left mouse-downs, so a click in
    /// another app and one in ice-rust's own windows both count. They are
    /// installed only while smart rehide is on (R2), so the Timed and
    /// Focused app strategies pay nothing per click; `smart_click` checks
    /// R2 again. Show on click has its own monitor; the two never act on
    /// the same click, since R6 drops every click in the menu bar.
    pub(super) fn install_smart_rehide(&self) {
        for monitor in self.rehide_watch.monitors.borrow_mut().drain(..) {
            // SAFETY: each monitor came from an add...Monitor call and is
            // removed once.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if !rehide_policy::smart_enabled(self.settings.borrow().rehide()) {
            return;
        }
        let monitors = self.monitor(NSEventMask::LeftMouseDown, Controller::smart_click);
        *self.rehide_watch.monitors.borrow_mut() = monitors;
    }

    /// R2 to R8: reads the click's inputs and starts a check when the
    /// policy asks for one.
    fn smart_click(&self, event: &NSEvent) {
        let number = event.windowNumber();
        let target = if number > 0 && self.status.is_icon_window(number) {
            ClickTarget::AppIcon
        } else if self.bar.owns_window(number) {
            ClickTarget::IceBar
        } else {
            ClickTarget::Other
        };
        let click = SmartClick {
            settings: self.settings.borrow().rehide(),
            target,
            // A batch or move in flight covers posted events whose tag the
            // monitor does not see.
            posted_by_app: posted_by_app(event) || self.input_busy(),
            revealed: self.policy_menu_bar().is_revealed(),
            pointer_in_menu_bar: self.pointer_in_menu_bar_here(),
            space: active_space(),
        };
        let ClickDecision::Check { space, after } = rehide_policy::smart_click(&click) else {
            return;
        };
        // R8: no handle is kept, so nothing cancels or replaces the check.
        // It fires in the common run loop modes, so an open menu or a
        // control tracking the mouse does not hold it back.
        let me = self.me.clone();
        timer::after_common(self.mtm, after, move || {
            if let Some(c) = me.upgrade() {
                c.smart_check(space);
            }
        });
    }

    /// R9 to R17 with D5, after the wait. The settings are not read again
    /// (R9). The window list is read once per check (D7). The hit test
    /// runs here, on the main thread, at the pointer read after the wait;
    /// without a pointer there is nothing to test.
    fn smart_check(&self, snapshot: SpaceId) {
        let windows = windows::list_on_screen_owners();
        let pointer = events::pointer_location();
        let check = SmartCheck {
            snapshot_space: snapshot,
            current_space: active_space(),
            pointer,
            windows: &windows,
            cursor_level: CGWindowLevelForKey(CGWindowLevelKey::CursorWindowLevelKey),
            hit_test: pointer.and_then(hit_test_pid),
        };
        if rehide_policy::smart_check(&check, running_app) == CheckDecision::HideEverything {
            self.run_hide(&HIDE_ORDER);
        }
    }

    /// F1: watches the frontmost app on the main thread. Like a key-value
    /// observation, it also delivers the current frontmost app once now.
    /// The controller calls it after the launch hide, so that delivery
    /// finds nothing left to hide.
    pub(super) fn observe_frontmost(&self) {
        let me = self.me.clone();
        let block = RcBlock::new(move |note: NonNull<NSNotification>| {
            if let Some(c) = me.upgrade() {
                // SAFETY: the notification center passes a live
                // notification for the duration of the call.
                c.frontmost_changed(activated_pid(unsafe { note.as_ref() }));
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
        let current = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier());
        self.frontmost_changed(current);
    }

    /// F2 to F4 with D2 and D3: asks the policy and schedules the hide.
    fn frontmost_changed(&self, frontmost: Option<Pid>) {
        let change = FocusChange {
            settings: self.settings.borrow().rehide(),
            frontmost,
            own: std::process::id() as Pid,
            last_other: self.rehide_watch.last_other.get(),
            pointer_in_menu_bar: self.pointer_in_menu_bar_here(),
        };
        let outcome = rehide_policy::focus_change(&change);
        self.rehide_watch.last_other.set(outcome.last_other);
        if let FocusDecision::Hide { section, after } = outcome.decision {
            let me = self.me.clone();
            timer::after_common(self.mtm, after, move || {
                if let Some(c) = me.upgrade() {
                    c.run_hide(&[section]);
                }
            });
        }
    }

    /// The menu bar as the hide action sees it (Terms). ice-rust's icon
    /// shows as active whenever another section is shown, so the visible
    /// section counts as shown exactly then.
    fn policy_menu_bar(&self) -> MenuBar {
        let state = self.menu_state();
        MenuBar {
            visible: divider(state != MenuBarState::AllHidden),
            hidden: divider(state.is_shown(Section::Hidden)),
            always_hidden: divider(state.is_shown(Section::AlwaysHidden)),
            ice_bar: self.bar.is_visible().then(|| self.bar_section.get()),
            use_ice_bar: self.use_ice_bar(),
        }
    }

    /// Runs the hide action (H1 to H4) on `sections` in order. As with the
    /// Timed checks, nothing hides while a move or the macOS 27 Menu Bar
    /// Layout session holds the sections.
    fn run_hide(&self, sections: &[Section]) {
        if self.arranging() {
            return;
        }
        let outcome = rehide_policy::hide_sections(&self.policy_menu_bar(), sections);
        if !outcome.changed() {
            return;
        }
        if outcome.close_ice_bar {
            self.close_bar();
        }
        match menu_bar_state(&outcome.menu_bar) {
            // `hide` also ends the Timed idle count (H4).
            MenuBarState::AllHidden => self.hide(),
            state => self.apply_state(state),
        }
    }

    /// D4: ice-rust's pointer-in-menu-bar test (the menu bar rectangle that
    /// `geometry::menu_bar_display` gives, with the backdrop window's
    /// height), measured on the display under the pointer rather than the
    /// display with the icon. No display under the pointer means no.
    fn pointer_in_menu_bar_here(&self) -> bool {
        let Some(pointer) = events::pointer_location() else {
            return false;
        };
        if !near_menu_bar(pointer) {
            return false;
        }
        let displays = windows::displays();
        let Some(index) = geometry::display_containing(&displays, pointer.0, pointer.1) else {
            return false;
        };
        let on_screen = windows::list_on_screen();
        let icon = self.icon_rect(&on_screen, &displays);
        // That display is the only choice; the icon gives the height when
        // it has no backdrop window.
        geometry::menu_bar_display(
            &on_screen,
            &displays[index..=index],
            icon.as_ref(),
            None,
            Some(pointer),
        )
        .is_some_and(|place| {
            let b = place.display.bounds;
            Rect::new(b.x, b.y, b.width, place.menu_bar_height).contains(pointer.0, pointer.1)
        })
    }
}
