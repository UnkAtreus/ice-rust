//! The menu bar triggers (v1.5): event monitors for clicks, scrolls and
//! mouse moves, and what they show or hide. `crate::triggers` makes the
//! decisions; this module reads the window list, the frontmost app's menus
//! and the notch, and calls back into the controller.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSTimer;

use super::Controller;
use crate::geometry::appkit_to_cg;
use crate::model::{Rect, STATUS_LAYER, WindowRecord, is_menu_layer};
use crate::platform::{events, menu_bar_space, timer, windows};
use crate::sections::{MenuBarState, Section};
use crate::triggers::{
    HoverCheck, HoverPointer, ICE_BAR_HOVER_PAD, MenuBarSpace, Modifiers, ScrollAction,
    click_section, is_empty_space, scroll_amount,
};

/// How far below a display's top edge a pointer can be in its menu bar;
/// the cheap test before the window list is read.
const MENU_BAR_REACH: f64 = 50.0;
/// How long the menu bar rectangle stays cached for the per-move test.
const BAR_CACHE: Duration = Duration::from_secs(2);
/// How long the end of the app menus stays cached for hover checks. A
/// click always reads it again.
const MENUS_CACHE: Duration = Duration::from_secs(2);
/// How far the Apple menu reaches from the menu bar's left edge; the end of
/// the app menus while ice-rust hides them (measured 44 pt).
const APPLE_MENU_REACH: f64 = 60.0;
/// Wait after a click in empty space before acting, like Ice (50 ms).
const CLICK_ACTION_DELAY: Duration = Duration::from_millis(50);

/// Trigger state owned by the controller.
#[derive(Default)]
pub(super) struct Triggers {
    monitors: RefCell<Vec<Retained<AnyObject>>>,
    hover: RefCell<crate::triggers::Hover>,
    hover_timer: RefCell<Option<Retained<NSTimer>>>,
    scroll: RefCell<crate::triggers::ScrollGesture>,
    /// The active menu bar in CG space and when it was read.
    bar: Cell<Option<(Rect, Instant)>>,
    /// Frontmost process, where its menus end, and when that was read.
    menus: Cell<Option<(i32, f64, Instant)>>,
}

impl Triggers {
    /// Items no longer belong to hovering: leaving the menu bar does not
    /// hide them.
    pub(super) fn release_hover(&self) {
        self.hover.borrow_mut().release();
        if let Some(timer) = self.hover_timer.borrow_mut().take() {
            timer.invalidate();
        }
    }
}

fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        option: flags.contains(NSEventModifierFlags::Option),
        command: flags.contains(NSEventModifierFlags::Command),
    }
}

/// Whether `pointer` is near the top of a display, where a menu bar can be.
pub(super) fn near_menu_bar(pointer: (f64, f64)) -> bool {
    let (x, y) = pointer;
    windows::displays()
        .iter()
        .any(|d| d.bounds.contains(x, y) && y < d.bounds.y + MENU_BAR_REACH)
}

pub(super) fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.min_x() < b.max_x() && b.min_x() < a.max_x() && a.min_y() < b.max_y() && b.min_y() < a.max_y()
}

impl Controller {
    /// Installs the monitors the settings ask for, replacing earlier ones.
    /// Clicks are watched for Show on click and, with Show on hover, to
    /// let a click take hover-shown items over.
    pub(super) fn install_triggers(&self) {
        self.remove_triggers();
        let s = self.settings.borrow().clone();
        let mut monitors = Vec::new();
        if s.show_on_click || s.show_on_hover {
            monitors.extend(self.monitor(
                NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
                Controller::trigger_click,
            ));
        }
        if s.show_on_scroll {
            monitors.extend(self.monitor(NSEventMask::ScrollWheel, Controller::trigger_scroll));
        }
        if s.show_on_hover {
            monitors.extend(self.monitor(NSEventMask::MouseMoved, Controller::trigger_moved));
        } else {
            self.triggers.release_hover();
        }
        *self.triggers.monitors.borrow_mut() = monitors;
    }

