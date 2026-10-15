//! The menu bar app controller: wires status items, the Ice Bar, the move
//! engine, the Settings window, timers, event monitors and notifications
//! together.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSApplicationDidChangeScreenParametersNotification, NSApplicationWillTerminateNotification,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSImage, NSMenu, NSMenuItem, NSRunningApplication, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
};
use objc2_core_graphics::CGImage;
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObject, NSObjectProtocol,
    NSOperationQueue, NSSize, NSString, NSTimer,
};

use super::brand;
use super::capture::{self, ItemImage};
use super::events::{self, InputError};
use super::ice_bar::{BarItem, ClickHandler, IceBar};
use super::layout_pane::{self, LayoutSnapshot};
use super::menu_icon::{self, CustomIcon};
use super::mover::{MoveCallback, MoveRequest, Mover, MoverHost};
use super::permissions;
use super::settings_window::{Page, SettingsHost, SettingsWindow};
use super::status::{self, StatusItems};
use super::timer;
use super::updater::Updater;
use super::windows;
use crate::bar_layout::{layout, overflow_label, text_tile_size};
use crate::chain::{DividerLookup, find_divider_or_frame, find_titled, section_chains};
use crate::geometry::{self, ControlOrder, Display, MenuBarDisplay, cg_to_appkit, ice_bar_frame};
use crate::menu_icon::IconStyle;
use crate::model::{
    BAR_PADDING, CLICK_SETTLE_POLL, CLICK_SETTLE_TIMEOUT, ControlFrames, ControlNames, REHIDE_TICK,
    Rect, WindowId, WindowRecord, is_menu_layer,
};
use crate::rehide::{Activity, Decision, Rehide};
use crate::sections::{MenuBarState, Section};
use crate::settings::{self, KEY_CUSTOM_ICON_IMAGE, Settings};

/// Ignore a reopen request this soon after a click outside closed the bar,
/// since that click may have been on the icon itself.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
/// Delay between a click in the Ice Bar and acting on it, so the panel is
/// not torn down inside its own event handler.
const CLICK_DEFER: Duration = Duration::from_millis(25);
/// The items are created with every section shown. The first hide waits
/// this long so Control Center has placed their windows before the live
/// order check reads them (M4).
const LAUNCH_HIDE_DELAY: Duration = Duration::from_millis(500);
/// How long after the first activation request the Settings window checks
/// that ice-rust became active before it falls back (L2).
const SETTINGS_ACTIVATE_CHECK: Duration = Duration::from_millis(250);

const MSG_NO_SCREEN_RECORDING: &str = "Screen Recording permission needed";
const MSG_NO_DIVIDER: &str = "Divider not found";
const MSG_NO_ALWAYS_HIDDEN_DIVIDER: &str = "Always-hidden divider not found";
const MSG_NO_HIDDEN_ITEMS: &str = "No hidden items";
const MSG_NO_ALWAYS_HIDDEN_ITEMS: &str = "No always-hidden items";
const MSG_NO_EVENT_ACCESS: &str = "Accessibility permission needed to click items";
const MSG_ICON_LEFT_OF_DIVIDER: &str = "Command-drag the icon right of the dividers";

define_class!(
    // SAFETY: NSObject has no subclassing requirements. No Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<Weak<Controller>>]
    struct Target;

    // SAFETY: every method has the action signature `-(void)x:(id)sender`.
    impl Target {
        #[unsafe(method(iconClicked:))]
        fn icon_clicked(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.icon_clicked();
            }
        }

        #[unsafe(method(toggleSection:))]
        fn toggle_section(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.toggle(Section::Hidden);
            }
        }

        #[unsafe(method(toggleAlwaysHidden:))]
        fn toggle_always_hidden(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.toggle(Section::AlwaysHidden);
            }
        }

        #[unsafe(method(toggleIceBar:))]
        fn toggle_ice_bar(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.toggle_ice_bar_mode();
            }
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.open_settings(None);
            }
        }

        #[unsafe(method(arrangeItems:))]
        fn arrange_items(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.open_settings(Some(Page::Layout));
            }
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.updater.check_now();
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            if let Some(c) = self.controller() {
                c.shutdown();
            }
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(Weak::new()));
        // SAFETY: init is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    fn controller(&self) -> Option<Rc<Controller>> {
        self.ivars().borrow().upgrade()
    }
}

type Observer = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

/// Bridges the move engine and the Settings window to the controller.
struct Host(Weak<Controller>);

impl MoverHost for Host {
    fn menu_bar_state(&self) -> MenuBarState {
        self.0
            .upgrade()
            .map_or(MenuBarState::AllHidden, |c| c.status.state())
    }

    fn set_menu_bar_state(&self, state: MenuBarState) {
        if let Some(c) = self.0.upgrade() {
            c.set_menu_bar_state(state);
        }
    }

