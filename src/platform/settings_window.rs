//! The Settings window (v1.1): a vibrant sidebar with the app title and
//! one row per page, and pages for General, Menu Bar Layout, Permissions
//! and About. The same window renders offscreen for `render-settings`.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSBitmapImageFileType, NSButton, NSColor,
    NSColorRenderingIntent, NSColorSpace, NSControl, NSControlStateValueOff, NSControlStateValueOn,
    NSEvent, NSImageScaling, NSImageView, NSModalResponse, NSModalResponseOK, NSOpenPanel,
    NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle, NSResponder, NSSlider, NSSwitch,
    NSTextAlignment, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSDictionary, NSObject, NSRect, NSSize, NSString, NSTimer, NSURL};
use objc2_uniform_type_identifiers::UTType;

use super::brand;
use super::hotkeys_pane::HotkeysPane;
use super::layout_pane::{LayoutCanvas, LayoutSnapshot};
use super::login_item;
use super::menu_icon::{self, CustomIcon};
use super::mover::{MoveCallback, MoveError, MoveRequest};
use super::permissions;
use super::timer;
use super::ui::{
    ActionTarget, BoxStyle, FlippedView, Follow, RoundedBox, Weight, WindowDelegate, add, follow,
    label, rect, symbol, symbol_view, wrapping_label,
};
use crate::arrange::{
    BarModel, DropOutcome, apply_drop, contains_all, drop_destination, orders_agree, same_order,
};
use crate::hotkeys::HotkeyAction;
use crate::login_item as login;
use crate::menu_icon::IconStyle;
use crate::model::{LAYOUT_REFRESH, PERMISSIONS_REFRESH, WindowId};
use crate::rehide_policy::{RehideSettings, RehideStrategy, rehide_controls};
use crate::sections::Section;
use crate::settings::{Settings, UI_REHIDE_MAX_SECONDS, UI_REHIDE_MIN_SECONDS, ui_rehide_seconds};
use crate::triggers::MAX_HOVER_DELAY;

/// Default content size of the window, also its minimum (v1.2).
pub const DEFAULT_SIZE: (f64, f64) = (820.0, 600.0);
const SIDEBAR_WIDTH: f64 = 216.0;
/// Height of the title bar area that holds the page title.
const HEADER_HEIGHT: f64 = 44.0;
pub(super) const MARGIN: f64 = 24.0;
/// Height of a settings row on the General and Hidden Items pages.
const ROW: f64 = 56.0;
/// Space between the rehide group and the note under it.
const NOTE_GAP: f64 = 14.0;
/// After the last move, the order is read this often until two reads in a
/// row agree (v1.7): showing and hiding the sections moves the status
/// item windows asynchronously, and a read in the middle shows bars that
/// are half empty.
const SETTLE_POLL: Duration = Duration::from_millis(150);
/// Longest wait for the menu bar to settle after the last move.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the periodic refresh waits to confirm a read that lacks items
/// the bars show, before showing it.
const CONFIRM_MISSING: Duration = Duration::from_secs(1);

/// The Custom image row's note while nothing went wrong.
const CUSTOM_NOTE: &str = "Used by Custom Image";
/// The Automatically rehide row's detail line while the switch is off. With
/// the switch on it describes the selected strategy (SPEC S3).
const AUTO_REHIDE_NOTE: &str = "Hide shown items again on their own";

const SCREEN_RECORDING_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
const ACCESSIBILITY_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// What the window needs from the app.
pub trait SettingsHost {
    fn settings(&self) -> Settings;
    /// Saves and applies new settings at once.
    fn apply_settings(&self, settings: &Settings);
    /// Stores a chosen custom icon image and switches the icon to it.
    fn set_custom_icon(&self, icon: &CustomIcon);
    fn load_layout(&self) -> LayoutSnapshot;
    /// Only the order of the bars, without images; `None` when the
    /// dividers cannot be found.
    fn load_layout_order(&self) -> Option<BarModel>;
    fn request_move(&self, request: MoveRequest, done: MoveCallback);
    /// Actions whose hotkey macOS refused at the last registration (v1.9).
    fn refused_hotkeys(&self) -> Vec<HotkeyAction>;
    /// The window is closing.
    fn closed(&self);
    /// "Check for Updates..." on the About page.
    fn check_for_updates(&self);
    /// The About page's update status line.
    fn update_status_line(&self) -> String;
    /// The menu bar item spacing offset stored now (v1.13).
    fn item_spacing(&self) -> i64;
    /// Applies an item spacing offset after asking; `finished` gets false
    /// when nothing changed, so the controls can be used again.
    fn apply_item_spacing(&self, offset: i64, finished: Box<dyn Fn(bool)>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    HiddenItems,
    Layout,
    Hotkeys,
    Permissions,
    About,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::General,
        Page::HiddenItems,
        Page::Layout,
        Page::Hotkeys,
        Page::Permissions,
        Page::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::HiddenItems => "Hidden Items",
            Page::Layout => "Menu Bar Layout",
            Page::Hotkeys => "Hotkeys",
            Page::Permissions => "Permissions",
            Page::About => "About",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Page::General => "gearshape",
            Page::HiddenItems => "eye",
            Page::Layout => "menubar.rectangle",
            Page::Hotkeys => "keyboard",
            Page::Permissions => "lock.shield",
            Page::About => "info.circle",
        }
    }

    /// The `--page` names of `render-settings`.
    pub fn from_arg(name: &str) -> Option<Page> {
        match name {
            "general" => Some(Page::General),
            "hidden-items" => Some(Page::HiddenItems),
            "layout" => Some(Page::Layout),
            "hotkeys" => Some(Page::Hotkeys),
            "permissions" => Some(Page::Permissions),
            "about" => Some(Page::About),
            _ => None,
        }
    }
}