    fn remove_triggers(&self) {
        for monitor in self.triggers.monitors.borrow_mut().drain(..) {
            // SAFETY: each monitor came from an add...Monitor call and is
            // removed once.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }

    /// A local and a global monitor for `mask`, like Ice's universal
    /// monitor: the global one sees events sent to other apps, the local
    /// one events sent to ice-rust, such as a click in the menu bar while
    /// Settings is in front.
    pub(super) fn monitor(
        &self,
        mask: NSEventMask,
        handle: fn(&Controller, &NSEvent),
    ) -> Vec<Retained<AnyObject>> {
        let me = self.me.clone();
        let handler = Rc::new(move |event: &NSEvent| {
            if let Some(c) = me.upgrade() {
                handle(&c, event);
            }
        });
        let local_handler = handler.clone();
        let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a live event for the duration of the
            // call.
            local_handler(unsafe { event.as_ref() });
            event.as_ptr()
        });
        let global = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: as above.
            handler(unsafe { event.as_ref() });
        });
        let mut out = Vec::new();
        // SAFETY: the handler returns the event it was given, so AppKit
        // dispatches it unchanged.
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
        {
            out.push(monitor);
        }
        if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global)
        {
            out.push(monitor);
        }
        out
    }

    /// Our own synthetic clicks and drags must not trigger anything.
    pub(super) fn input_busy(&self) -> bool {
        self.mover.is_busy() || self.agent_arrange.is_moving() || events::is_posting()
    }

    /// The active menu bar (the display with our icon) in CG space, read
    /// from the window list and cached for the per-move test.
    fn menu_bar_rect(&self, on_screen: &[WindowRecord], pointer: (f64, f64)) -> Option<Rect> {
        let place = self.menu_bar_place(on_screen, Some(pointer))?;
        let b = place.display.bounds;
        let bar = Rect::new(b.x, b.y, b.width, place.menu_bar_height);
        self.triggers.bar.set(Some((bar, Instant::now())));
        Some(bar)
    }

    /// The cached menu bar for the per-move test, read again once it is
    /// older than `BAR_CACHE` and the pointer is near a display's top.
    fn cached_menu_bar(&self, pointer: (f64, f64)) -> Option<Rect> {
        match self.triggers.bar.get() {
            Some((bar, at)) if at.elapsed() < BAR_CACHE => Some(bar),
            cached if near_menu_bar(pointer) => self
                .menu_bar_rect(&windows::list_on_screen(), pointer)
                .or(cached.map(|(bar, _)| bar)),
            cached => cached.map(|(bar, _)| bar),
        }
    }

    fn pointer_in_menu_bar(&self) -> bool {
        let Some(pointer) = events::pointer_location() else {
            return false;
        };
        near_menu_bar(pointer)
            && self
                .menu_bar_rect(&windows::list_on_screen(), pointer)
                .is_some_and(|bar| bar.contains(pointer.0, pointer.1))
    }

    /// Within `ICE_BAR_HOVER_PAD` of the open Ice Bar.
    fn near_ice_bar(&self, pointer: (f64, f64)) -> bool {
        self.bar.frame().is_some_and(|frame| {
            let f = appkit_to_cg(frame, windows::primary_height());
            Rect::new(
                f.x - ICE_BAR_HOVER_PAD,
                f.y - ICE_BAR_HOVER_PAD,
                f.width + 2.0 * ICE_BAR_HOVER_PAD,
                f.height + 2.0 * ICE_BAR_HOVER_PAD,
            )
            .contains(pointer.0, pointer.1)
        })
    }

    /// Where the frontmost app's menus end on `bar`. `fresh` reads them
    /// again; otherwise a reading younger than `MENUS_CACHE` for the same
    /// app is reused. `None` when they cannot be read (no Accessibility, no
    /// answer in time, ice-rust itself in front for Settings) or lie on
    /// another display. While ice-rust hides the app menus only the Apple
    /// menu is left.
    pub(super) fn app_menus_end(&self, bar: &Rect, fresh: bool) -> Option<f64> {
        let pid = menu_bar_space::frontmost_pid()?;
        if !fresh
            && let Some((cached, end, at)) = self.triggers.menus.get()
            && cached == pid
            && at.elapsed() < MENUS_CACHE
        {
            return Some(end);
        }
        // Asking our own process from its main thread would only time out.
        if pid == std::process::id() as i32 {
            return self
                .app_menus
                .is_hiding()
                .then(|| bar.min_x() + APPLE_MENU_REACH);
        }
        let menu = menu_bar_space::last_menu_frame(pid)?;
        if !overlaps(&menu, bar) {
            return None;
        }
        let end = menu.max_x();
        self.triggers.menus.set(Some((pid, end, Instant::now())));
        Some(end)
    }

    fn menu_bar_space(
        &self,
        on_screen: &[WindowRecord],
        bar: Rect,
        fresh_menus: bool,
    ) -> Option<MenuBarSpace> {
        let app_menus_end = self.app_menus_end(&bar, fresh_menus)?;
        let items = if self.agent_menu_bar {
            self.agent_occupied(&bar)?
        } else {
            on_screen
                .iter()
                .filter(|w| w.layer == STATUS_LAYER && overlaps(&w.bounds, &bar))
                .map(|w| w.bounds)
                .collect()
        };
        let notch = menu_bar_space::notches(self.mtm, windows::primary_height())
            .into_iter()
            .find(|n| overlaps(n, &bar));
        Some(MenuBarSpace {
            bar,
            app_menus_end,
            items,
            notch,
        })
    }

    /// Shows `section` for a trigger: the Ice Bar in Ice Bar mode, the
    /// section in the menu bar otherwise. Returns false when it is already
    /// shown.
    fn trigger_show(&self, section: Section) -> bool {
        if self.bar.is_visible() || self.menu_state().is_shown(section) {
            return false;
        }
        self.triggers.release_hover();
        let use_bar = self.use_ice_bar();
        if use_bar {
            self.open_bar(section);
        } else {
            self.reveal(section);
        }
        true
    }

    fn trigger_hide(&self) {
        self.triggers.release_hover();
        if self.bar.is_visible() {
            self.close_bar();
        } else if self.menu_state() != MenuBarState::AllHidden {
            self.hide();
        }
    }

    /// Show on click: a left click in empty menu bar space acts like a
    /// click on the icon. Any click in the menu bar takes hover-shown items
    /// over, like Ice's latch.
    fn trigger_click(&self, event: &NSEvent) {
        if self.input_busy() {
            return;
        }
        // On macOS 27 a click in the empty-looking menu bar lands on our
        // own expanded divider: the global monitor sees it on MenuBarAgent's
        // window, then MenuBarAgent forwards it to the divider's button and
        // the local monitor sees it again (v1.8).
        if self.agent_menu_bar && self.status.owns_window(event.windowNumber()) {
            return;
        }
        let Some(pointer) = events::pointer_location() else {
            return;
        };
        if !near_menu_bar(pointer) {
            return;
        }
        let on_screen = windows::list_on_screen();
        let Some(bar) = self.menu_bar_rect(&on_screen, pointer) else {
            return;
        };
        if !bar.contains(pointer.0, pointer.1) {
            return;
        }
        self.triggers.release_hover();
        let show_on_click = self.settings.borrow().show_on_click;
        if !show_on_click || event.r#type() != NSEventType::LeftMouseDown {
            return;
        }
        let Some(section) = click_section(modifiers(event.modifierFlags())) else {
            return;
        };
        let Some(space) = self.menu_bar_space(&on_screen, bar, true) else {
            return;
        };
        if !is_empty_space(pointer, &space) {
            return;
        }
        let me = self.me.clone();
        timer::after(self.mtm, CLICK_ACTION_DELAY, move || {
            if let Some(c) = me.upgrade() {
                c.toggle(section);
            }
        });
    }

    /// Show on scroll: down or right over the menu bar shows the hidden
    /// items, up or left hides them, once per gesture.
    fn trigger_scroll(&self, event: &NSEvent) {
        if self.input_busy() {
            return;
        }
        let amount = scroll_amount(
            event.scrollingDeltaX(),
            event.scrollingDeltaY(),
            event.isDirectionInvertedFromDevice(),
            event.hasPreciseScrollingDeltas(),
        );
        let action = self
            .triggers
            .scroll
            .borrow_mut()
            .feed(amount, Instant::now(), || self.pointer_in_menu_bar());
        match action {
            Some(ScrollAction::Show) => {
                self.trigger_show(Section::Hidden);
            }
            Some(ScrollAction::Hide) => self.trigger_hide(),
            None => {}
        }
    }

    /// Show on hover: every mouse move updates the hover state with the
    /// cheap test; a check runs after the delay.
    fn trigger_moved(&self, _event: &NSEvent) {
        if self.input_busy() {
            return;
        }
        let Some(pointer) = events::pointer_location() else {
            return;
        };
        let in_menu_bar = self
            .cached_menu_bar(pointer)
            .is_some_and(|bar| bar.contains(pointer.0, pointer.1));
        let near_ice_bar = self.near_ice_bar(pointer);
        let items_hidden = self.menu_state() == MenuBarState::AllHidden && !self.bar.is_visible();
        let delay = self.settings.borrow().hover_delay;
        let started = self.triggers.hover.borrow_mut().moved(
            HoverPointer {
                in_menu_bar,
                near_ice_bar,
            },
            items_hidden,
            delay,
            Instant::now(),
        );
        if let Some(delay) = started {
            let me = self.me.clone();
            let timer = timer::after(self.mtm, delay, move || {
                if let Some(c) = me.upgrade() {
                    c.hover_due();
                }
            });
            if let Some(old) = self.triggers.hover_timer.borrow_mut().replace(timer) {
                old.invalidate();
            }
        }
    }

    /// A hover check is due: re-test the pointer precisely, then show or
    /// hide. Nothing changes while a menu is open.
    fn hover_due(&self) {
        self.triggers.hover_timer.borrow_mut().take();
        let check = self.triggers.hover.borrow_mut().due(Instant::now());
        let Some(check) = check else {
            return;
        };
        let show_on_hover = self.settings.borrow().show_on_hover;
        if !show_on_hover || self.input_busy() {
            return;
        }
        let Some(pointer) = events::pointer_location() else {
            return;
        };
        let on_screen = windows::list_on_screen();
        if on_screen.iter().any(|w| is_menu_layer(w.layer)) {
            return;
        }
        let bar = self.menu_bar_rect(&on_screen, pointer);
        match check {
            HoverCheck::Show => {
                if self.menu_state() != MenuBarState::AllHidden || self.bar.is_visible() {
                    return;
                }
                let empty = bar
                    .and_then(|bar| self.menu_bar_space(&on_screen, bar, false))
                    .is_some_and(|space| is_empty_space(pointer, &space));
                if empty && self.trigger_show(Section::Hidden) {
                    self.triggers.hover.borrow_mut().showed();
                }
            }
            HoverCheck::Hide => {
                if !self.triggers.hover.borrow().is_shown() {
                    return;
                }
                let in_menu_bar = bar.is_some_and(|bar| bar.contains(pointer.0, pointer.1));
                if !in_menu_bar && !self.near_ice_bar(pointer) {
                    self.trigger_hide();
                }
            }
        }
    }
}