    fn active_display(&self, windows: &[WindowRecord]) -> Option<Rect> {
        let c = self.0.upgrade()?;
        c.menu_bar_place(windows, None).map(|p| p.display.bounds)
    }
}

impl SettingsHost for Host {
    fn settings(&self) -> Settings {
        self.0
            .upgrade()
            .map_or_else(Settings::default, |c| c.settings.borrow().clone())
    }

    fn apply_settings(&self, settings: &Settings) {
        if let Some(c) = self.0.upgrade() {
            c.apply_settings(settings);
        }
    }

    fn set_custom_icon(&self, icon: &CustomIcon) {
        if let Some(c) = self.0.upgrade() {
            c.set_custom_icon(icon);
        }
    }

    fn load_layout(&self) -> LayoutSnapshot {
        match self.0.upgrade() {
            Some(c) => c.load_layout(),
            None => LayoutSnapshot::empty("ice-rust is quitting"),
        }
    }

    fn request_move(&self, request: MoveRequest, done: MoveCallback) {
        match self.0.upgrade() {
            Some(c) => c.mover.request(request, done),
            None => done(Err(super::mover::MoveError::NotFound)),
        }
    }

    fn closed(&self) {
        if let Some(c) = self.0.upgrade() {
            c.settings_closed();
        }
    }

    fn check_for_updates(&self) {
        if let Some(c) = self.0.upgrade() {
            c.updater.check_now();
        }
    }

    fn update_status_line(&self) -> String {
        self.0
            .upgrade()
            .map_or_else(String::new, |c| c.updater.status_line())
    }
}

