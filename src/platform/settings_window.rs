//! The Settings window (v1.1): a vibrant sidebar with the app title and
//! one row per page, and pages for General, Menu Bar Layout, Permissions
//! and About. The same window renders offscreen for `render-settings`.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::Duration;

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
use crate::arrange::{DropOutcome, drop_destination, model_matches};
use crate::login_item as login;
use crate::menu_icon::IconStyle;
use crate::model::{LAYOUT_REFRESH, PERMISSIONS_REFRESH, WindowId};
use crate::sections::Section;
use crate::settings::{Settings, UI_REHIDE_MAX_SECONDS, UI_REHIDE_MIN_SECONDS, ui_rehide_seconds};

/// Default content size of the window, also its minimum (v1.2).
pub const DEFAULT_SIZE: (f64, f64) = (820.0, 600.0);
const SIDEBAR_WIDTH: f64 = 216.0;
/// Height of the title bar area that holds the page title.
const HEADER_HEIGHT: f64 = 44.0;
const MARGIN: f64 = 24.0;
/// Pause before re-reading the menu bar after a move, so the sections have
/// settled back.
const SETTLE_AFTER_MOVE: Duration = Duration::from_millis(300);

/// The Custom image row's note while nothing went wrong.
const CUSTOM_NOTE: &str = "Used by Custom Image";

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
    fn request_move(&self, request: MoveRequest, done: MoveCallback);
    /// The window is closing.
    fn closed(&self);
    /// "Check for Updates..." on the About page.
    fn check_for_updates(&self);
    /// The About page's update status line.
    fn update_status_line(&self) -> String;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Layout,
    Permissions,
    About,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::General, Page::Layout, Page::Permissions, Page::About];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Layout => "Menu Bar Layout",
            Page::Permissions => "Permissions",
            Page::About => "About",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Page::General => "gearshape",
            Page::Layout => "menubar.rectangle",
            Page::Permissions => "lock.shield",
            Page::About => "info.circle",
        }
    }

    /// The `--page` names of `render-settings`.
    pub fn from_arg(name: &str) -> Option<Page> {
        match name {
            "general" => Some(Page::General),
            "layout" => Some(Page::Layout),
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

struct GeneralParts {
    icon_style: Retained<NSPopUpButton>,
    custom_note: Retained<NSTextField>,
    custom_tint: Retained<NSButton>,
    use_bar: Retained<NSSwitch>,
    auto_rehide: Retained<NSSwitch>,
    interval: Retained<NSSlider>,
    interval_value: Retained<NSTextField>,
    login: Retained<NSSwitch>,
    login_note: Retained<NSTextField>,
    login_button: Retained<NSButton>,
    auto_updates: Retained<NSSwitch>,
}

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
    canvas: Retained<LayoutCanvas>,
    spinner: Retained<NSProgressIndicator>,
    permission_rows: Vec<(Permission, PermissionRow)>,
    about_status: Retained<NSTextField>,
    /// Targets of the controls; controls do not retain their targets.
    _targets: Vec<Retained<ActionTarget>>,
    refresh: RefCell<Option<Retained<NSTimer>>>,
    ticks: Cell<u32>,
    /// The last layout refresh timed out, so the next periodic one is
    /// skipped to let the stuck capture finish.
    skip_refresh: Cell<bool>,
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

            // General.
            let general_view = FlippedView::new(mtm, page_frame);
            let general =
                build_general(mtm, &general_view, page_width, &settings, me, &mut targets);

            // Menu Bar Layout.
            let layout_view = FlippedView::new(mtm, page_frame);
            let (canvas, spinner) = build_layout(mtm, &layout_view, page_width, me);

            // Permissions.
            let permissions_view = FlippedView::new(mtm, page_frame);
            let permission_rows =
                build_permissions(mtm, &permissions_view, page_width, me, &mut targets);

            // About.
            let about_view = FlippedView::new(mtm, page_frame);
            let about_status = build_about(mtm, &about_view, content_width, me, &mut targets);

            let pages = vec![
                (Page::General, general_view),
                (Page::Layout, layout_view),
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
                canvas,
                spinner,
                permission_rows,
                about_status,
                _targets: targets,
                refresh: RefCell::new(None),
                ticks: Cell::new(0),
                skip_refresh: Cell::new(false),
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
            Page::General => self.sync_general(),
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
                    self.reload_layout();
                }
            }
            _ => {}
        }
    }

    fn will_close(&self) {
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
        let seconds = ui_rehide_seconds(s.rehide_interval);
        self.general.interval.setDoubleValue(seconds);
        self.general
            .interval_value
            .setStringValue(&NSString::from_str(&seconds_text(seconds)));
        self.general.interval.setEnabled(s.auto_rehide);
        self.general
            .auto_updates
            .setState(switch_state(s.auto_check_updates));
        self.sync_icon();
        self.sync_login();
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
    /// interval is read only when the slider moved, so a stored value
    /// outside the slider's range survives changes to the switches.
    fn general_changed(&self, slider_moved: bool) {
        let mut s = self.host.settings();
        s.use_ice_bar = self.general.use_bar.state() == NSControlStateValueOn;
        s.auto_rehide = self.general.auto_rehide.state() == NSControlStateValueOn;
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
        self.general.interval.setEnabled(s.auto_rehide);
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

    fn dropped(&self, item: WindowId, section: Section, index: usize) {
        let decision = self.canvas.with_snapshot(|s| {
            s.model.as_ref().map(|m| {
                (
                    drop_destination(m, item, section, index),
                    m.find(item).map(|(from, _)| from),
                )
            })
        });
        match decision {
            None | Some((Ok(DropOutcome::NoChange), _)) => {}
            Some((Err(err), _)) => self.canvas.set_message(Some(err.to_string()), true),
            Some((Ok(DropOutcome::Move(destination)), Some(from))) => {
                self.set_busy(true);
                self.canvas.set_message(Some("Moving...".into()), false);
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
                            w.move_finished(request, result);
                        }
                    }),
                );
            }
            Some((Ok(DropOutcome::Move(_)), None)) => {}
        }
    }

    fn move_finished(&self, request: MoveRequest, result: Result<(), MoveError>) {
        let me = self.me.clone();
        timer::after(self.mtm, SETTLE_AFTER_MOVE, move || {
            let Some(w) = me.upgrade() else {
                return;
            };
            let snapshot = w.host.load_layout();
            let matches = snapshot
                .model
                .as_ref()
                .is_some_and(|m| model_matches(m, request.item, request.destination));
            w.show_snapshot(snapshot);
            w.set_busy(false);
            let message = match (&result, matches) {
                (Ok(()), true) => None,
                (Ok(()), false) => {
                    Some("The menu bar order does not match the requested order".to_string())
                }
                (Err(err), _) => Some(format!("Move failed: {err}")),
            };
            let error = message.is_some();
            w.canvas.set_message(message, error);
        });
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
fn row_text(
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

fn separator(mtm: MainThreadMarker, parent: &NSView, y: f64, width: f64) {
    let line = RoundedBox::new(mtm, rect(16.0, y, width - 32.0, 1.0), BoxStyle::Separator);
    add(parent, &line, Follow::Width);
}

fn build_general(
    mtm: MainThreadMarker,
    page: &NSView,
    width: f64,
    settings: &Settings,
    me: &Weak<SettingsWindow>,
    targets: &mut Vec<Retained<ActionTarget>>,
) -> GeneralParts {
    const ROW: f64 = 56.0;
    let (icon_style, custom_note, custom_tint) =
        build_icon_group(mtm, page, width, ROW, settings, me, targets);

    let first = 8.0 + 2.0 * ROW + 16.0;
    let group = RoundedBox::new(mtm, rect(MARGIN, first, width, 3.0 * ROW), BoxStyle::Group);
    add(page, &group, Follow::Width);
    // Rows with a switch leave room for it; the slider row for the slider.
    let text_width = width - 100.0;
    let slider_text_width = width - 240.0;

    let changed = |slider_moved: bool| {
        let me = me.clone();
        move |_: &AnyObject| {
            if let Some(w) = me.upgrade() {
                w.general_changed(slider_moved);
            }
        }
    };

    row_text(
        mtm,
        &group,
        "Use Ice Bar",
        "Show hidden items in a bar below the menu bar",
        16.0,
        11.0,
        text_width,
    );
    let use_bar =
        NSSwitch::initWithFrame(NSSwitch::alloc(mtm), rect(width - 58.0, 16.0, 42.0, 24.0));
    use_bar.setState(switch_state(settings.use_ice_bar));
    add(&group, &use_bar, Follow::Right);
    separator(mtm, &group, ROW, width);

    row_text(
        mtm,
        &group,
        "Automatically rehide",
        "Hide shown items again while the pointer is away from the menu bar",
        16.0,
        ROW + 11.0,
        text_width,
    );
    let auto_rehide = NSSwitch::initWithFrame(
        NSSwitch::alloc(mtm),
        rect(width - 58.0, ROW + 16.0, 42.0, 24.0),
    );
    auto_rehide.setState(switch_state(settings.auto_rehide));
    add(&group, &auto_rehide, Follow::Right);
    separator(mtm, &group, 2.0 * ROW, width);

    row_text(
        mtm,
        &group,
        "Rehide after",
        "Idle time before shown items hide again",
        16.0,
        2.0 * ROW + 11.0,
        slider_text_width,
    );
    let seconds = ui_rehide_seconds(settings.rehide_interval);
    // SAFETY: no target or action yet; both are set by `attach` below.
    let interval = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(
            seconds,
            UI_REHIDE_MIN_SECONDS,
            UI_REHIDE_MAX_SECONDS,
            None,
            None,
            mtm,
        )
    };
    interval.setFrame(rect(width - 216.0, 2.0 * ROW + 18.0, 150.0, 20.0));
    interval.setContinuous(true);
    interval.setEnabled(settings.auto_rehide);
    add(&group, &interval, Follow::Right);
    let interval_value = label(
        mtm,
        &seconds_text(seconds),
        rect(width - 58.0, 2.0 * ROW + 19.0, 44.0, 17.0),
        13.0,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
    );
    interval_value.setAlignment(NSTextAlignment::Right);
    add(&group, &interval_value, Follow::Right);

    let controls: [(&NSControl, bool); 3] =
        [(&use_bar, false), (&auto_rehide, false), (&interval, true)];
    for (control, slider_moved) in controls {
        let target = ActionTarget::new(mtm, changed(slider_moved));
        target.attach(control);
        targets.push(target);
    }

    // Launch at login and updates (v1.3).
    let top = first + 3.0 * ROW + 16.0;
    let system = RoundedBox::new(mtm, rect(MARGIN, top, width, 2.0 * ROW), BoxStyle::Group);
    add(page, &system, Follow::Width);
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

    row_text(
        mtm,
        &system,
        "Automatically check for updates",
        "Check GitHub for a new version once a day",
        16.0,
        ROW + 11.0,
        text_width,
    );
    let auto_updates = NSSwitch::initWithFrame(
        NSSwitch::alloc(mtm),
        rect(width - 58.0, ROW + 16.0, 42.0, 24.0),
    );
    auto_updates.setState(switch_state(settings.auto_check_updates));
    add(&system, &auto_updates, Follow::Right);

    let target = ActionTarget::new(mtm, changed(false));
    target.attach(&auto_updates);
    targets.push(target);
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

    add(
        page,
        &wrapping_label(
            mtm,
            "Click the ice-rust icon to show hidden items. Option-click it to show the \
             always-hidden items too. Right-click or Control-click it for the menu.",
            rect(MARGIN + 4.0, top + 2.0 * ROW + 14.0, width - 8.0, 36.0),
            11.0,
            &NSColor::secondaryLabelColor(),
        ),
        Follow::Width,
    );

    GeneralParts {
        icon_style,
        custom_note,
        custom_tint,
        use_bar,
        auto_rehide,
        interval,
        interval_value,
        login,
        login_note,
        login_button,
        auto_updates,
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
    attach(&custom_tint, |w| w.general_changed(false));
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
            "Clicks and moves menu bar items",
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
