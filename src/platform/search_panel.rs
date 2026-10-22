//! The search panel (v1.11): a floating panel with a search field over the
//! menu bar items, like Ice's. Typing filters the list (`crate::search`);
//! the arrow keys move the selection, and Return, a double click or the
//! Click Item button clicks the selected item. Escape or a click anywhere
//! else closes the panel.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplication, NSBackingStoreType, NSBezierPath, NSBorderType,
    NSButton, NSCellImagePosition, NSColor, NSCompositingOperation, NSControl,
    NSControlTextEditingDelegate, NSEvent, NSEventMask, NSEventModifierFlags,
    NSFloatingWindowLevel, NSFocusRingType, NSImage, NSPanel, NSResponder, NSScreen, NSScrollView,
    NSTextField, NSTextFieldDelegate, NSTextView, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer};

use super::item_names;
use super::layout_pane::{LayoutSnapshot, draw_text};
use super::timer;
use super::ui::{
    ActionTarget, BoxStyle, FlippedView, RoundedBox, Weight, font, is_dark, rect, round_corners,
    srgb, symbol, symbol_view,
};
use crate::arrange::{BarModel, Tile};
use crate::brand;
use crate::model::{PANEL_COLLECTION_BEHAVIOR, PANEL_STYLE_MASK, Rect, WindowId};
use crate::search::{self, Entry, Row};
use crate::sections::Section;

/// Ice's search panel is 600 by 400 points.
const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 400.0;
const CORNER_RADIUS: f64 = 12.0;
/// Height of the area with the search field, above the list.
const FIELD_AREA: f64 = 52.0;
/// Height of the bar with the buttons, below the list.
const BUTTON_BAR: f64 = 40.0;
/// How often the names are read again while the panel is open: the app
/// names may arrive after it opened (`item_names::look_up`).
const NAME_REFRESH: Duration = Duration::from_millis(500);
const PLACEHOLDER: &str = "Search menu bar items";
const NO_ITEMS: &str = "No menu bar items";
const NO_MATCH: &str = "No items match";

/// Called with the chosen item and its section.
pub type ChooseHandler = Rc<dyn Fn(WindowId, Section)>;

/// What a listed item shows besides its name.
struct Listed {
    /// The item's own name (`BarModel::display_name`), from which
    /// `item_names::name` gives the one shown.
    own: String,
    icon: Option<Retained<NSImage>>,
    image: Option<Retained<NSImage>>,
    size: (f64, f64),
}

#[derive(Default)]
struct ListState {
    /// In list order (`search::entries`), beside `listed`.
    entries: Vec<Entry>,
    listed: Vec<Listed>,
    rows: Vec<Row>,
    /// The selected row.
    selected: Option<usize>,
    /// Why there is nothing to list.
    notice: Option<String>,
    /// The fill behind the item images (`brand::bar_fill_for`).
    fill: Option<[f64; 3]>,
}

define_class!(
    // SAFETY: NSPanel has no subclassing requirements beyond using its
    // designated initializer, which `KeyPanel::new` does. No Drop impl.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct KeyPanel;

    // SAFETY: canBecomeKeyWindow returns BOOL and performKeyEquivalent:
    // takes an NSEvent and returns BOOL, as NSWindow declares.
    impl KeyPanel {
        /// A borderless panel takes no keystrokes otherwise.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// Command-X, C, V and A reach the field: ice-rust's main menu
        /// has no Edit menu to carry them.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
            let action = event
                .charactersIgnoringModifiers()
                .filter(|_| flags == NSEventModifierFlags::Command)
                .and_then(|keys| edit_action(&keys.to_string()));
            if let Some(action) = action {
                let app = NSApplication::sharedApplication(self.mtm());
                // SAFETY: the actions are standard responder actions with
                // the `-(void)x:(id)sender` signature; a nil target sends
                // them to the key window's first responder.
                if unsafe { app.sendAction_to_from(action, None, Some(self)) } {
                    return Bool::YES;
                }
            }
            // SAFETY: as above; NSWindow's implementation.
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }
    }
);