struct Controller {
    mtm: MainThreadMarker,
    me: Weak<Controller>,
    names: ControlNames,
    host: Rc<Host>,
    settings: RefCell<Settings>,
    status: StatusItems,
    bar: IceBar,
    bar_section: Cell<Section>,
    rehide: RefCell<Rehide>,
    mover: Rc<Mover>,
    updater: Rc<Updater>,
    settings_window: RefCell<Option<Rc<SettingsWindow>>>,
    /// The app that was in front when Settings opened; it gets focus back
    /// when the window closes.
    previous_app: RefCell<Option<Retained<NSRunningApplication>>>,
    /// When and for which section a click outside closed the bar.
    bar_closed: Cell<Option<(Instant, Section)>>,
    outside_monitor: RefCell<Option<Retained<AnyObject>>>,
    settle_timer: RefCell<Option<Retained<NSTimer>>>,
    tick_timer: RefCell<Option<Retained<NSTimer>>>,
    observers: RefCell<Vec<Observer>>,
    menu: Retained<NSMenu>,
    toggle_item: Retained<NSMenuItem>,
    always_item: Retained<NSMenuItem>,
    use_bar_item: Retained<NSMenuItem>,
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key: &str,
    target: Option<&Target>,
) -> Retained<NSMenuItem> {
    // SAFETY: `action` names a method that `Target` implements with the
    // `-(void)x:(id)sender` signature, or a standard responder action when
    // no target is given.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    if let Some(target) = target {
        // SAFETY: the target lives for the whole app run (see `run`).
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

/// The main menu, visible only while the Settings window makes the app a
/// regular app: Settings, Quit, and Close Window for Command-W.
fn main_menu(mtm: MainThreadMarker, target: &Target) -> Retained<NSMenu> {
    let main = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&menu_item(
        mtm,
        "Settings...",
        sel!(openSettings:),
        ",",
        Some(target),
    ));
    app_menu.addItem(&menu_item(
        mtm,
        "Check for Updates...",
        sel!(checkForUpdates:),
        "",
        Some(target),
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&menu_item(
        mtm,
        "Quit ice-rust",
        sel!(quit:),
        "q",
        Some(target),
    ));
    app_item.setSubmenu(Some(&app_menu));
    main.addItem(&app_item);

    let window_item = NSMenuItem::new(mtm);
    let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Window"));
    window_menu.addItem(&menu_item(mtm, "Close", sel!(performClose:), "w", None));
    window_item.setSubmenu(Some(&window_menu));
    main.addItem(&window_item);
    main
}

impl Controller {
    fn new(mtm: MainThreadMarker, target: &Target) -> Rc<Self> {
        let settings = settings::load();
        let rehide = Rehide::new(settings.rehide_interval, Instant::now());
        // SAFETY: `target` lives for the whole app run and implements
        // iconClicked: with the required signature.
        let status = unsafe {
            StatusItems::create(
                mtm,
                target,
                sel!(iconClicked:),
                menu_icon::icon_images(settings.icon_style, settings.tint_custom_icon),
            )
        };

        let menu = NSMenu::new(mtm);
        let toggle_item = menu_item(
            mtm,
            "Show Hidden Items",
            sel!(toggleSection:),
            "",
            Some(target),
        );
        let always_item = menu_item(
            mtm,
            "Show Always-Hidden Items",
            sel!(toggleAlwaysHidden:),
            "",
            Some(target),
        );
        let use_bar_item = menu_item(mtm, "Use Ice Bar", sel!(toggleIceBar:), "", Some(target));
        menu.addItem(&toggle_item);
        menu.addItem(&always_item);
        menu.addItem(&use_bar_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(
            mtm,
            "Settings...",
            sel!(openSettings:),
            ",",
            Some(target),
        ));
        menu.addItem(&menu_item(
            mtm,
            "Arrange Menu Bar Items...",
            sel!(arrangeItems:),
            "",
            Some(target),
        ));
        menu.addItem(&menu_item(
            mtm,
            "Check for Updates...",
            sel!(checkForUpdates:),
            "",
            Some(target),
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(
            mtm,
            "Quit ice-rust",
            sel!(quit:),
            "q",
            Some(target),
        ));

        Rc::new_cyclic(|me: &Weak<Controller>| {
            let host = Rc::new(Host(me.clone()));
            let mover = Mover::new(mtm, host.clone());
            let settings_of = me.clone();
            let updater = Updater::new(mtm, move || {
                settings_of
                    .upgrade()
                    .is_some_and(|c| c.settings.borrow().auto_check_updates)
            });
            Self {
                mtm,
                me: me.clone(),
                names: ControlNames::ours(),
                host,
                settings: RefCell::new(settings),
                status,
                bar: IceBar::new(mtm),
                bar_section: Cell::new(Section::Hidden),
                rehide: RefCell::new(rehide),
                mover,
                updater,
                settings_window: RefCell::new(None),
                previous_app: RefCell::new(None),
                bar_closed: Cell::new(None),
                outside_monitor: RefCell::new(None),
                settle_timer: RefCell::new(None),
                tick_timer: RefCell::new(None),
                observers: RefCell::new(Vec::new()),
                menu,
                toggle_item,
                always_item,
                use_bar_item,
            }
        })
    }

    fn start(&self) {
        let me = self.me.clone();
        let timer = timer::schedule(self.mtm, REHIDE_TICK, true, move |_| {
            if let Some(c) = me.upgrade() {
                c.tick();
            }
        });
        *self.tick_timer.borrow_mut() = Some(timer);

        // SAFETY: both are immutable notification name constants.
        let (space_changed, screens_changed) = unsafe {
            (
                NSWorkspaceActiveSpaceDidChangeNotification,
                NSApplicationDidChangeScreenParametersNotification,
            )
        };
        self.observe(
            &NSWorkspace::sharedWorkspace().notificationCenter(),
            space_changed,
        );
        self.observe(&NSNotificationCenter::defaultCenter(), screens_changed);
        self.observe_termination();

        // Keeps the About page's update status current while it is open.
        let me = self.me.clone();
        self.updater.set_on_change(move || {
            if let Some(c) = me.upgrade()
                && let Some(w) = &*c.settings_window.borrow()
            {
                w.refresh_update_status();
            }
        });
        self.updater.start();

        // The items start with every section shown; hiding goes through the
        // order check like every other state change. The placement check
        // runs first, while the always-hidden divider is collapsed and on
        // screen.
        let me = self.me.clone();
        timer::after(self.mtm, LAUNCH_HIDE_DELAY, move || {
            if let Some(c) = me.upgrade() {
                c.check_always_hidden_placement();
                c.hide();
            }
        });
    }

    /// Finishes input in flight before the app exits, on every quit path.
    fn observe_termination(&self) {
        let me = self.me.clone();
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
            if MainThreadMarker::new().is_some()
                && let Some(c) = me.upgrade()
            {
                c.shutdown();
            }
        });
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: the name is an immutable notification name constant. No
        // object filter and no queue, so the block runs synchronously on the
        // posting thread before the process exits. NSApplication posts it
        // from terminate:, on the main thread; the block checks that before
        // it touches the captured Weak<Controller>, which is not Send. The
        // observer is never removed, so the block is never dropped.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationWillTerminateNotification),
                None,
                None,
                &block,
            )
        };
        self.observers.borrow_mut().push((center, token));
    }

    /// Stops the move engine and finishes any input batch in flight, so a
    /// posted mouse-down never outlives the app without its mouse-up.
    fn shutdown(&self) {
        self.mover.cancel();
        events::cancel_active();
    }

    /// Closes the Ice Bar whenever `name` is posted on `center`.
    fn observe(&self, center: &NSNotificationCenter, name: &NSNotificationName) {
        let me = self.me.clone();
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
            if let Some(c) = me.upgrade() {
                c.close_bar();
            }
        });
        let queue = NSOperationQueue::mainQueue();
        // SAFETY: no object filter. The main queue runs the block on the
        // main thread, which the captured Weak<Controller> (not Send) and the
        // AppKit calls in close_bar require, whatever thread posts.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&queue),
                &block,
            )
        };
        self.observers.borrow_mut().push((center.retain(), token));
    }

    fn icon_clicked(&self) {
        let event = NSApplication::sharedApplication(self.mtm).currentEvent();
        let flags = event
            .as_ref()
            .map_or(NSEventModifierFlags::empty(), |e| e.modifierFlags());
        let wants_menu = event
            .as_ref()
            .is_some_and(|e| e.r#type() == NSEventType::RightMouseUp)
            || flags.contains(NSEventModifierFlags::Control);
        if wants_menu {
            self.show_menu();
        } else if flags.contains(NSEventModifierFlags::Option) {
            self.toggle(Section::AlwaysHidden);
        } else {
            self.toggle(Section::Hidden);
        }
    }

    fn show_menu(&self) {
        let use_bar = self.settings.borrow().use_ice_bar;
        self.use_bar_item.setState(if use_bar {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let state = self.menu_state();
        let bar_open = |section| self.bar.is_visible() && self.bar_section.get() == section;
        let hidden_shown = bar_open(Section::Hidden) || state.is_shown(Section::Hidden);
        let always_shown = bar_open(Section::AlwaysHidden) || state.is_shown(Section::AlwaysHidden);
        self.toggle_item
            .setTitle(&NSString::from_str(if hidden_shown {
                "Hide Hidden Items"
            } else {
                "Show Hidden Items"
            }));
        self.always_item
            .setTitle(&NSString::from_str(if always_shown {
                "Hide Always-Hidden Items"
            } else {
                "Show Always-Hidden Items"
            }));
        self.status.show_menu(&self.menu);
    }

    /// Left click toggles the hidden section, Option-click the always-hidden
    /// section (which shows everything). In Ice Bar mode both open the bar
    /// with that section's items instead.
    fn toggle(&self, section: Section) {
        self.rehide.borrow_mut().reset(Instant::now());
        let use_bar = self.settings.borrow().use_ice_bar;
        if self.bar.is_visible() {
            let same = self.bar_section.get() == section;
            self.close_bar();
            if use_bar && !same {
                self.open_bar(section);
            }
            return;
        }
        let state = self.menu_state();
        let shown = state.is_shown(section);
        if shown || !use_bar {
            self.apply_state(state.toggle(section));
            if !shown {
                self.rehide.borrow_mut().reveal(Instant::now());
            }
        } else if let Some((at, closed)) = self.bar_closed.get()
            && at.elapsed() < REOPEN_GUARD
            && closed == section
        {
            self.bar_closed.set(None);
        } else {
            self.open_bar(section);
        }
    }

    fn toggle_ice_bar_mode(&self) {
        let mut next = self.settings.borrow().clone();
        next.use_ice_bar = !next.use_ice_bar;
        self.apply_settings(&next);
    }

    /// Saves new settings and applies them live.
    fn apply_settings(&self, next: &Settings) {
        let previous = self.settings.replace(next.clone());
        settings::save(next);
        self.rehide.borrow_mut().set_interval(next.rehide_interval);
        if previous.use_ice_bar != next.use_ice_bar {
            if next.use_ice_bar {
                self.hide();
            } else {
                self.close_bar();
            }
        }
        if previous.icon_style != next.icon_style
            || previous.tint_custom_icon != next.tint_custom_icon
        {
            self.refresh_icon();
        }
    }

    /// Stores a newly chosen custom image and switches the icon to it, with
    /// the tint its colors suggest.
    fn set_custom_icon(&self, icon: &CustomIcon) {
        settings::write_data(KEY_CUSTOM_ICON_IMAGE, &icon.png);
        let mut next = self.settings.borrow().clone();
        next.icon_style = IconStyle::Custom;
        next.tint_custom_icon = icon.tint;
        self.apply_settings(&next);
        // The style and tint may be unchanged while the image is new.
        self.refresh_icon();
    }

    /// Reloads the icon's images from the settings and the stored custom
    /// image.
    fn refresh_icon(&self) {
        let s = self.settings.borrow().clone();
        self.status
            .set_icon_images(menu_icon::icon_images(s.icon_style, s.tint_custom_icon));
    }

    /// The state the user last asked for: while a move runs, the state the
    /// move engine returns to, otherwise the current one.
    fn menu_state(&self) -> MenuBarState {
        self.mover
            .restore_state()
            .unwrap_or_else(|| self.status.state())
    }

    fn reveal(&self, section: Section) {
        self.apply_state(self.menu_state().show(section));
        self.rehide.borrow_mut().reveal(Instant::now());
    }

    /// A state change the user asked for. While a move runs the move engine
    /// owns the dividers, so the change waits for the move to end.
    fn apply_state(&self, state: MenuBarState) {
        if !self.mover.set_restore_state(state) {
            self.set_menu_bar_state(state);
        }
    }

    /// Hides both sections. Rehide comes through here too.
    fn hide(&self) {
        self.rehide.borrow_mut().hide();
        self.apply_state(MenuBarState::AllHidden);
    }

    /// Sets the divider lengths for `state`. Every state change goes through
    /// here. A state that would expand a divider sitting right of the icon
    /// would push the icon off screen with the section, so everything stays
    /// shown instead until the order is fixed.
    fn set_menu_bar_state(&self, state: MenuBarState) {
        let lengths = state.lengths();
        let mut applied = state;
        if lengths.hidden_expanded || lengths.always_hidden_expanded {
            let order = self.control_order();
            if !order.allows(state) {
                applied = MenuBarState::AllShown;
                self.show_message(MSG_ICON_LEFT_OF_DIVIDER);
            } else if !order.dividers_in_order {
                eprintln!(
                    "ice-rust: the always-hidden divider is right of the hidden divider; \
                     Command-drag the double chevron left of the single chevron"
                );
            }
        }
        self.status.set_state(applied);
        self.status
            .set_icon_active(applied != MenuBarState::AllHidden || self.bar.is_visible());
    }

    /// Live order of the three control items when their titles are
    /// readable, otherwise the stored positions.
    fn control_order(&self) -> ControlOrder {
        let all = windows::list_windows();
        let frames = self.control_frames();
        let icon = find_titled(&all, &self.names.icon)
            .map(|w| w.bounds)
            .or(frames.icon);
        let hidden = self.find_hidden(&all, &frames);
        let always = match self.find_always_hidden(&all, &frames) {
            DividerLookup::Found(d) => Some(d),
            _ => None,
        };
        match (icon, hidden) {
            (Some(icon), DividerLookup::Found(hidden)) => {
                geometry::control_order(&icon, &hidden.bounds, always.as_ref().map(|a| &a.bounds))
            }
            _ => {
                let [icon, hidden, always] = status::stored_positions();
                ControlOrder {
                    hidden_left_of_icon: !settings::positions_inverted(icon, hidden),
                    always_hidden_left_of_icon: !settings::positions_inverted(icon, always),
                    dividers_in_order: !settings::positions_inverted(hidden, always),
                }
            }
        }
    }

    /// Warns when the always-hidden divider is not the leftmost status item
    /// on its display (L1). Items left of it would be always hidden. The
    /// divider is not moved.
    fn check_always_hidden_placement(&self) {
        let all = windows::list_windows();
        let DividerLookup::Found(always) = self.find_always_hidden(&all, &self.control_frames())
        else {
            return;
        };
        let b = always.bounds;
        let Some(display) = windows::displays()
            .into_iter()
            .find(|d| d.bounds.contains(b.mid_x(), b.mid_y()))
        else {
            return;
        };
        if !geometry::always_hidden_leftmost(&all, &always, &display.bounds) {
            eprintln!(
                "ice-rust: warning: the always-hidden divider is not the leftmost status item; \
                 items left of it are always hidden. Command-drag the double chevron to the \
                 far left to change that"
            );
        }
    }

    /// Our three control item windows in CG space, from AppKit. The icon's
    /// frame counts only while it sits at the top of a display.
    fn control_frames(&self) -> ControlFrames {
        let primary_height = windows::primary_height();
        let to_cg = |frame: Option<Rect>| {
            frame
                .filter(|f| !f.is_empty())
                .map(|f| geometry::appkit_to_cg(f, primary_height))
        };
        ControlFrames {
            icon: geometry::icon_rect(
                &windows::displays(),
                None,
                self.status.icon_frame().as_ref(),
                primary_height,
            ),
            hidden: to_cg(self.status.hidden_frame()),
            always_hidden: to_cg(self.status.always_hidden_frame()),
        }
    }

    fn find_hidden(&self, windows: &[WindowRecord], frames: &ControlFrames) -> DividerLookup {
        find_divider_or_frame(
            windows,
            &self.names.hidden,
            &self.names.icon,
            frames.hidden.as_ref(),
        )
    }

    fn find_always_hidden(
        &self,
        windows: &[WindowRecord],
        frames: &ControlFrames,
    ) -> DividerLookup {
        find_divider_or_frame(
            windows,
            &self.names.always_hidden,
            &self.names.icon,
            frames.always_hidden.as_ref(),
        )
    }

    /// Our icon in CG space: its titled window when Screen Recording allows,
    /// otherwise the AppKit frame of its button window.
    fn icon_rect(&self, windows: &[WindowRecord], displays: &[Display]) -> Option<Rect> {
        let cg = find_titled(windows, &self.names.icon).map(|w| w.bounds);
        geometry::icon_rect(
            displays,
            cg.as_ref(),
            self.status.icon_frame().as_ref(),
            windows::primary_height(),
        )
    }

    /// The active menu bar display and our icon's x center. The pointer is
    /// the last resort; this runs after a click on the icon or in the bar.
    fn bar_place(
        &self,
        windows: &[WindowRecord],
        divider: Option<&Rect>,
    ) -> Option<(MenuBarDisplay, Option<f64>)> {
        let displays = windows::displays();
        let icon = self.icon_rect(windows, &displays);
        let pointer = events::pointer_location();
        let place =
            geometry::menu_bar_display(windows, &displays, icon.as_ref(), divider, pointer)?;
        Some((place, icon.map(|r| r.mid_x())))
    }

    /// The items of `section` (hidden or always-hidden), left to right, or
    /// the message to show instead.
    fn section_items(
        &self,
        all: &[WindowRecord],
        section: Section,
    ) -> (
        Option<WindowRecord>,
        Result<Vec<WindowRecord>, &'static str>,
    ) {
        let frames = self.control_frames();
        let hidden = match self.find_hidden(all, &frames) {
            DividerLookup::Found(d) => d,
            DividerLookup::NotFound => return (None, Err(MSG_NO_DIVIDER)),
            DividerLookup::TitlesUnreadable => return (None, Err(MSG_NO_SCREEN_RECORDING)),
        };
        let always = match self.find_always_hidden(all, &frames) {
            DividerLookup::Found(d) => Some(d),
            _ => None,
        };
        let chains = section_chains(all, &hidden, always.as_ref());
        let items = match section {
            Section::AlwaysHidden if always.is_none() => Err(MSG_NO_ALWAYS_HIDDEN_DIVIDER),
            Section::AlwaysHidden if chains.always_hidden.is_empty() => {
                Err(MSG_NO_ALWAYS_HIDDEN_ITEMS)
            }
            Section::AlwaysHidden => Ok(chains.always_hidden),
            _ if chains.hidden.is_empty() => Err(MSG_NO_HIDDEN_ITEMS),
            _ => Ok(chains.hidden),
        };
        (Some(hidden), items)
    }

    fn open_bar(&self, section: Section) {
        let all = windows::list_windows();
        let (divider, items) = self.section_items(&all, section);
        let Some((place, anchor)) = self.bar_place(&all, divider.as_ref().map(|d| &d.bounds))
        else {
            eprintln!("ice-rust: cannot find the active menu bar display");
            return;
        };
        let (items, sizes) = match items {
            Ok(list) => {
                // Bounded (G5): a stuck capture leaves placeholder tiles.
                let result = capture::capture_items_bounded(&list);
                if let Err(err) = &result {
                    eprintln!("ice-rust: capture {err}; showing placeholders");
                }
                bar_items(&list, &capture::images_or_placeholders(&list, result))
            }
            Err(message) => message_item(message),
        };
        self.bar_section.set(section);
        self.present(&place, anchor, &items, &sizes);
    }

    /// Shows `message` in the Ice Bar as a tile that ignores clicks.
    fn show_message(&self, message: &str) {
        eprintln!("ice-rust: {message}");
        let all = windows::list_on_screen();
        let Some((place, anchor)) = self.bar_place(&all, None) else {
            return;
        };
        let (items, sizes) = message_item(message);
        self.present(&place, anchor, &items, &sizes);
    }

    fn present(
        &self,
        place: &MenuBarDisplay,
        anchor: Option<f64>,
        items: &[BarItem],
        sizes: &[(f64, f64)],
    ) {
        let screen = cg_to_appkit(place.display.bounds, windows::primary_height());
        // Sized for the largest count the tile can show.
        let overflow_size = text_tile_size(&overflow_label(sizes.len()));
        let bar_layout = layout(
            sizes,
            place.menu_bar_height,
            BAR_PADDING,
            screen.width,
            overflow_size,
        );
        if bar_layout.dropped > 0 {
            eprintln!(
                "ice-rust: {} of {} items do not fit in the Ice Bar",
                bar_layout.dropped,
                sizes.len()
            );
        }
        let frame = ice_bar_frame(
            &screen,
            place.menu_bar_height,
            anchor,
            bar_layout.width,
            bar_layout.height,
        );

        let me = self.me.clone();
        let on_click: ClickHandler = Rc::new(move |id| {
            if let Some(c) = me.upgrade() {
                c.defer_click_through(id);
            }
        });
        self.bar.show(&frame, &bar_layout, items, &on_click);
        self.status.set_icon_active(true);
        self.install_outside_monitor();
    }

    fn close_bar(&self) {
        if let Some(monitor) = self.outside_monitor.borrow_mut().take() {
            // SAFETY: `monitor` was returned by addGlobalMonitorForEvents and
            // has not been removed yet.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if self.bar.is_visible() {
            self.bar.close();
        }
        self.status.set_icon_active(!self.status.is_hidden());
    }

    /// Clicks in other apps close the bar.
    fn install_outside_monitor(&self) {
        if self.outside_monitor.borrow().is_some() {
            return;
        }
        let me = self.me.clone();
        let block = RcBlock::new(move |_event: NonNull<NSEvent>| {
            if let Some(c) = me.upgrade() {
                c.bar_closed
                    .set(Some((Instant::now(), c.bar_section.get())));
                c.close_bar();
            }
        });
        let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &block,
        );
        *self.outside_monitor.borrow_mut() = monitor;
    }

    fn defer_click_through(&self, id: WindowId) {
        let me = self.me.clone();
        timer::after(self.mtm, CLICK_DEFER, move || {
            if let Some(c) = me.upgrade() {
                c.click_through(id);
            }
        });
    }

    /// Reveals the bar's section, waits for the item to land on screen,
    /// then clicks it. Nothing here blocks: the wait is a timer and the
    /// click restores the cursor per G4. A move in progress owns the mouse,
    /// so the click waits for it to end, and the wait starts over then.
    fn click_through(&self, id: WindowId) {
        let section = self.bar_section.get();
        self.close_bar();
        self.reveal(section);
        if !permissions::check().can_post_events() {
            // CGEventPost would drop the click silently. The section stays
            // shown so the item can be clicked by hand.
            permissions::request_event_access();
            self.show_message(MSG_NO_EVENT_ACCESS);
            return;
        }
        if let Some(old) = self.settle_timer.borrow_mut().take() {
            old.invalidate();
        }
        let deadline = Cell::new(Instant::now() + CLICK_SETTLE_TIMEOUT);
        let me = self.me.clone();
        let timer = timer::schedule(self.mtm, CLICK_SETTLE_POLL, true, move |timer| {
            let Some(c) = me.upgrade() else {
                timer.invalidate();
                return;
            };
            if c.mover.is_busy() || events::is_posting() {
                deadline.set(Instant::now() + CLICK_SETTLE_TIMEOUT);
                return;
            }
            let clicked = c.try_click(id);
            if clicked || Instant::now() >= deadline.get() {
                timer.invalidate();
                c.settle_timer.borrow_mut().take();
                if !clicked {
                    eprintln!(
                        "ice-rust: item {id} did not appear on screen within {CLICK_SETTLE_TIMEOUT:?}"
                    );
                }
            }
        });
        *self.settle_timer.borrow_mut() = Some(timer);
    }

    /// Posts the click once the item window is on screen on the active
    /// display, reading its bounds in the same pass. Returns false while it
    /// is not there yet, or while other input is in flight.
    fn try_click(&self, id: WindowId) -> bool {
        let on_screen = windows::list_on_screen();
        let Some(item) = on_screen.iter().find(|w| w.id == id) else {
            return false;
        };
        let Some(place) = self.menu_bar_place(&on_screen, None) else {
            return false;
        };
        if item.bounds.x < place.display.bounds.min_x() {
            return false;
        }
        match events::click_item(self.mtm, id, &item.bounds) {
            Ok(()) => true,
            Err(InputError::Busy) => false,
            Err(err) => {
                eprintln!("ice-rust: click on item {id} failed: {err}");
                true
            }
        }
    }

    fn menu_bar_place(
        &self,
        windows: &[WindowRecord],
        pointer: Option<(f64, f64)>,
    ) -> Option<MenuBarDisplay> {
        let displays = windows::displays();
        let icon = self.icon_rect(windows, &displays);
        geometry::menu_bar_display(windows, &displays, icon.as_ref(), None, pointer)
    }

    fn tick(&self) {
        // The move engine shows and restores sections on its own, and its
        // drag counts as activity, so the idle count starts after it.
        if self.mover.is_busy() {
            self.rehide.borrow_mut().reset(Instant::now());
            return;
        }
        let bar_visible = self.bar.is_visible();
        if self.status.is_hidden() && !bar_visible {
            return;
        }
        if !self.settings.borrow().auto_rehide {
            return;
        }
        let on_screen = windows::list_on_screen();
        let popup_menu_visible = on_screen.iter().any(|w| is_menu_layer(w.layer));
        // Without our icon's position, the display under the pointer stands
        // in, so the check becomes "in that display's menu bar".
        let pointer = events::pointer_location();
        let pointer_in_menu_bar = match (self.menu_bar_place(&on_screen, pointer), pointer) {
            (Some(place), Some((x, y))) => {
                let b = place.display.bounds;
                Rect::new(b.x, b.y, b.width, place.menu_bar_height).contains(x, y)
            }
            _ => false,
        };
        let activity = Activity {
            pointer_in_menu_bar,
            popup_menu_visible,
            panel_visible: bar_visible,
        };
        if self.rehide.borrow_mut().tick(activity, Instant::now()) == Decision::Rehide {
            self.hide();
        }
    }

    /// Reads the menu bar for the Menu Bar Layout pane.
    fn load_layout(&self) -> LayoutSnapshot {
        layout_pane::load_snapshot(&self.names, &self.control_frames())
    }

    /// Opens the single Settings window, on `page` when given. The app is a
    /// regular app while the window is open.
    fn open_settings(&self, page: Option<Page>) {
        let existing = self.settings_window.borrow().clone();
        let window = existing.unwrap_or_else(|| {
            let host: Rc<dyn SettingsHost> = self.host.clone();
            let created = SettingsWindow::new(self.mtm, host, true);
            *self.settings_window.borrow_mut() = Some(created.clone());
            created
        });
        let app = NSApplication::sharedApplication(self.mtm);
        if !app.isActive() {
            let current = NSRunningApplication::currentApplication();
            *self.previous_app.borrow_mut() = NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .filter(|front| **front != *current);
        }
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        if let Some(page) = page {
            window.select(page);
        }
        // Activating in the same pass as the policy change left the window
        // behind the frontmost app (L2), so it happens on the next pass.
        let me = self.me.clone();
        timer::after(self.mtm, Duration::ZERO, move || {
            if let Some(c) = me.upgrade() {
                c.bring_settings_front(false);
            }
        });
    }

    /// Activates ice-rust and orders the Settings window front. The first
    /// attempt asks politely; when ice-rust is still not active
    /// `SETTINGS_ACTIVATE_CHECK` later, one forced attempt follows.
    fn bring_settings_front(&self, forced: bool) {
        let Some(window) = self.settings_window.borrow().clone() else {
            return;
        };
        if forced && !window.is_visible() {
            return;
        }
        let app = NSApplication::sharedApplication(self.mtm);
        if forced || status::os_major() < 14 {
            // Deprecated since macOS 14, where `activate` may decline while
            // another app is in front; the design allows it as the fallback.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        } else {
            app.activate();
        }
        window.show();
        if forced {
            return;
        }
        let me = self.me.clone();
        timer::after(self.mtm, SETTINGS_ACTIVATE_CHECK, move || {
            if let Some(c) = me.upgrade()
                && !NSApplication::sharedApplication(c.mtm).isActive()
            {
                eprintln!("ice-rust: Settings did not come to the front; activating again");
                c.bring_settings_front(true);
            }
        });
    }

    fn settings_closed(&self) {
        let app = NSApplication::sharedApplication(self.mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        // Switching to accessory keeps ice-rust active with no window; hand
        // focus back to the app that was in front before Settings opened.
        if let Some(previous) = self.previous_app.borrow_mut().take()
            && app.isActive()
            && !previous.isTerminated()
        {
            previous.activateWithOptions(NSApplicationActivationOptions::empty());
        }
        // Drop the window after its delegate callback has returned, with its
        // content detached so the pages are freed now (L7).
        let me = self.me.clone();
        timer::after(self.mtm, Duration::ZERO, move || {
            if let Some(c) = me.upgrade() {
                let window = c.settings_window.borrow_mut().take();
                if let Some(window) = window {
                    window.release_content();
                }
            }
        });
    }
}

/// Bar entries and their sizes in points. Items without a usable image get
/// a placeholder tile the size of their menu bar window.
fn bar_items(list: &[WindowRecord], images: &[ItemImage]) -> (Vec<BarItem>, Vec<(f64, f64)>) {
    let mut items = Vec::with_capacity(list.len());
    let mut sizes = Vec::with_capacity(list.len());
    for (window, captured) in list.iter().zip(images) {
        let title = window.title_str().unwrap_or("Item").to_string();
        let usable = captured.image.as_deref().filter(|_| !captured.transparent);
        let (image, size) = match usable {
            Some(cg) => {
                let w = CGImage::width(Some(cg)) as f64 / captured.scale;
                let h = CGImage::height(Some(cg)) as f64 / captured.scale;
                let image = NSImage::initWithCGImage_size(NSImage::alloc(), cg, NSSize::new(w, h));
                (Some(image), (w, h))
            }
            None => (None, (window.bounds.width, window.bounds.height)),
        };
        items.push(BarItem {
            id: window.id,
            title,
            image,
            clickable: true,
        });
        sizes.push(size);
    }
    (items, sizes)
}

fn message_item(message: &str) -> (Vec<BarItem>, Vec<(f64, f64)>) {
    let item = BarItem {
        id: 0,
        title: message.to_string(),
        image: None,
        clickable: false,
    };
    (vec![item], vec![text_tile_size(message)])
}

/// Runs the menu bar app. Does not return until the app terminates.
pub fn run() {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("ice-rust: the menu bar app must run on the main thread");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if let Some(icon) = brand::app_icon() {
        // SAFETY: a valid NSImage; AppKit copies what it needs for the Dock
        // tile and keeps its own reference.
        unsafe { app.setApplicationIconImage(Some(&icon)) };
    }

    let granted = permissions::check();
    if !granted.screen_recording {
        permissions::request_screen_recording();
    }
    if !granted.can_post_events() {
        permissions::request_event_access();
    }

    let target = Target::new(mtm);
    app.setMainMenu(Some(&main_menu(mtm, &target)));
    let controller = Controller::new(mtm, &target);
    *target.ivars().borrow_mut() = Rc::downgrade(&controller);
    controller.start();

    app.run();

    // Keep both alive for the whole run loop.
    drop(controller);
    drop(target);
}