pub struct RowIvars {
    selected: Cell<bool>,
    /// The window is key: the selection uses the accent color, otherwise
    /// the unemphasized gray like other macOS sidebars.
    emphasized: Cell<bool>,
    icon: Option<Retained<NSImageView>>,
    text: Retained<NSTextField>,
    on_click: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `SidebarRow::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowIvars]
    pub struct SidebarRow;

    // SAFETY: argument and return types match NSView's declarations:
    // isFlipped -> BOOL, acceptsFirstMouse: (NSEvent *) -> BOOL, drawRect:
    // (NSRect) -> void, mouseDown: (NSEvent *) -> void.
    impl SidebarRow {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if self.ivars().selected.get() {
                let fill = if self.ivars().emphasized.get() {
                    NSColor::controlAccentColor()
                } else {
                    NSColor::unemphasizedSelectedContentBackgroundColor()
                };
                fill.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 6.0, 6.0)
                    .fill();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            (self.ivars().on_click)();
        }
    }
);

impl SidebarRow {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        page: Page,
        on_click: Box<dyn Fn()>,
    ) -> Retained<Self> {
        let height = frame.size.height;
        let icon = symbol_view(
            mtm,
            page.symbol(),
            rect(8.0, (height - 20.0) / 2.0, 22.0, 20.0),
            14.0,
            &NSColor::controlAccentColor(),
        );
        let text = label(
            mtm,
            page.title(),
            rect(36.0, (height - 17.0) / 2.0, frame.size.width - 42.0, 17.0),
            13.0,
            Weight::Regular,
            &NSColor::labelColor(),
        );
        let this = Self::alloc(mtm).set_ivars(RowIvars {
            selected: Cell::new(false),
            emphasized: Cell::new(true),
            icon,
            text,
            on_click,
        });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        let row: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        if let Some(icon) = &row.ivars().icon {
            row.addSubview(icon);
        }
        row.addSubview(&row.ivars().text);
        row
    }

    fn set_selected(&self, selected: bool) {
        self.ivars().selected.set(selected);
        self.update_look();
    }

    fn set_emphasized(&self, emphasized: bool) {
        self.ivars().emphasized.set(emphasized);
        self.update_look();
    }

    fn update_look(&self) {
        let ivars = self.ivars();
        let (text, tint) = if ivars.selected.get() && ivars.emphasized.get() {
            (NSColor::whiteColor(), NSColor::whiteColor())
        } else {
            (NSColor::labelColor(), NSColor::controlAccentColor())
        };
        ivars.text.setTextColor(Some(&text));
        if let Some(icon) = &ivars.icon {
            icon.setContentTintColor(Some(&tint));
        }
        self.setNeedsDisplay(true);
    }
}

/// The controls of the General and Hidden Items pages.
struct GeneralParts {
    icon_style: Retained<NSPopUpButton>,
    custom_note: Retained<NSTextField>,
    custom_tint: Retained<NSButton>,
    show_dividers: Retained<NSSwitch>,
    use_bar: Retained<NSSwitch>,
    hide_app_menus: Retained<NSSwitch>,
    show_on_click: Retained<NSSwitch>,
    show_on_hover: Retained<NSSwitch>,
    hover_delay: Retained<NSSlider>,
    hover_delay_value: Retained<NSTextField>,
    show_on_scroll: Retained<NSSwitch>,
    auto_rehide: Retained<NSSwitch>,
    /// The detail line of the Automatically rehide row.
    rehide_note: Retained<NSTextField>,
    /// The Strategy picker (SPEC D9).
    strategy: Retained<NSPopUpButton>,
    /// The group with Automatically rehide and the interval row; it grows
    /// and shrinks with the interval row.
    rehide_group: Retained<RoundedBox>,
    interval_row: Retained<FlippedView>,
    interval: Retained<NSSlider>,
    interval_value: Retained<NSTextField>,
    /// The note under the rehide group.
    hidden_note: Retained<NSTextField>,
    login: Retained<NSSwitch>,
    login_note: Retained<NSTextField>,
    login_button: Retained<NSButton>,
    auto_updates: Retained<NSSwitch>,
}

/// The Menu bar item spacing group (v1.13).
struct SpacingParts {
    slider: Retained<NSSlider>,
    value: Retained<NSTextField>,
    apply: Retained<NSButton>,
    reset: Retained<NSButton>,
    note: Retained<NSTextField>,
}

/// Where the spacing group starts on the General page: below the icon,
/// dividers and system groups.
const SPACING_TOP: f64 = 8.0 + 2.0 * ROW + 16.0 + ROW + 16.0 + 2.0 * ROW + 16.0;
/// The spacing group's note while nothing runs.
const SPACING_NOTE: &str = "Apply saves it, and can reopen the apps with items";

struct PermissionRow {
    granted_icon: Option<Retained<NSImageView>>,
    missing_icon: Option<Retained<NSImageView>>,
    status: Retained<NSTextField>,
    button: Retained<NSButton>,
    requested: Cell<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Permission {
    ScreenRecording,
    Accessibility,
}

pub struct SettingsWindow {
    mtm: MainThreadMarker,
    me: Weak<SettingsWindow>,
    host: Rc<dyn SettingsHost>,
    window: Retained<NSWindow>,
    _delegate: Retained<WindowDelegate>,
    title: Retained<NSTextField>,
    rows: Vec<(Page, Retained<SidebarRow>)>,
    pages: Vec<(Page, Retained<FlippedView>)>,
    current: Cell<Page>,
    general: GeneralParts,
    /// Menu bar item spacing on the General page (v1.13).
    spacing: SpacingParts,
    canvas: Retained<LayoutCanvas>,
    spinner: Retained<NSProgressIndicator>,
    permission_rows: Vec<(Permission, PermissionRow)>,
    hotkeys: Rc<HotkeysPane>,
    about_status: Retained<NSTextField>,
    /// Targets of the controls; controls do not retain their targets.
    _targets: Vec<Retained<ActionTarget>>,
    refresh: RefCell<Option<Retained<NSTimer>>>,
    ticks: Cell<u32>,
    /// The last layout refresh timed out, so the next periodic one is
    /// skipped to let the stuck capture finish.
    skip_refresh: Cell<bool>,
    /// Moves requested from the layout page and not finished yet (v1.7).
    moves_pending: Cell<usize>,
    /// The first failure among the moves since the bars last settled.
    move_error: RefCell<Option<String>>,
    /// The bars as the user dropped them, shown while the moves run.
    expected: RefCell<Option<BarModel>>,
    /// Polls the order until the menu bar settles after the last move.
    settle: RefCell<Option<Retained<NSTimer>>>,
}

/// Hide app menus works with Use Ice Bar off.
fn app_menus_switch_enabled(use_ice_bar: bool) -> bool {
    !use_ice_bar
}

fn switch_state(on: bool) -> isize {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

fn seconds_text(seconds: f64) -> String {
    format!("{} s", seconds.round() as i64)
}

fn tenths_text(seconds: f64) -> String {
    format!("{seconds:.1} s")
}

/// A hover delay slider value rounded to tenths of a second, in range.
fn ui_hover_delay(seconds: f64) -> f64 {
    ((seconds * 10.0).round() / 10.0).clamp(0.0, MAX_HOVER_DELAY.as_secs_f64())
}

fn open_url(url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

impl SettingsWindow {
    /// Builds the window at `DEFAULT_SIZE` without showing it. `live`
    /// starts the refresh timer; offscreen rendering passes false.
    pub fn new(mtm: MainThreadMarker, host: Rc<dyn SettingsHost>, live: bool) -> Rc<Self> {
        let (width, height) = DEFAULT_SIZE;
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: a window releases itself when closed unless told otherwise;
        // releasedWhenClosed is turned off right below, before the window
        // can be closed, and `SettingsWindow` owns it from then on.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, width, height),
                style,
                NSBackingStoreType::Buffered,
                true,
            )
        };
        // SAFETY: the window is owned by `SettingsWindow` and must survive
        // being closed until the app drops it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("ice-rust Settings"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setContentMinSize(NSSize::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1));

        let settings = host.settings();
        let this = Rc::new_cyclic(|me: &Weak<SettingsWindow>| {
            let root = FlippedView::new(mtm, rect(0.0, 0.0, width, height));
            let mut targets = Vec::new();

            // Sidebar: vibrant material under the title bar, app title, rows.
            let sidebar = NSVisualEffectView::initWithFrame(
                NSVisualEffectView::alloc(mtm),
                rect(0.0, 0.0, SIDEBAR_WIDTH, height),
            );
            sidebar.setMaterial(NSVisualEffectMaterial::Sidebar);
            sidebar.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            sidebar.setState(NSVisualEffectState::FollowsWindowActiveState);
            follow(&sidebar, Follow::Height);
            let side = FlippedView::new(mtm, rect(0.0, 0.0, SIDEBAR_WIDTH, height));
            follow(&side, Follow::Height);
            side.addSubview(&label(
                mtm,
                "ice-rust",
                rect(20.0, 40.0, SIDEBAR_WIDTH - 40.0, 34.0),
                26.0,
                Weight::Bold,
                &NSColor::labelColor(),
            ));
            let mut rows = Vec::new();
            for (i, page) in Page::ALL.into_iter().enumerate() {
                let me = me.clone();
                let row = SidebarRow::new(
                    mtm,
                    rect(10.0, 92.0 + 32.0 * i as f64, SIDEBAR_WIDTH - 20.0, 28.0),
                    page,
                    Box::new(move || {
                        if let Some(w) = me.upgrade() {
                            w.select(page);
                        }
                    }),
                );
                side.addSubview(&row);
                rows.push((page, row));
            }
            sidebar.addSubview(&side);
            root.addSubview(&sidebar);

            let content_width = width - SIDEBAR_WIDTH;
            let title = label(
                mtm,
                "",
                rect(
                    SIDEBAR_WIDTH + MARGIN,
                    8.0,
                    content_width - 2.0 * MARGIN,
                    22.0,
                ),
                15.0,
                Weight::Bold,
                &NSColor::labelColor(),
            );
            add(&root, &title, Follow::Width);
            let page_frame = rect(
                SIDEBAR_WIDTH,
                HEADER_HEIGHT,
                content_width,
                height - HEADER_HEIGHT,
            );
            let page_width = content_width - 2.0 * MARGIN;

            // General and Hidden Items.
            let general_view = FlippedView::new(mtm, page_frame);
            let hidden_view = FlippedView::new(mtm, page_frame);
            let general = build_general(
                mtm,
                [&general_view, &hidden_view],
                page_width,
                &settings,
                me,
                &mut targets,
            );
            let spacing = build_spacing_group(
                mtm,
                &general_view,
                SPACING_TOP,
                page_width,
                me,
                &mut targets,
            );

            // Menu Bar Layout.
            let layout_view = FlippedView::new(mtm, page_frame);
            let (canvas, spinner) = build_layout(mtm, &layout_view, page_width, me);

            // Hotkeys.
            let hotkeys_view = FlippedView::new(mtm, page_frame);
            let hotkeys = HotkeysPane::build(mtm, &hotkeys_view, page_width, host.clone());

            // Permissions.
            let permissions_view = FlippedView::new(mtm, page_frame);
            let permission_rows =
                build_permissions(mtm, &permissions_view, page_width, me, &mut targets);

            // About.
            let about_view = FlippedView::new(mtm, page_frame);
            let about_status = build_about(mtm, &about_view, content_width, me, &mut targets);

            let pages = vec![
                (Page::General, general_view),
                (Page::HiddenItems, hidden_view),
                (Page::Layout, layout_view),
                (Page::Hotkeys, hotkeys_view),
                (Page::Permissions, permissions_view),
                (Page::About, about_view),
            ];
            for (_, view) in &pages {
                view.setHidden(true);
                add(&root, view, Follow::Both);
            }
            window.setContentView(Some(&root));

            let closer = me.clone();
            let keyed = me.clone();
            let delegate = WindowDelegate::new(
                mtm,
                move || {
                    if let Some(w) = closer.upgrade() {
                        w.will_close();
                    }
                },
                move |key| {
                    if let Some(w) = keyed.upgrade() {
                        for (_, row) in &w.rows {
                            row.set_emphasized(key);
                        }
                        // A recording ends when the window stops taking keys.
                        if !key {
                            w.hotkeys.stop_recording();
                        }
                    }
                },
            );
            window.setDelegate(Some(delegate.as_protocol()));

            SettingsWindow {
                mtm,
                me: me.clone(),
                host: host.clone(),
                window,
                _delegate: delegate,
                title,
                rows,
                pages,
                current: Cell::new(Page::General),
                general,
                spacing,
                canvas,
                spinner,
                permission_rows,
                hotkeys,
                about_status,
                _targets: targets,
                refresh: RefCell::new(None),
                ticks: Cell::new(0),
                skip_refresh: Cell::new(false),
                moves_pending: Cell::new(0),
                move_error: RefCell::new(None),
                expected: RefCell::new(None),
                settle: RefCell::new(None),
            }
        });
        this.select(Page::General);
        this.refresh_permissions();
        if live {
            this.start_refresh();
        }
        this
    }

    /// Shows `page` and refreshes it.
    pub fn select(&self, page: Page) {
        self.hotkeys.stop_recording();
        self.current.set(page);
        for (p, row) in &self.rows {
            row.set_selected(*p == page);
        }
        for (p, view) in &self.pages {
            view.setHidden(*p != page);
        }
        self.title.setStringValue(&NSString::from_str(page.title()));
        match page {
            // A move in progress reloads when it ends.
            Page::Layout if self.canvas.is_idle() => self.reload_layout(),
            Page::Layout => {}
            Page::Permissions => self.refresh_permissions(),
            Page::General | Page::HiddenItems => self.sync_general(),
            Page::Hotkeys => self.hotkeys.sync(),
            Page::About => self.refresh_update_status(),
        }
    }

    /// Orders the window front, even while ice-rust is not active, and
    /// makes it key.
    pub fn show(&self) {
        if !self.window.isVisible() {
            self.window.center();
        }
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    /// Shows settings changed elsewhere (a hotkey) on the General and
    /// Hidden Items pages.
    pub fn show_settings(&self) {
        self.sync_general();
    }

    /// Shows the stored item spacing, with nothing to apply.
    fn sync_spacing(&self) {
        let current = self.host.item_spacing();
        let s = &self.spacing;
        s.slider.setDoubleValue(current as f64);
        s.value
            .setStringValue(&NSString::from_str(&crate::spacing::offset_label(current)));
        s.slider.setEnabled(true);
        s.apply.setEnabled(false);
        s.reset.setEnabled(current != 0);
        s.note.setStringValue(&NSString::from_str(SPACING_NOTE));
    }

    fn spacing_offset(&self) -> i64 {
        crate::spacing::offset_from_slider(self.spacing.slider.doubleValue())
    }

    fn spacing_changed(&self) {
        let offset = self.spacing_offset();
        self.spacing
            .value
            .setStringValue(&NSString::from_str(&crate::spacing::offset_label(offset)));
        self.spacing
            .apply
            .setEnabled(offset != self.host.item_spacing());
    }

    fn reset_spacing(&self) {
        self.spacing.slider.setDoubleValue(0.0);
        self.spacing_changed();
        self.apply_spacing();
    }

    fn apply_spacing(&self) {
        let offset = self.spacing_offset();
        let s = &self.spacing;
        for control in [&*s.slider as &NSControl, &s.apply, &s.reset] {
            control.setEnabled(false);
        }
        s.note.setStringValue(&NSString::from_str(
            "Finding the apps with menu bar items...",
        ));
        let me = self.me.clone();
        self.host.apply_item_spacing(
            offset,
            Box::new(move |changed| {
                if let Some(w) = me.upgrade() {
                    if changed {
                        w.spacing
                            .note
                            .setStringValue(&NSString::from_str("Applied; ice-rust opens again"));
                    } else {
                        w.sync_spacing();
                    }
                }
            }),
        );
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// Sets the content size the way a live resize does: the views follow
    /// their autoresizing masks and the layout bars lay out again.
    pub fn set_content_size(&self, size: (f64, f64)) {
        self.window.setContentSize(NSSize::new(size.0, size.1));
    }

    fn start_refresh(&self) {
        let me = self.me.clone();
        let timer = timer::schedule(self.mtm, PERMISSIONS_REFRESH, true, move |_| {
            if let Some(w) = me.upgrade() {
                w.tick();
            }
        });
        *self.refresh.borrow_mut() = Some(timer);
    }

    fn tick(&self) {
        if !self.window.isVisible() {
            return;
        }
        match self.current.get() {
            Page::Permissions => self.refresh_permissions(),
            // Approval in System Settings changes the login item's state.
            Page::General => self.sync_login(),
            Page::Layout => {
                let every = (LAYOUT_REFRESH.as_secs_f64() / PERMISSIONS_REFRESH.as_secs_f64())
                    .round()
                    .max(1.0) as u32;
                let ticks = self.ticks.get().wrapping_add(1);
                self.ticks.set(ticks);
                if ticks.is_multiple_of(every) && self.canvas.is_idle() {
                    if self.skip_refresh.replace(false) {
                        return;
                    }
                    self.refresh_layout();
                }
            }
            _ => {}
        }
    }

    fn will_close(&self) {
        self.hotkeys.stop_recording();
        if let Some(timer) = self.refresh.borrow_mut().take() {
            timer.invalidate();
        }
        self.canvas.clear_tool_tips();
        self.host.closed();
    }

    /// Detaches the views and the delegate from the closed window. AppKit
    /// keeps a closed window alive as the app's previous key window and in
    /// the current autorelease pool until a later event, and the window's
    /// content kept every page, the layout snapshot and its images alive
    /// with it (L7). After this only an empty window remains.
    pub fn release_content(&self) {
        self.window.setDelegate(None);
        self.window.setContentView(None);
    }

    fn sync_general(&self) {
        let s = self.host.settings();
        self.general.use_bar.setState(switch_state(s.use_ice_bar));
        self.general
            .auto_rehide
            .setState(switch_state(s.auto_rehide));
        self.general
            .show_dividers
            .setState(switch_state(s.show_section_dividers));
        let index = RehideStrategy::ALL
            .iter()
            .position(|&strategy| strategy == s.rehide_strategy)
            .unwrap_or(0);
        self.general.strategy.selectItemAtIndex(index as isize);
        let seconds = ui_rehide_seconds(s.rehide_interval);
        self.general.interval.setDoubleValue(seconds);
        self.general
            .interval_value
            .setStringValue(&NSString::from_str(&seconds_text(seconds)));
        self.layout_rehide(s.rehide());
        self.general
            .hide_app_menus
            .setState(switch_state(s.hide_app_menus));
        self.general
            .hide_app_menus
            .setEnabled(app_menus_switch_enabled(s.use_ice_bar));
        self.general
            .show_on_click
            .setState(switch_state(s.show_on_click));
        self.general
            .show_on_hover
            .setState(switch_state(s.show_on_hover));
        self.general
            .show_on_scroll
            .setState(switch_state(s.show_on_scroll));
        let delay = s.hover_delay.as_secs_f64();
        self.general.hover_delay.setDoubleValue(delay);
        self.general
            .hover_delay_value
            .setStringValue(&NSString::from_str(&tenths_text(delay)));
        self.general.hover_delay.setEnabled(s.show_on_hover);
        self.general
            .auto_updates
            .setState(switch_state(s.auto_check_updates));
        self.sync_icon();
        self.sync_login();
        self.sync_spacing();
    }

    /// Shows the stored icon style and tint, and the custom image in the
    /// popup's Custom Image item.
    fn sync_icon(&self) {
        let s = self.host.settings();
        let index = IconStyle::ALL
            .iter()
            .position(|&style| style == s.icon_style)
            .unwrap_or(0);
        self.general.icon_style.selectItemAtIndex(index as isize);
        self.set_custom_preview(s.tint_custom_icon);
        self.general
            .custom_tint
            .setState(switch_state(s.tint_custom_icon));
        self.general
            .custom_tint
            .setEnabled(s.icon_style == IconStyle::Custom);
        self.set_custom_note(CUSTOM_NOTE, false);
    }

    fn set_custom_preview(&self, tint: bool) {
        let index = IconStyle::ALL.len() - 1;
        if let Some(item) = self.general.icon_style.itemAtIndex(index as isize) {
            item.setImage(menu_icon::stored_custom(tint).as_deref());
        }
    }

    fn set_custom_note(&self, text: &str, error: bool) {
        let note = &self.general.custom_note;
        note.setStringValue(&NSString::from_str(text));
        note.setToolTip(error.then(|| NSString::from_str(text)).as_deref());
        let color = if error {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        note.setTextColor(Some(&color));
    }

    fn selected_strategy(&self) -> RehideStrategy {
        usize::try_from(self.general.strategy.indexOfSelectedItem())
            .ok()
            .and_then(|i| RehideStrategy::ALL.get(i).copied())
            .unwrap_or_default()
    }

    /// Shows the rehide controls SPEC S3 asks for: the Strategy picker and
    /// its description while the switch is on, the interval row only for
    /// Timed. The group and the note under it follow the row count.
    fn layout_rehide(&self, settings: RehideSettings) {
        let controls = rehide_controls(settings);
        let g = &self.general;
        g.strategy.setHidden(!controls.strategy_picker);
        let note = if controls.strategy_picker {
            settings.strategy.description()
        } else {
            AUTO_REHIDE_NOTE
        };
        g.rehide_note.setStringValue(&NSString::from_str(note));
        g.interval_row.setHidden(!controls.interval);
        let mut group = g.rehide_group.frame();
        group.size.height = if controls.interval { 2.0 * ROW } else { ROW };
        g.rehide_group.setFrame(group);
        let mut below = g.hidden_note.frame();
        below.origin.y = group.origin.y + group.size.height + NOTE_GAP;
        g.hidden_note.setFrame(below);
    }

    fn selected_style(&self) -> IconStyle {
        usize::try_from(self.general.icon_style.indexOfSelectedItem())
            .ok()
            .and_then(|i| IconStyle::ALL.get(i).copied())
            .unwrap_or_default()
    }

    /// The icon popup changed. Custom Image without a stored image asks
    /// for one first.
    fn icon_style_changed(&self) {
        if self.selected_style() == IconStyle::Custom && menu_icon::stored_custom(true).is_none() {
            self.choose_custom_icon();
        } else {
            self.general_changed(false);
        }
    }

    /// Asks for an image file in a sheet on the window.
    fn choose_custom_icon(&self) {
        let panel = NSOpenPanel::openPanel(self.mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        if let Some(image) = UTType::typeWithIdentifier(&NSString::from_str("public.image")) {
            panel.setAllowedContentTypes(&NSArray::from_retained_slice(&[image]));
        }
        panel.setMessage(Some(&NSString::from_str(
            "Choose an image for the menu bar icon",
        )));
        panel.setPrompt(Some(&NSString::from_str("Choose")));
        let me = self.me.clone();
        // The handler keeps the panel to read its URL; AppKit releases the
        // handler once the sheet ends.
        let chosen = panel.clone();
        let handler = RcBlock::new(move |response: NSModalResponse| {
            let path = (response == NSModalResponseOK)
                .then(|| chosen.URL())
                .flatten()
                .and_then(|url| url.path())
                .map(|path| path.to_string());
            if let Some(w) = me.upgrade() {
                w.custom_icon_chosen(path);
            }
        });
        panel.beginSheetModalForWindow_completionHandler(&self.window, &handler);
    }

    /// The open panel ended. A cancelled panel or an image that fails to
    /// load puts the popup back on the stored style.
    fn custom_icon_chosen(&self, path: Option<String>) {
        let Some(path) = path else {
            self.sync_icon();
            return;
        };
        let path = Path::new(&path);
        match menu_icon::prepare_custom(path) {
            Ok(icon) => {
                self.host.set_custom_icon(&icon);
                self.sync_icon();
            }
            Err(err) => {
                self.sync_icon();
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into(),
                );
                eprintln!("ice-rust: custom icon {}: {err}", path.display());
                self.set_custom_note(&format!("{name}: {err}"), true);
            }
        }
    }

    /// Shows the login item's state: switch, note and approval button.
    fn sync_login(&self) {
        let state = login_item::state();
        self.general
            .login
            .setState(switch_state(login::switch_on(state)));
        self.general.login.setEnabled(login::switch_enabled(state));
        self.general
            .login_note
            .setStringValue(&NSString::from_str(login::note(state)));
        self.general
            .login_note
            .setTextColor(Some(&NSColor::secondaryLabelColor()));
        self.general
            .login_button
            .setHidden(!login::needs_approval_button(state));
    }

    /// The Launch at login switch moved: registers or unregisters, then
    /// shows the state macOS reports, or the error.
    fn login_changed(&self) {
        let on = self.general.login.state() == NSControlStateValueOn;
        let result = login_item::set_enabled(on);
        self.sync_login();
        if let Err(err) = result {
            eprintln!("ice-rust: launch at login: {err}");
            self.general
                .login_note
                .setStringValue(&NSString::from_str(&err));
            self.general
                .login_note
                .setTextColor(Some(&NSColor::systemRedColor()));
        }
    }

    /// Shows the updater's status on the About page.
    pub fn refresh_update_status(&self) {
        self.about_status
            .setStringValue(&NSString::from_str(&self.host.update_status_line()));
    }

    /// Reads the controls into settings, saves and applies them. The
    /// interval is read only when its slider moved, so a stored value
    /// outside the slider's range survives changes to the switches. The
    /// hover delay slider covers every accepted value.
    fn general_changed(&self, slider_moved: bool) {
        let mut s = self.host.settings();
        s.use_ice_bar = self.general.use_bar.state() == NSControlStateValueOn;
        s.hide_app_menus = self.general.hide_app_menus.state() == NSControlStateValueOn;
        self.general
            .hide_app_menus
            .setEnabled(app_menus_switch_enabled(s.use_ice_bar));
        s.show_on_click = self.general.show_on_click.state() == NSControlStateValueOn;
        s.show_on_hover = self.general.show_on_hover.state() == NSControlStateValueOn;
        s.show_on_scroll = self.general.show_on_scroll.state() == NSControlStateValueOn;
        let delay = ui_hover_delay(self.general.hover_delay.doubleValue());
        s.hover_delay = Duration::from_secs_f64(delay);
        self.general
            .hover_delay_value
            .setStringValue(&NSString::from_str(&tenths_text(delay)));
        self.general.hover_delay.setEnabled(s.show_on_hover);
        s.auto_rehide = self.general.auto_rehide.state() == NSControlStateValueOn;
        s.show_section_dividers = self.general.show_dividers.state() == NSControlStateValueOn;
        s.rehide_strategy = self.selected_strategy();
        s.auto_check_updates = self.general.auto_updates.state() == NSControlStateValueOn;
        s.icon_style = self.selected_style();
        s.tint_custom_icon = self.general.custom_tint.state() == NSControlStateValueOn;
        let seconds = if slider_moved {
            let seconds = self
                .general
                .interval
                .doubleValue()
                .round()
                .clamp(UI_REHIDE_MIN_SECONDS, UI_REHIDE_MAX_SECONDS);
            s.rehide_interval = Duration::from_secs_f64(seconds);
            seconds
        } else {
            ui_rehide_seconds(s.rehide_interval)
        };
        self.general
            .interval_value
            .setStringValue(&NSString::from_str(&seconds_text(seconds)));
        self.layout_rehide(s.rehide());
        self.general
            .custom_tint
            .setEnabled(s.icon_style == IconStyle::Custom);
        self.set_custom_preview(s.tint_custom_icon);
        self.host.apply_settings(&s);
    }

    fn refresh_permissions(&self) {
        let state = permissions::check();
        for (kind, row) in &self.permission_rows {
            let granted = match kind {
                Permission::ScreenRecording => state.screen_recording,
                Permission::Accessibility => state.can_post_events(),
            };
            if let Some(icon) = &row.granted_icon {
                icon.setHidden(!granted);
            }
            if let Some(icon) = &row.missing_icon {
                icon.setHidden(granted);
            }
            row.status.setStringValue(&NSString::from_str(if granted {
                "Granted"
            } else {
                "Not granted"
            }));
            let color = if granted {
                NSColor::systemGreenColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            row.status.setTextColor(Some(&color));
            row.button.setHidden(granted);
            let title = if row.requested.get() {
                "Open System Settings"
            } else {
                "Grant Access"
            };
            row.button.setTitle(&NSString::from_str(title));
        }
    }

    fn permission_clicked(&self, kind: Permission) {
        let Some((_, row)) = self.permission_rows.iter().find(|(k, _)| *k == kind) else {
            return;
        };
        if row.requested.get() {
            open_url(match kind {
                Permission::ScreenRecording => SCREEN_RECORDING_PANE,
                Permission::Accessibility => ACCESSIBILITY_PANE,
            });
        } else {
            row.requested.set(true);
            match kind {
                Permission::ScreenRecording => {
                    permissions::request_screen_recording();
                }
                Permission::Accessibility => {
                    permissions::request_event_access();
                }
            }
        }
        self.refresh_permissions();
    }

    /// Rebuilds the bars from the real menu bar.
    pub fn reload_layout(&self) {
        let snapshot = self.host.load_layout();
        self.show_snapshot(snapshot);
    }

    /// The periodic refresh. An order that differs from the bars shown is
    /// read again a moment later and shown only if it holds, so a section
    /// caught while it shows or hides never flashes empty (v1.7).
    fn refresh_layout(&self) {
        let Some(read) = self.host.load_layout_order() else {
            // No dividers: the full reload shows why.
            self.reload_layout();
            return;
        };
        let shown = self.canvas.with_snapshot(|s| s.model.clone());
        if shown.as_ref().is_some_and(|shown| same_order(shown, &read)) {
            self.reload_layout();
            return;
        }
        // Items missing from the read are often still on their way after
        // the sections showed or hid, so that read must hold for longer.
        let missing = shown.is_some_and(|shown| !contains_all(&read, &shown));
        let confirm = if missing {
            CONFIRM_MISSING
        } else {
            SETTLE_POLL
        };
        let me = self.me.clone();
        timer::after(self.mtm, confirm, move || {
            let Some(w) = me.upgrade() else {
                return;
            };
            if !w.canvas.is_idle() || w.moves_pending.get() > 0 {
                return;
            }
            let again = w.host.load_layout_order();
            if again.is_some_and(|again| same_order(&again, &read)) {
                w.reload_layout();
            }
        });
    }

    /// Shows a fresh snapshot. Images that timed out keep the earlier ones
    /// for the same windows. Only the note changes; a message from the last
    /// drop stays.
    fn show_snapshot(&self, mut snapshot: LayoutSnapshot) {
        self.canvas.with_snapshot(|old| snapshot.reuse_stale(old));
        let timed_out = snapshot.timed_out();
        self.skip_refresh.set(timed_out);
        self.canvas.set_snapshot(snapshot);
        self.canvas.set_note(timed_out.then(|| {
            "Some item images timed out; earlier images or placeholders shown".to_string()
        }));
    }

    fn set_busy(&self, busy: bool) {
        self.canvas.set_busy(busy);
        self.spinner.setHidden(!busy);
        // SAFETY: a nil sender is allowed.
        unsafe {
            if busy {
                self.spinner.startAnimation(None);
            } else {
                self.spinner.stopAnimation(None);
            }
        }
    }

    /// A tile was dropped. The bars show the new order at once and the
    /// real item moves in the background; further drops queue behind it,
    /// like Ice's layout editor (v1.7).
    fn dropped(&self, item: WindowId, section: Section, index: usize) {
        let decision = self.canvas.with_snapshot(|s| {
            s.model.as_ref().map(|m| {
                (
                    drop_destination(m, item, section, index),
                    m.find(item).map(|(from, _)| from),
                    apply_drop(m, item, section, index),
                )
            })
        });
        match decision {
            None | Some((Ok(DropOutcome::NoChange), _, _)) => {}
            Some((Err(err), _, _)) => self.canvas.set_message(Some(err.to_string()), true),
            Some((Ok(DropOutcome::Move(destination)), Some(from), Some(next))) => {
                if let Some(settle) = self.settle.borrow_mut().take() {
                    settle.invalidate();
                }
                if self.moves_pending.get() == 0 {
                    self.move_error.borrow_mut().take();
                }
                self.moves_pending.set(self.moves_pending.get() + 1);
                self.canvas.show_model(next.clone());
                *self.expected.borrow_mut() = Some(next);
                self.canvas.set_message(None, false);
                self.set_busy(true);
                let request = MoveRequest {
                    item,
                    destination,
                    sections: [from, section],
                };
                let me = self.me.clone();
                self.host.request_move(
                    request,
                    Box::new(move |result| {
                        if let Some(w) = me.upgrade() {
                            w.move_finished(result);
                        }
                    }),
                );
            }
            Some(_) => {}
        }
    }

    fn move_finished(&self, result: Result<(), MoveError>) {
        let left = self.moves_pending.get().saturating_sub(1);
        self.moves_pending.set(left);
        if let Err(err) = result {
            self.move_error
                .borrow_mut()
                .get_or_insert_with(|| format!("Move failed: {err}"));
        }
        if left == 0 {
            self.settle_after_moves();
        }
    }

    /// Reads the order every `SETTLE_POLL` until two reads in a row agree
    /// or `SETTLE_TIMEOUT` passes, then shows the real bars once.
    fn settle_after_moves(&self) {
        if let Some(old) = self.settle.borrow_mut().take() {
            old.invalidate();
        }
        let started = Instant::now();
        let last: RefCell<Option<BarModel>> = RefCell::new(None);
        let reads = Cell::new(0u32);
        let me = self.me.clone();
        let timer = timer::schedule(self.mtm, SETTLE_POLL, true, move |timer| {
            let Some(w) = me.upgrade() else {
                timer.invalidate();
                return;
            };
            // A newer drop restarts settling when it finishes.
            if w.moves_pending.get() > 0 {
                timer.invalidate();
                return;
            }
            let read = w.host.load_layout_order();
            reads.set(reads.get() + 1);
            let stable = matches!(
                (&read, &*last.borrow()),
                (Some(a), Some(b)) if same_order(a, b)
            );
            // Two equal reads can both miss items still on their way, so a
            // settled read also has every item the bars show.
            let complete = read.as_ref().is_some_and(|read| {
                w.expected
                    .borrow()
                    .as_ref()
                    .is_none_or(|expected| contains_all(read, expected))
            });
            *last.borrow_mut() = read;
            let timed_out = started.elapsed() >= SETTLE_TIMEOUT;
            let settled = stable && complete;
            if !settled && !timed_out {
                return;
            }
            timer.invalidate();
            w.settle.borrow_mut().take();
            eprintln!(
                "ice-rust: layout {} after {} reads ({} ms)",
                match (settled, complete) {
                    (true, _) => "settled",
                    (false, true) => "still changing, shown as read",
                    (false, false) => "incomplete, kept as dropped",
                },
                reads.get(),
                started.elapsed().as_millis(),
            );
            if complete {
                w.finish_moves();
            } else {
                w.keep_dropped();
            }
        });
        *self.settle.borrow_mut() = Some(timer);
    }

    /// The menu bar did not settle in time with every item: the bars keep
    /// the order as dropped, which the mover verified move by move, and
    /// the periodic refresh catches up once the menu bar has settled.
    fn keep_dropped(&self) {
        self.expected.borrow_mut().take();
        self.set_busy(false);
        let message = self.move_error.borrow_mut().take();
        let error = message.is_some();
        self.canvas.set_message(message, error);
    }

    /// Shows the settled menu bar and says whether it ended up as dropped.
    fn finish_moves(&self) {
        let snapshot = self.host.load_layout();
        let expected = self.expected.borrow_mut().take();
        let agrees = match (&snapshot.model, &expected) {
            (Some(actual), Some(expected)) => orders_agree(actual, expected),
            _ => false,
        };
        self.show_snapshot(snapshot);
        self.set_busy(false);
        let message = match self.move_error.borrow_mut().take() {
            Some(err) => Some(err),
            None if !agrees => Some(
                "The menu bar did not end up in the order you dropped; the bars show it as it is"
                    .to_string(),
            ),
            None => None,
        };
        let error = message.is_some();
        self.canvas.set_message(message, error);
    }

    /// Draws the window content, title bar included, into a PNG without
    /// showing the window. Returns the pixel size.
    pub fn render_png(&self, path: &Path) -> Result<(isize, isize), String> {
        let content = self
            .window
            .contentView()
            .ok_or("the window has no content view")?;
        // SAFETY: the frame view is retained by the window, which outlives
        // this call.
        let view = unsafe { content.superview() }.unwrap_or(content);
        view.layoutSubtreeIfNeeded();
        let bounds = view.bounds();
        let rep = view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .ok_or("could not create a bitmap for the window")?;
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        // The cached bitmap is in the display's color space, and its ICC
        // profile names the display model. Writing sRGB keeps that out of
        // the PNG.
        let rep = rep
            .bitmapImageRepByConvertingToColorSpace_renderingIntent(
                &NSColorSpace::sRGBColorSpace(),
                NSColorRenderingIntent::Default,
            )
            .ok_or("could not convert the bitmap to sRGB")?;
        let properties = NSDictionary::new();
        // SAFETY: an empty properties dictionary is a valid argument.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
        }
        .ok_or("PNG encoding failed")?;
        std::fs::write(path, data.to_vec()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((rep.pixelsWide(), rep.pixelsHigh()))
    }

    pub fn canvas(&self) -> &LayoutCanvas {
        &self.canvas
    }
}

/// A row title and a secondary line under it.
pub(super) fn row_text(
    mtm: MainThreadMarker,
    parent: &NSView,
    title: &str,
    detail: &str,
    x: f64,
    y: f64,
    width: f64,
) {
    add(
        parent,
        &label(
            mtm,
            title,
            rect(x, y, width, 17.0),
            13.0,
            Weight::Medium,
            &NSColor::labelColor(),
        ),
        Follow::Width,
    );
    add(
        parent,
        &label(
            mtm,
            detail,
            rect(x, y + 19.0, width, 15.0),
            11.0,
            Weight::Regular,
            &NSColor::secondaryLabelColor(),
        ),
        Follow::Width,
    );
}

pub(super) fn separator(mtm: MainThreadMarker, parent: &NSView, y: f64, width: f64) {
    let line = RoundedBox::new(mtm, rect(16.0, y, width - 32.0, 1.0), BoxStyle::Separator);
    add(parent, &line, Follow::Width);
}

/// A title, a detail line and a switch at the right, in `group` at `y`.
fn switch_row(
    mtm: MainThreadMarker,
    group: &NSView,
    (title, detail): (&str, &str),
    y: f64,
    width: f64,
    on: bool,
) -> Retained<NSSwitch> {
    row_text(mtm, group, title, detail, 16.0, y + 11.0, width - 100.0);
    let switch = NSSwitch::initWithFrame(
        NSSwitch::alloc(mtm),
        rect(width - 58.0, y + 16.0, 42.0, 24.0),
    );
    switch.setState(switch_state(on));
    add(group, &switch, Follow::Right);
    switch
}

/// A title, a detail line, a slider and its value at the right, in
/// `group` at `y`.
fn slider_row(
    mtm: MainThreadMarker,
    group: &NSView,
    (title, detail): (&str, &str),
    y: f64,
    width: f64,
    (value, min, max): (f64, f64, f64),
    text: &str,
) -> (Retained<NSSlider>, Retained<NSTextField>) {
    row_text(mtm, group, title, detail, 16.0, y + 11.0, width - 240.0);
    // SAFETY: no target or action yet; the caller attaches both.
    let slider = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(value, min, max, None, None, mtm)
    };
    slider.setFrame(rect(width - 216.0, y + 18.0, 150.0, 20.0));
    slider.setContinuous(true);
    add(group, &slider, Follow::Right);
    let value_label = label(
        mtm,
        text,
        rect(width - 58.0, y + 19.0, 44.0, 17.0),
        13.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    value_label.setAlignment(NSTextAlignment::Right);
    add(group, &value_label, Follow::Right);
    (slider, value_label)
}

/// Builds the General page (the menu bar icon, launch at login, updates)
/// and the Hidden Items page (Ice Bar, show on click, hover and scroll,
/// rehide) into `general_page` and `hidden_page`.
fn build_general(
    mtm: MainThreadMarker,
    [general_page, hidden_page]: [&NSView; 2],
    width: f64,
    settings: &Settings,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> GeneralParts {
    let text_width = width - 100.0;
    let changed = |slider_moved: bool| {
        let me = me.clone();
        move |_: &AnyObject| {
            if let Some(w) = me.upgrade() {
                w.general_changed(slider_moved);
            }
        }
    };

    // General: the menu bar icon (v1.4).
    let (icon_style, custom_note, custom_tint) =
        build_icon_group(mtm, general_page, width, ROW, settings, me, targets);

    // General: the dividers' chevrons (v1.12).
    let top = 8.0 + 2.0 * ROW + 16.0;
    let dividers_group = RoundedBox::new(mtm, rect(MARGIN, top, width, ROW), BoxStyle::Group);
    add(general_page, &dividers_group, Follow::Width);
    let show_dividers = switch_row(
        mtm,
        &dividers_group,
        (
            "Show section dividers",
            "Show a chevron where a shown section starts; off, a blank space",
        ),
        0.0,
        width,
        settings.show_section_dividers,
    );

    // General: launch at login and updates (v1.3).
    let top = top + ROW + 16.0;
    let system = RoundedBox::new(mtm, rect(MARGIN, top, width, 2.0 * ROW), BoxStyle::Group);
    add(general_page, &system, Follow::Width);
    add(
        &system,
        &label(
            mtm,
            "Launch at login",
            rect(16.0, 11.0, text_width - 150.0, 17.0),
            13.0,
            Weight::Medium,
            &NSColor::labelColor(),
        ),
        Follow::Width,
    );
    let login_note = label(
        mtm,
        "",
        rect(16.0, 30.0, text_width - 150.0, 15.0),
        11.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    add(&system, &login_note, Follow::Width);
    // SAFETY: no target or action yet; both are set by `attach` below.
    let login_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Open Login Items"),
            None,
            None,
            mtm,
        )
    };
    login_button.setFrame(rect(width - 206.0, 13.0, 140.0, 30.0));
    login_button.setHidden(true);
    add(&system, &login_button, Follow::Right);
    let login = NSSwitch::initWithFrame(NSSwitch::alloc(mtm), rect(width - 58.0, 16.0, 42.0, 24.0));
    add(&system, &login, Follow::Right);
    separator(mtm, &system, ROW, width);
    let auto_updates = switch_row(
        mtm,
        &system,
        (
            "Automatically check for updates",
            "Check GitHub for a new version once a day",
        ),
        ROW,
        width,
        settings.auto_check_updates,
    );

    // Hidden Items: how hidden items show (v1.5, v1.6).
    let show = RoundedBox::new(mtm, rect(MARGIN, 8.0, width, 6.0 * ROW), BoxStyle::Group);
    add(hidden_page, &show, Follow::Width);
    let use_bar = switch_row(
        mtm,
        &show,
        (
            "Use Ice Bar",
            "Show hidden items in a bar below the menu bar",
        ),
        0.0,
        width,
        settings.use_ice_bar,
    );
    separator(mtm, &show, ROW, width);
    let hide_app_menus = switch_row(
        mtm,
        &show,
        (
            "Hide app menus",
            "Hide the app's menus while shown items need the room (Use Ice Bar off)",
        ),
        ROW,
        width,
        settings.hide_app_menus,
    );
    hide_app_menus.setEnabled(app_menus_switch_enabled(settings.use_ice_bar));
    separator(mtm, &show, 2.0 * ROW, width);
    let show_on_click = switch_row(
        mtm,
        &show,
        (
            "Show on click",
            "Click an empty part of the menu bar to show or hide hidden items",
        ),
        2.0 * ROW,
        width,
        settings.show_on_click,
    );
    separator(mtm, &show, 3.0 * ROW, width);
    let show_on_hover = switch_row(
        mtm,
        &show,
        (
            "Show on hover",
            "Show hidden items while the pointer rests on an empty part of the menu bar",
        ),
        3.0 * ROW,
        width,
        settings.show_on_hover,
    );
    separator(mtm, &show, 4.0 * ROW, width);
    let delay = settings.hover_delay.as_secs_f64();
    let (hover_delay, hover_delay_value) = slider_row(
        mtm,
        &show,
        (
            "Hover delay",
            "How long the pointer rests before items show or hide",
        ),
        4.0 * ROW,
        width,
        (delay, 0.0, MAX_HOVER_DELAY.as_secs_f64()),
        &tenths_text(delay),
    );
    hover_delay.setEnabled(settings.show_on_hover);
    separator(mtm, &show, 5.0 * ROW, width);
    let show_on_scroll = switch_row(
        mtm,
        &show,
        (
            "Show on scroll",
            "Swipe or scroll down over the menu bar to show hidden items, up to hide them",
        ),
        5.0 * ROW,
        width,
        settings.show_on_scroll,
    );

    // Hidden Items: rehide. The Strategy picker joins the switch's row and
    // its detail line describes the selected strategy (SPEC S3, D9); the
    // interval row shows only for Timed. `SettingsWindow::layout_rehide`
    // sizes the group.
    let top = 8.0 + 6.0 * ROW + 16.0;
    let rehide = RoundedBox::new(mtm, rect(MARGIN, top, width, 2.0 * ROW), BoxStyle::Group);
    add(hidden_page, &rehide, Follow::Width);
    let picker_width = 128.0;
    let note_width = width - 16.0 - (58.0 + 8.0 + picker_width) - 12.0;
    add(
        &rehide,
        &label(
            mtm,
            "Automatically rehide",
            rect(16.0, 11.0, note_width, 17.0),
            13.0,
            Weight::Medium,
            &NSColor::labelColor(),
        ),
        Follow::Width,
    );
    let rehide_note = label(
        mtm,
        AUTO_REHIDE_NOTE,
        rect(16.0, 30.0, note_width, 15.0),
        11.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    add(&rehide, &rehide_note, Follow::Width);
    let strategy = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        rect(width - 66.0 - picker_width, 15.0, picker_width, 26.0),
        false,
    );
    for choice in RehideStrategy::ALL {
        strategy.addItemWithTitle(&NSString::from_str(choice.label()));
    }
    strategy.setToolTip(Some(&NSString::from_str("Strategy")));
    add(&rehide, &strategy, Follow::Right);
    let auto_rehide =
        NSSwitch::initWithFrame(NSSwitch::alloc(mtm), rect(width - 58.0, 16.0, 42.0, 24.0));
    auto_rehide.setState(switch_state(settings.auto_rehide));
    add(&rehide, &auto_rehide, Follow::Right);
    let interval_row = FlippedView::new(mtm, rect(0.0, ROW, width, ROW));
    add(&rehide, &interval_row, Follow::Width);
    separator(mtm, &interval_row, 0.0, width);
    let seconds = ui_rehide_seconds(settings.rehide_interval);
    let (interval, interval_value) = slider_row(
        mtm,
        &interval_row,
        ("Rehide after", "Idle time before shown items hide again"),
        0.0,
        width,
        (seconds, UI_REHIDE_MIN_SECONDS, UI_REHIDE_MAX_SECONDS),
        &seconds_text(seconds),
    );

    let controls: [(&NSControl, bool); 12] = [
        (&show_dividers, false),
        (&use_bar, false),
        (&hide_app_menus, false),
        (&show_on_click, false),
        (&show_on_hover, false),
        (&hover_delay, false),
        (&show_on_scroll, false),
        (&auto_rehide, false),
        (&strategy, false),
        (&interval, true),
        (&auto_updates, false),
        (&custom_tint, false),
    ];
    for (control, slider_moved) in controls {
        let target = ActionTarget::new(mtm, changed(slider_moved));
        target.attach(control);
        targets.push(target);
    }
    let me_login = me.clone();
    let target = ActionTarget::new(mtm, move |_| {
        if let Some(w) = me_login.upgrade() {
            w.login_changed();
        }
    });
    target.attach(&login);
    targets.push(target);
    let target = ActionTarget::new(mtm, |_| login_item::open_login_items());
    target.attach(&login_button);
    targets.push(target);

    let hidden_note = wrapping_label(
        mtm,
        "Click the ice-rust icon or an empty part of the menu bar to show hidden items; \
         Option-click either to show the always-hidden items too. Right-click or \
         Control-click the icon for the menu. Show on click and Show on hover need \
         Accessibility to tell empty space from the app's menus.",
        rect(MARGIN + 4.0, top + 2.0 * ROW + NOTE_GAP, width - 8.0, 52.0),
        11.0,
        &NSColor::secondaryLabelColor(),
    );
    add(hidden_page, &hidden_note, Follow::Width);

    GeneralParts {
        icon_style,
        custom_note,
        custom_tint,
        show_dividers,
        use_bar,
        hide_app_menus,
        show_on_click,
        show_on_hover,
        hover_delay,
        hover_delay_value,
        show_on_scroll,
        auto_rehide,
        rehide_note,
        strategy,
        rehide_group: rehide,
        interval_row,
        interval,
        interval_value,
        hidden_note,
        login,
        login_note,
        login_button,
        auto_updates,
    }
}

/// The Menu bar item spacing group (v1.13): a slider from -16 to +16 in
/// steps of 2 with its value, and Reset and Apply, which act only after the
/// app asks (`SettingsHost::apply_item_spacing`).
fn build_spacing_group(
    mtm: MainThreadMarker,
    page: &NSView,
    top: f64,
    width: f64,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> SpacingParts {
    use crate::spacing::{MAX_OFFSET, MIN_OFFSET, OFFSET_STEP, offset_label};
    let group = RoundedBox::new(mtm, rect(MARGIN, top, width, 2.0 * ROW), BoxStyle::Group);
    add(page, &group, Follow::Width);
    let (slider, value) = slider_row(
        mtm,
        &group,
        (
            "Menu bar item spacing",
            "The space around each item in the menu bar",
        ),
        0.0,
        width,
        (0.0, MIN_OFFSET as f64, MAX_OFFSET as f64),
        &offset_label(0),
    );
    slider.setNumberOfTickMarks(((MAX_OFFSET - MIN_OFFSET) / OFFSET_STEP + 1) as isize);
    slider.setAllowsTickMarkValuesOnly(true);
    // Room for "Default" next to the slider.
    slider.setFrame(rect(width - 246.0, 18.0, 150.0, 20.0));
    value.setFrame(rect(width - 88.0, 19.0, 74.0, 17.0));
    separator(mtm, &group, ROW, width);
    let note = label(
        mtm,
        SPACING_NOTE,
        rect(16.0, ROW + 20.0, width - 260.0, 15.0),
        11.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    add(&group, &note, Follow::Width);
    // SAFETY: no target or action yet; `attach` below sets both.
    let (reset, apply) = unsafe {
        (
            NSButton::buttonWithTitle_target_action(&NSString::from_str("Reset"), None, None, mtm),
            NSButton::buttonWithTitle_target_action(&NSString::from_str("Apply"), None, None, mtm),
        )
    };
    reset.setFrame(rect(width - 216.0, ROW + 13.0, 90.0, 30.0));
    apply.setFrame(rect(width - 116.0, ROW + 13.0, 100.0, 30.0));
    add(&group, &reset, Follow::Right);
    add(&group, &apply, Follow::Right);
    let mut attach = |control: &NSControl, action: fn(&SettingsWindow)| {
        let me = me.clone();
        let target = ActionTarget::new(mtm, move |_| {
            if let Some(w) = me.upgrade() {
                action(&w);
            }
        });
        target.attach(control);
        targets.push(target);
    };
    attach(&slider, SettingsWindow::spacing_changed);
    attach(&apply, SettingsWindow::apply_spacing);
    attach(&reset, SettingsWindow::reset_spacing);
    SpacingParts {
        slider,
        value,
        apply,
        reset,
        note,
    }
}

/// The Menu bar icon group (v1.4): the style popup, and a row to choose
/// the custom image and whether it takes the menu bar's color.
fn build_icon_group(
    mtm: MainThreadMarker,
    page: &NSView,
    width: f64,
    row: f64,
    settings: &Settings,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> (
    Retained<NSPopUpButton>,
    Retained<NSTextField>,
    Retained<NSButton>,
) {
    let group = RoundedBox::new(mtm, rect(MARGIN, 8.0, width, 2.0 * row), BoxStyle::Group);
    add(page, &group, Follow::Width);

    row_text(
        mtm,
        &group,
        "Menu bar icon",
        "The icon you click to show and hide items",
        16.0,
        11.0,
        width - 240.0,
    );
    let icon_style = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        rect(width - 196.0, 15.0, 180.0, 26.0),
        false,
    );
    for style in IconStyle::ALL {
        icon_style.addItemWithTitle(&NSString::from_str(style.title()));
        // The Custom Image item gets its image from `sync_icon`.
        let image = menu_icon::style_images(style, true).map(|images| images.hidden);
        if let (Some(item), Some(image)) = (icon_style.lastItem(), image) {
            item.setImage(Some(&image));
        }
    }
    add(&group, &icon_style, Follow::Right);
    separator(mtm, &group, row, width);

    let text_width = width - 360.0;
    add(
        &group,
        &label(
            mtm,
            "Custom image",
            rect(16.0, row + 11.0, text_width, 17.0),
            13.0,
            Weight::Medium,
            &NSColor::labelColor(),
        ),
        Follow::Width,
    );
    let custom_note = label(
        mtm,
        CUSTOM_NOTE,
        rect(16.0, row + 30.0, text_width, 15.0),
        11.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    add(&group, &custom_note, Follow::Width);
    // SAFETY: no target or action yet; both are set by `attach` below.
    let choose = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Choose Image..."),
            None,
            None,
            mtm,
        )
    };
    choose.setFrame(rect(width - 344.0, row + 13.0, 140.0, 30.0));
    add(&group, &choose, Follow::Right);
    // SAFETY: as above.
    let custom_tint = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Match menu bar color"),
            None,
            None,
            mtm,
        )
    };
    custom_tint.setFrame(rect(width - 196.0, row + 18.0, 180.0, 20.0));
    custom_tint.setState(switch_state(settings.tint_custom_icon));
    custom_tint.setEnabled(settings.icon_style == IconStyle::Custom);
    add(&group, &custom_tint, Follow::Right);

    let mut attach = |control: &NSControl, action: fn(&SettingsWindow)| {
        let me = me.clone();
        let target = ActionTarget::new(mtm, move |_| {
            if let Some(w) = me.upgrade() {
                action(&w);
            }
        });
        target.attach(control);
        targets.push(target);
    };
    attach(&icon_style, SettingsWindow::icon_style_changed);
    attach(&choose, SettingsWindow::choose_custom_icon);
    (icon_style, custom_note, custom_tint)
}

fn build_layout(
    mtm: MainThreadMarker,
    page: &NSView,
    width: f64,
    me: &Weak<SettingsWindow>,
) -> (Retained<LayoutCanvas>, Retained<NSProgressIndicator>) {
    add(
        page,
        &label(
            mtm,
            "Drag to arrange your menu bar items",
            rect(MARGIN, 6.0, width - 30.0, 20.0),
            15.0,
            Weight::Semibold,
            &NSColor::labelColor(),
        ),
        Follow::Width,
    );
    let spinner = NSProgressIndicator::initWithFrame(
        NSProgressIndicator::alloc(mtm),
        rect(MARGIN + width - 16.0, 8.0, 16.0, 16.0),
    );
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setDisplayedWhenStopped(false);
    spinner.setHidden(true);
    add(page, &spinner, Follow::Right);

    let tip = RoundedBox::new(mtm, rect(MARGIN, 36.0, width, 40.0), BoxStyle::Tip);
    if let Some(bulb) = symbol_view(
        mtm,
        "lightbulb",
        rect(12.0, 10.0, 20.0, 20.0),
        14.0,
        &NSColor::systemYellowColor(),
    ) {
        tip.addSubview(&bulb);
    }
    add(
        &tip,
        &label(
            mtm,
            "Tip: you can also arrange menu bar items by Command + dragging them in the menu bar",
            rect(40.0, 11.0, width - 52.0, 17.0),
            12.0,
            Weight::Regular,
            &NSColor::secondaryLabelColor(),
        ),
        Follow::Width,
    );
    add(page, &tip, Follow::Width);

    let snapshot = LayoutSnapshot::empty("Loading...");
    let height = LayoutCanvas::content_height(30.0).max(page.bounds().size.height - 96.0);
    let canvas = LayoutCanvas::new(mtm, rect(MARGIN, 92.0, width, height), snapshot);
    let me = me.clone();
    canvas.set_on_drop(Box::new(move |item, section, index| {
        if let Some(w) = me.upgrade() {
            w.dropped(item, section, index);
        }
    }));
    add(page, &canvas, Follow::Both);
    (canvas, spinner)
}

fn build_permissions(
    mtm: MainThreadMarker,
    page: &NSView,
    width: f64,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> Vec<(Permission, PermissionRow)> {
    const ROW: f64 = 64.0;
    add(
        page,
        &wrapping_label(
            mtm,
            "ice-rust needs these permissions to read, show and move menu bar items. \
             Accessibility updates here within a second; Screen Recording applies after \
             ice-rust relaunches.",
            rect(MARGIN + 4.0, 6.0, width - 8.0, 34.0),
            12.0,
            &NSColor::secondaryLabelColor(),
        ),
        Follow::Width,
    );
    let group = RoundedBox::new(mtm, rect(MARGIN, 48.0, width, 2.0 * ROW), BoxStyle::Group);
    add(page, &group, Follow::Width);
    let rows = [
        (
            Permission::ScreenRecording,
            "Screen Recording",
            "Reads item titles and captures item images",
        ),
        (
            Permission::Accessibility,
            "Accessibility",
            "Clicks and moves items, finds empty menu bar space",
        ),
    ];
    let mut out = Vec::new();
    for (i, (kind, title, detail)) in rows.into_iter().enumerate() {
        let y = i as f64 * ROW;
        if i > 0 {
            separator(mtm, &group, y, width);
        }
        let icon_frame = rect(16.0, y + 20.0, 24.0, 24.0);
        let granted_icon = symbol_view(
            mtm,
            "checkmark.circle.fill",
            icon_frame,
            18.0,
            &NSColor::systemGreenColor(),
        );
        let missing_icon = symbol_view(
            mtm,
            "exclamationmark.circle.fill",
            icon_frame,
            18.0,
            &NSColor::systemOrangeColor(),
        );
        for icon in [&granted_icon, &missing_icon].into_iter().flatten() {
            group.addSubview(icon);
        }
        row_text(mtm, &group, title, detail, 52.0, y + 14.0, width - 304.0);
        // Status at the right edge, the button left of it; a granted row
        // hides the button.
        let status = label(
            mtm,
            "",
            rect(width - 86.0, y + 23.0, 70.0, 17.0),
            12.0,
            Weight::Medium,
            &NSColor::secondaryLabelColor(),
        );
        status.setAlignment(NSTextAlignment::Right);
        add(&group, &status, Follow::Right);
        // SAFETY: no target or action yet; both are set by `attach` below.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Grant Access"),
                None,
                None,
                mtm,
            )
        };
        button.setFrame(rect(width - 244.0, y + 17.0, 150.0, 30.0));
        add(&group, &button, Follow::Right);
        let me = me.clone();
        let target = ActionTarget::new(mtm, move |_| {
            if let Some(w) = me.upgrade() {
                w.permission_clicked(kind);
            }
        });
        target.attach(&button);
        targets.push(target);
        out.push((
            kind,
            PermissionRow {
                granted_icon,
                missing_icon,
                status,
                button,
                requested: Cell::new(false),
            },
        ));
    }
    add(
        page,
        &wrapping_label(
            mtm,
            "If IceRust is switched on in System Settings but shows as missing here, select it \
             there and remove it with the minus (-) button, then grant it again. Turning the \
             switch off and on does not help.",
            rect(MARGIN + 4.0, 48.0 + 2.0 * ROW + 12.0, width - 8.0, 52.0),
            12.0,
            &NSColor::secondaryLabelColor(),
        ),
        Follow::Width,
    );
    out
}

fn build_about(
    mtm: MainThreadMarker,
    page: &NSView,
    content_width: f64,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> Retained<NSTextField> {
    let center = |w: f64| (content_width - w) / 2.0;
    if let Some(image) = brand::app_icon() {
        let view = NSImageView::imageViewWithImage(&image, mtm);
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        view.setFrame(rect(center(88.0), 28.0, 88.0, 88.0));
        add(page, &view, Follow::Center);
    } else if let Some(image) = symbol("menubar.rectangle", 52.0, Weight::Regular) {
        let view = NSImageView::imageViewWithImage(&image, mtm);
        view.setContentTintColor(Some(&NSColor::controlAccentColor()));
        view.setFrame(rect(center(80.0), 40.0, 80.0, 64.0));
        add(page, &view, Follow::Center);
    }
    let centered = |text: &str, y: f64, h: f64, size: f64, weight: Weight, color: &NSColor| {
        let field = label(
            mtm,
            text,
            rect(MARGIN, y, content_width - 2.0 * MARGIN, h),
            size,
            weight,
            color,
        );
        field.setAlignment(NSTextAlignment::Center);
        add(page, &field, Follow::Width);
    };
    centered(
        "ice-rust",
        116.0,
        34.0,
        26.0,
        Weight::Bold,
        &NSColor::labelColor(),
    );
    centered(
        &format!("Version {}", env!("CARGO_PKG_VERSION")),
        154.0,
        18.0,
        13.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    centered(
        "Hides menu bar items behind dividers and shows them in a bar below the menu bar.",
        192.0,
        18.0,
        13.0,
        Weight::Regular,
        &NSColor::labelColor(),
    );
    centered(
        "A Rust port of Ice by Jordan Baird. Not affiliated with or endorsed by Ice.",
        220.0,
        18.0,
        12.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );

    // Updates (v1.3): the status line and a manual check.
    let status = label(
        mtm,
        "",
        rect(MARGIN, 262.0, content_width - 2.0 * MARGIN, 18.0),
        12.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    status.setAlignment(NSTextAlignment::Center);
    add(page, &status, Follow::Width);
    // SAFETY: no target or action yet; both are set by `attach` below.
    let check = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Check for Updates..."),
            None,
            None,
            mtm,
        )
    };
    check.setFrame(rect(center(180.0), 288.0, 180.0, 30.0));
    add(page, &check, Follow::Center);
    let me = me.clone();
    let target = ActionTarget::new(mtm, move |_| {
        if let Some(w) = me.upgrade() {
            w.host.check_for_updates();
            w.refresh_update_status();
        }
    });
    target.attach(&check);
    targets.push(target);
    status
}