/// The edit action of a Command key.
fn edit_action(key: &str) -> Option<Sel> {
    match key {
        "x" => Some(sel!(cut:)),
        "c" => Some(sel!(copy:)),
        "v" => Some(sel!(paste:)),
        "a" => Some(sel!(selectAll:)),
        _ => None,
    }
}

impl KeyPanel {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: initWithContentRect:styleMask:backing:defer: is NSWindow's
        // designated initializer; the arguments have its types.
        unsafe {
            msg_send![
                super(this),
                initWithContentRect: rect(0.0, 0.0, WIDTH, HEIGHT),
                styleMask: NSWindowStyleMask(PANEL_STYLE_MASK),
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `ResultList::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<SearchPanel>]
    pub struct ResultList;

    // SAFETY: argument and return types match NSView's declarations:
    // isFlipped -> BOOL, acceptsFirstMouse: (NSEvent *) -> BOOL, drawRect:
    // (NSRect) -> void, mouseDown: (NSEvent *) -> void.
    impl ResultList {
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
            if let Some(panel) = self.ivars().upgrade() {
                panel.draw_list(self.bounds().size.width);
            }
        }

        /// Selects the row under the pointer; a double click also clicks
        /// its item. The search field keeps the keyboard.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(panel) = self.ivars().upgrade() {
                panel.pressed(point.y, event.clickCount());
            }
        }
    }
);

impl ResultList {
    fn new(mtm: MainThreadMarker, panel: Weak<SearchPanel>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(panel);
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. No Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<SearchPanel>]
    pub struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    // SAFETY: controlTextDidChange: takes an NSNotification and returns
    // void; control:textView:doCommandBySelector: takes an NSControl, an
    // NSTextView and a selector and returns BOOL, as
    // NSControlTextEditingDelegate declares.
    unsafe impl NSControlTextEditingDelegate for FieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            if let Some(panel) = self.ivars().upgrade() {
                panel.query_changed();
            }
        }

        /// The arrow keys, Return and Escape act on the list.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _text_view: &NSTextView, command: Sel) -> Bool {
            let Some(panel) = self.ivars().upgrade() else {
                return Bool::NO;
            };
            if command == sel!(moveDown:) {
                panel.step(true);
            } else if command == sel!(moveUp:) {
                panel.step(false);
            } else if command == sel!(insertNewline:) {
                panel.choose_selected();
            } else if command == sel!(cancelOperation:) {
                panel.close();
            } else {
                return Bool::NO;
            }
            Bool::YES
        }
    }

    unsafe impl NSTextFieldDelegate for FieldDelegate {}
);

impl FieldDelegate {
    fn new(mtm: MainThreadMarker, panel: Weak<SearchPanel>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(panel);
        // SAFETY: init is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

pub struct SearchPanel {
    mtm: MainThreadMarker,
    panel: Retained<KeyPanel>,
    field: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    list: Retained<ResultList>,
    click_button: Retained<NSButton>,
    /// The field's delegate, which the field does not retain.
    _delegate: Retained<FieldDelegate>,
    /// The buttons' targets, which the buttons do not retain.
    _targets: Vec<Retained<ActionTarget>>,
    state: RefCell<ListState>,
    on_choose: ChooseHandler,
    on_settings: Rc<dyn Fn()>,
    /// Mouse-down monitors that close the panel on clicks elsewhere.
    monitors: RefCell<Vec<Retained<AnyObject>>>,
    refresh: RefCell<Option<Retained<NSTimer>>>,
}

impl SearchPanel {
    /// `on_choose` clicks an item; `on_settings` opens Settings. Both run
    /// after the panel closed.
    pub fn new(
        mtm: MainThreadMarker,
        on_choose: ChooseHandler,
        on_settings: Rc<dyn Fn()>,
    ) -> Rc<Self> {
        Rc::new_cyclic(|me: &Weak<SearchPanel>| {
            let panel = KeyPanel::new(mtm);
            // SAFETY: the panel is owned by `SearchPanel` and must survive
            // orderOut.
            unsafe { panel.setReleasedWhenClosed(false) };
            panel.setFloatingPanel(true);
            panel.setLevel(NSFloatingWindowLevel);
            panel.setCollectionBehavior(NSWindowCollectionBehavior(PANEL_COLLECTION_BEHAVIOR));
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setHidesOnDeactivate(false);

            let backdrop = NSVisualEffectView::new(mtm);
            backdrop.setFrame(rect(0.0, 0.0, WIDTH, HEIGHT));
            backdrop.setMaterial(NSVisualEffectMaterial::Popover);
            backdrop.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            backdrop.setState(NSVisualEffectState::Active);
            round_corners(&backdrop, CORNER_RADIUS);
            let content = FlippedView::new(mtm, rect(0.0, 0.0, WIDTH, HEIGHT));
            backdrop.addSubview(&content);
            panel.setContentView(Some(&backdrop));

            if let Some(glass) = symbol_view(
                mtm,
                "magnifyingglass",
                rect(16.0, 15.0, 22.0, 22.0),
                17.0,
                &NSColor::secondaryLabelColor(),
            ) {
                content.addSubview(&glass);
            }
            let delegate = FieldDelegate::new(mtm, me.clone());
            let field = NSTextField::textFieldWithString(&NSString::new(), mtm);
            field.setPlaceholderString(Some(&NSString::from_str(PLACEHOLDER)));
            field.setBezeled(false);
            field.setBordered(false);
            field.setDrawsBackground(false);
            field.setFocusRingType(NSFocusRingType::None);
            field.setUsesSingleLineMode(true);
            field.setFont(Some(&font(18.0, Weight::Regular)));
            field.setFrame(rect(46.0, 13.0, WIDTH - 62.0, 26.0));
            // SAFETY: `SearchPanel` keeps the delegate alive as long as the
            // field.
            unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            content.addSubview(&field);
            content.addSubview(&RoundedBox::new(
                mtm,
                rect(0.0, FIELD_AREA, WIDTH, 1.0),
                BoxStyle::Separator,
            ));

            let list_top = FIELD_AREA + 1.0;
            let list_height = HEIGHT - list_top - BUTTON_BAR - 1.0;
            let scroll = NSScrollView::new(mtm);
            scroll.setFrame(rect(0.0, list_top, WIDTH, list_height));
            scroll.setHasVerticalScroller(true);
            scroll.setHasHorizontalScroller(false);
            scroll.setAutohidesScrollers(true);
            scroll.setBorderType(NSBorderType::NoBorder);
            scroll.setDrawsBackground(false);
            scroll.contentView().setDrawsBackground(false);
            let list = ResultList::new(mtm, me.clone());
            list.setFrame(rect(0.0, 0.0, WIDTH, list_height));
            scroll.setDocumentView(Some(&list));
            content.addSubview(&scroll);
            content.addSubview(&RoundedBox::new(
                mtm,
                rect(0.0, HEIGHT - BUTTON_BAR - 1.0, WIDTH, 1.0),
                BoxStyle::Separator,
            ));

            let settings_target = {
                let me = me.clone();
                ActionTarget::new(mtm, move |_| {
                    if let Some(panel) = me.upgrade() {
                        panel.close();
                        (panel.on_settings)();
                    }
                })
            };
            let settings = NSButton::new(mtm);
            if let Some(gear) = symbol("gearshape", 15.0, Weight::Regular) {
                settings.setImage(Some(&gear));
            }
            settings.setBordered(false);
            settings.setFrame(rect(10.0, HEIGHT - BUTTON_BAR + 6.0, 28.0, 28.0));
            settings.setToolTip(Some(&NSString::from_str("Settings")));
            settings_target.attach(&settings);
            content.addSubview(&settings);

            let click_target = {
                let me = me.clone();
                ActionTarget::new(mtm, move |_| {
                    if let Some(panel) = me.upgrade() {
                        panel.choose_selected();
                    }
                })
            };
            let click_button = NSButton::new(mtm);
            click_button.setTitle(&NSString::from_str("Click Item"));
            if let Some(key) = symbol("return", 11.0, Weight::Regular) {
                click_button.setImage(Some(&key));
                click_button.setImagePosition(NSCellImagePosition::ImageTrailing);
            }
            click_button.setBordered(false);
            click_button.setFont(Some(&font(13.0, Weight::Regular)));
            click_button.setFrame(rect(WIDTH - 132.0, HEIGHT - BUTTON_BAR + 5.0, 120.0, 30.0));
            click_target.attach(&click_button);
            content.addSubview(&click_button);

            Self {
                mtm,
                panel,
                field,
                scroll,
                list,
                click_button,
                _delegate: delegate,
                _targets: vec![settings_target, click_target],
                state: RefCell::new(ListState::default()),
                on_choose,
                on_settings,
                monitors: RefCell::new(Vec::new()),
                refresh: RefCell::new(None),
            }
        })
    }

    pub fn is_visible(&self) -> bool {
        self.panel.isVisible()
    }

    /// Fills the list from `snapshot` (Menu Bar Layout's reading of the
    /// menu bar), centers the panel a little above the middle of the
    /// display with the pointer, as Ice does, and gives it the keyboard.
    pub fn show(self: &Rc<Self>, snapshot: &LayoutSnapshot) {
        self.load(snapshot);
        self.field.setStringValue(&NSString::new());
        self.refilter(true);
        self.place();
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.field));
        self.watch_clicks();
        let me = Rc::downgrade(self);
        let timer = timer::schedule(self.mtm, NAME_REFRESH, true, move |timer| {
            match me.upgrade() {
                Some(panel) => panel.refresh_names(),
                None => timer.invalidate(),
            }
        });
        if let Some(old) = self.refresh.borrow_mut().replace(timer) {
            old.invalidate();
        }
    }

    pub fn close(&self) {
        if let Some(timer) = self.refresh.borrow_mut().take() {
            timer.invalidate();
        }
        for monitor in self.monitors.borrow_mut().drain(..) {
            // SAFETY: each monitor came from an add...Monitor call and is
            // removed once.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if self.panel.isVisible() {
            self.panel.orderOut(None);
        }
        // The images and icons go with the list.
        *self.state.borrow_mut() = ListState::default();
    }

    fn load(&self, snapshot: &LayoutSnapshot) {
        let fill = brand::bar_fill_for(
            snapshot.light_glyphs,
            snapshot.bar_color,
            is_dark(&self.panel.effectiveAppearance()),
        );
        let state = match &snapshot.model {
            None => ListState {
                notice: Some(snapshot.notice.clone().unwrap_or_else(|| NO_ITEMS.into())),
                ..ListState::default()
            },
            Some(model) => {
                let entries = search::entries(model, model.anchors.icon, |tile| {
                    item_names::name(tile.id, &model.display_name(tile))
                });
                let listed = entries
                    .iter()
                    .map(|entry| {
                        let tile = find_tile(model, entry.id);
                        let image = snapshot.images.get(&entry.id);
                        Listed {
                            own: tile.map(|t| model.display_name(t)).unwrap_or_default(),
                            icon: item_names::icon(entry.id),
                            image: image.and_then(|i| i.image.clone()),
                            size: image.map_or((0.0, 0.0), |i| i.size),
                        }
                    })
                    .collect();
                ListState {
                    entries,
                    listed,
                    fill: Some(fill),
                    ..ListState::default()
                }
            }
        };
        *self.state.borrow_mut() = state;
    }

    /// Centered on the display with the pointer, its top an eighth of the
    /// display's height above the middle (Ice's placement).
    fn place(&self) {
        let pointer = NSEvent::mouseLocation();
        let screens = NSScreen::screens(self.mtm);
        let screen = screens
            .iter()
            .find(|s| {
                let f = s.frame();
                (f.origin.x..f.origin.x + f.size.width).contains(&pointer.x)
                    && (f.origin.y..f.origin.y + f.size.height).contains(&pointer.y)
            })
            .or_else(|| NSScreen::mainScreen(self.mtm));
        let Some(screen) = screen else {
            return;
        };
        let f = screen.frame();
        let top = f.origin.y + f.size.height / 2.0 + HEIGHT / 2.0 + f.size.height / 8.0;
        let x = f.origin.x + (f.size.width - WIDTH) / 2.0;
        self.panel
            .setFrame_display(rect(x, top - HEIGHT, WIDTH, HEIGHT), true);
    }

    /// A click in another app, or in another ice-rust window, closes the
    /// panel.
    fn watch_clicks(self: &Rc<Self>) {
        if !self.monitors.borrow().is_empty() {
            return;
        }
        let mask =
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        let me = Rc::downgrade(self);
        let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a live event for the duration of the
            // call.
            let number = unsafe { event.as_ref() }.windowNumber();
            if let Some(panel) = me.upgrade()
                && number != panel.panel.windowNumber()
            {
                panel.close();
            }
            event.as_ptr()
        });
        let me = Rc::downgrade(self);
        let global = RcBlock::new(move |_event: NonNull<NSEvent>| {
            if let Some(panel) = me.upgrade() {
                panel.close();
            }
        });
        let mut monitors = Vec::new();
        // SAFETY: the handler returns the event it was given, so AppKit
        // dispatches it unchanged.
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
        {
            monitors.push(monitor);
        }
        if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global)
        {
            monitors.push(monitor);
        }
        *self.monitors.borrow_mut() = monitors;
    }

    fn query_changed(&self) {
        self.refilter(true);
    }

    /// Rebuilds the rows for the query in the field. `reset` selects the
    /// first item and scrolls to the top; otherwise the selected item stays
    /// selected while it is listed.
    fn refilter(&self, reset: bool) {
        let query = self.field.stringValue().to_string();
        {
            let mut state = self.state.borrow_mut();
            let kept = state
                .selected
                .and_then(|row| match state.rows.get(row) {
                    Some(Row::Item(entry)) => Some(*entry),
                    _ => None,
                })
                .filter(|_| !reset);
            state.rows = search::rows(&state.entries, &query);
            state.selected = kept
                .and_then(|entry| search::row_of(&state.rows, entry))
                .or_else(|| search::first_item(&state.rows));
        }
        let height = search::row_tops(&self.state.borrow().rows).1;
        let visible = self.scroll.contentSize();
        self.list
            .setFrameSize(NSSize::new(visible.width, height.max(visible.height)));
        if reset {
            self.list.scrollPoint(NSPoint::new(0.0, 0.0));
        } else {
            self.show_selected();
        }
        self.selection_changed();
    }

    /// Reads the names and icons again, and refilters when one arrived.
    fn refresh_names(&self) {
        let changed = {
            let mut state = self.state.borrow_mut();
            let ListState {
                entries, listed, ..
            } = &mut *state;
            let mut changed = false;
            for (entry, listed) in entries.iter_mut().zip(listed.iter_mut()) {
                let name = item_names::name(entry.id, &listed.own);
                if name != entry.name {
                    entry.name = name;
                    changed = true;
                }
                if listed.icon.is_none()
                    && let Some(icon) = item_names::icon(entry.id)
                {
                    listed.icon = Some(icon);
                    changed = true;
                }
            }
            changed
        };
        if changed {
            self.refilter(false);
        }
    }

    /// Moves the selection down or up, as the arrow keys do.
    pub fn step(&self, down: bool) {
        {
            let mut state = self.state.borrow_mut();
            state.selected = search::step(&state.rows, state.selected, down);
        }
        self.show_selected();
        self.selection_changed();
    }

    /// The selected row and the clicks of a mouse down `y` from the list's
    /// top.
    fn pressed(&self, y: f64, clicks: isize) {
        let row = {
            let state = self.state.borrow();
            search::row_at(&state.rows, y).filter(|&r| matches!(state.rows[r], Row::Item(_)))
        };
        let Some(row) = row else {
            return;
        };
        self.state.borrow_mut().selected = Some(row);
        self.selection_changed();
        if clicks >= 2 {
            self.choose_selected();
        }
    }

    /// Closes the panel and clicks the selected item, as Return does.
    pub fn choose_selected(&self) {
        let chosen = {
            let state = self.state.borrow();
            match state.selected.and_then(|row| state.rows.get(row)) {
                Some(&Row::Item(entry)) => state.entries.get(entry).map(|e| (e.id, e.section)),
                _ => None,
            }
        };
        let Some((id, section)) = chosen else {
            return;
        };
        self.close();
        (self.on_choose)(id, section);
    }

    fn show_selected(&self) {
        let state = self.state.borrow();
        let Some(row) = state.selected else {
            return;
        };
        let (tops, _) = search::row_tops(&state.rows);
        if let (Some(top), Some(r)) = (tops.get(row), state.rows.get(row)) {
            // The neighbors' padding too, so a header above shows.
            let pad = search::LIST_PADDING;
            self.list
                .scrollRectToVisible(rect(0.0, top - pad, 1.0, r.height() + 2.0 * pad));
        }
    }

    fn selection_changed(&self) {
        let any = self.state.borrow().selected.is_some();
        self.click_button.setEnabled(any);
        self.list.setNeedsDisplay(true);
    }

    fn draw_list(&self, width: f64) {
        let state = self.state.borrow();
        if state.rows.is_empty() {
            let text = match &state.notice {
                Some(notice) => notice.as_str(),
                None if state.entries.is_empty() => NO_ITEMS,
                None => NO_MATCH,
            };
            draw_text(
                text,
                &Rect::new(16.0, 24.0, width - 32.0, 17.0),
                &font(13.0, Weight::Regular),
                &NSColor::secondaryLabelColor(),
            );
            return;
        }
        let (tops, _) = search::row_tops(&state.rows);
        let header_font = font(11.0, Weight::Semibold);
        let name_font = font(13.0, Weight::Regular);
        for (index, (row, top)) in state.rows.iter().zip(tops).enumerate() {
            match *row {
                Row::Header(section) => draw_text(
                    section.label(),
                    &search::header_frame(width, top),
                    &header_font,
                    &NSColor::secondaryLabelColor(),
                ),
                Row::Item(entry) => {
                    let (Some(entry), Some(listed)) =
                        (state.entries.get(entry), state.listed.get(entry))
                    else {
                        continue;
                    };
                    let image = listed.image.as_ref().map(|_| listed.size);
                    let frames = search::item_frames(width, top, image);
                    let selected = state.selected == Some(index);
                    if selected {
                        NSColor::selectedContentBackgroundColor().setFill();
                        rounded(&frames.row, 6.0).fill();
                    }
                    if let Some(icon) = &listed.icon {
                        draw_image(icon, &frames.icon);
                    }
                    let color = if selected {
                        NSColor::alternateSelectedControlTextColor()
                    } else {
                        NSColor::labelColor()
                    };
                    draw_text(&entry.name, &frames.name, &name_font, &color);
                    if let (Some(image), Some(at), Some(back)) =
                        (&listed.image, frames.image, frames.image_back)
                    {
                        if let Some(fill) = state.fill {
                            srgb(fill, 1.0).setFill();
                            rounded(&back, 5.0).fill();
                        }
                        draw_image(image, &at);
                    }
                }
            }
        }
    }
}

fn find_tile(model: &BarModel, id: WindowId) -> Option<&Tile> {
    Section::ALL
        .iter()
        .flat_map(|&section| model.bar(section))
        .find(|tile| tile.id == id)
}

fn rounded(r: &Rect, radius: f64) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        rect(r.x, r.y, r.width, r.height),
        radius,
        radius,
    )
}

fn draw_image(image: &NSImage, at: &Rect) {
    // SAFETY: a zero source rect draws the whole image; no hints.
    unsafe {
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            rect(at.x, at.y, at.width, at.height),
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
}

impl SearchPanel {
    /// The panel's window number.
    pub fn window_number(&self) -> isize {
        self.panel.windowNumber()
    }

    /// Whether the panel has the keyboard and the field takes the typing.
    pub fn has_keyboard(&self) -> bool {
        self.panel.isKeyWindow() && self.field.currentEditor().is_some()
    }

    /// Puts `text` in the field and filters, as typing does.
    pub fn set_query(&self, text: &str) {
        self.field.setStringValue(&NSString::from_str(text));
        self.query_changed();
    }

    /// The rows as text, top to bottom: headers in brackets and "> "
    /// before the selected item. For logs and checks.
    pub fn listed_names(&self) -> Vec<String> {
        let state = self.state.borrow();
        state
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| match *row {
                Row::Header(section) => format!("[{}]", section.label()),
                Row::Item(e) => {
                    let mark = if state.selected == Some(i) { "> " } else { "" };
                    format!("{mark}{}", state.entries[e].name)
                }
            })
            .collect()
    }
}
