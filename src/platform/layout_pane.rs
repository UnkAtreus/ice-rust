//! Menu Bar Layout pane: a snapshot of the real menu bar and the canvas
//! that shows three section bars of item tiles and tracks drags.
//!
//! Each bar is a horizontal scroll view (v1.2) whose document view, a
//! `BarStrip`, draws that section's tiles in document coordinates (see
//! `bar_scroll`). The canvas draws the labels, the bar backgrounds and the
//! message line under the scroll views, and a `DragLayer` over them draws
//! the tile being dragged. The strips pass mouse events to the canvas,
//! which converts the pointer into each bar's document coordinates for hit
//! tests, insertion indices and the marker.
//!
//! The canvas only reports drops (item, section, insertion index); the
//! Settings window turns them into moves and rebuilds the snapshot from the
//! real menu bar afterwards.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr;
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearanceCustomization, NSBezierPath, NSBorderType, NSColor, NSCompositingOperation,
    NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSLineBreakMode,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSResponder, NSScrollElasticity,
    NSScrollView, NSScrollerKnobStyle, NSScrollerStyle, NSStringDrawing, NSToolTipTag, NSView,
    NSViewToolTipOwner,
};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSDictionary, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer};

use super::capture::{self, CaptureMethod, ItemImage, SlotSource};
use super::item_names;
use super::menu_bar_space;
use super::timer;
use super::ui::{Weight, font, is_dark, rect, redraw_on_resize, srgb, tinted_symbol};
use super::windows;
use crate::agent_bar::{AgentLayout, OurFrames, agent_layout, bar_slots};
use crate::arrange::{Anchors, BarModel, Tile, bar_at, clamp_insertion, marker_x};
use crate::bar_scroll::{
    StripLayout, autoscroll_offset, document_x, insertion_at, keep_right_distance, strip_layout,
    visible_x,
};
use crate::brand;
use crate::chain::{
    DividerLookup, MSG_NO_ITEM_WINDOWS, SectionChains, find_divider_or_frame, find_titled,
    section_chains,
};
use crate::geometry::{menu_bar_display, menu_bar_window};
use crate::image_check::prefers_dark_text;
use crate::model::{
    ControlFrames, ControlNames, MENU_BAR_AGENT_OWNER, Rect, STATUS_LAYER, WindowId, WindowRecord,
};
use crate::sections::Section;

/// Space for a section label above its bar.
const LABEL_HEIGHT: f64 = 22.0;
/// Inset of the tiles from a bar's top and bottom, and of its scrolling
/// strip from the bar's ends.
const BAR_INSET: f64 = 6.0;
/// Space between one bar and the next label.
const BAR_GAP: f64 = 14.0;
/// Space between tiles, and between a strip's ends and its outer tiles.
const TILE_PADDING: f64 = 2.0;
/// Vertical slack when deciding which bar the pointer is over.
const BAR_SLACK: f64 = 12.0;
/// A drag this close to a strip's left or right edge scrolls the strip.
const AUTOSCROLL_EDGE: f64 = 24.0;
/// Most a strip scrolls per auto-scroll tick, in points.
const AUTOSCROLL_MAX_STEP: f64 = 8.0;
/// Auto-scroll timer period.
const AUTOSCROLL_TICK: Duration = Duration::from_millis(30);
/// Points per line for scroll wheels without precise deltas.
const LINE_SCROLL: f64 = 10.0;
/// SF Symbol drawn on a tile without an image.
const PLACEHOLDER_SYMBOL: &str = "questionmark.square.dashed";

/// A tile's image and its size in points. No image draws a placeholder.
pub struct TileImage {
    pub image: Option<Retained<NSImage>>,
    pub size: (f64, f64),
}

/// The real menu bar at one moment.
pub struct LayoutSnapshot {
    pub model: Option<BarModel>,
    pub images: HashMap<WindowId, TileImage>,
    pub bar_color: Option<[f64; 3]>,
    /// Whether the item glyphs are light, from the captured images
    /// (`capture::glyph_tone`); `None` without images to go by.
    pub light_glyphs: Option<bool>,
    pub menu_bar_height: f64,
    /// Why the bars are empty, when they are.
    pub notice: Option<String>,
    /// Capture outcome per section: tiles with an image, placeholders, and
    /// whether the capture timed out.
    pub captures: Vec<(Section, usize, usize, bool)>,
    /// Items whose capture timed out or found the capture gate busy.
    pub stale: HashSet<WindowId>,
    /// The color sample timed out or found the capture gate busy.
    pub color_stale: bool,
}

impl LayoutSnapshot {
    pub fn empty(notice: &str) -> Self {
        Self {
            model: None,
            images: HashMap::new(),
            bar_color: None,
            light_glyphs: None,
            menu_bar_height: 24.0,
            notice: Some(notice.to_string()),
            captures: Vec::new(),
            stale: HashSet::new(),
            color_stale: false,
        }
    }

    pub fn timed_out(&self) -> bool {
        self.captures.iter().any(|c| c.3)
    }

    /// Fills images that timed out with the ones `old` has for the same
    /// windows, and the color likewise. A window id names one item window
    /// for its whole life, so the earlier image still shows that item.
    pub fn reuse_stale(&mut self, old: &LayoutSnapshot) {
        for id in &self.stale {
            if let Some(previous) = old.images.get(id)
                && previous.image.is_some()
            {
                self.images.insert(
                    *id,
                    TileImage {
                        image: previous.image.clone(),
                        size: previous.size,
                    },
                );
            }
        }
        if self.color_stale && self.bar_color.is_none() {
            self.bar_color = old.bar_color;
        }
        if !self.stale.is_empty() && self.light_glyphs.is_none() {
            self.light_glyphs = old.light_glyphs;
        }
    }
}

fn tile_image(window: &WindowRecord, captured: &ItemImage) -> TileImage {
    let usable = captured.image.as_deref().filter(|_| !captured.transparent);
    match usable {
        Some(cg) => {
            let w = CGImage::width(Some(cg)) as f64 / captured.scale;
            let h = CGImage::height(Some(cg)) as f64 / captured.scale;
            let image = NSImage::initWithCGImage_size(NSImage::alloc(), cg, NSSize::new(w, h));
            TileImage {
                image: Some(image),
                size: (w, h),
            }
        }
        None => TileImage {
            image: None,
            size: (window.bounds.width, window.bounds.height),
        },
    }
}

/// Reads the menu bar through the control items named by `names`:
/// window list, three sections, item images and the menu bar color.
/// `frames` (CG space, from AppKit) find our own control item windows when
/// their titles do not; offscreen renders of another app pass none.
///
/// Every capture goes through one gate, and a capture that times out keeps
/// it closed for the ones after it. So the captures run in the order most
/// likely to succeed: the off-screen hidden and always-hidden items first,
/// then the on-screen visible items and the color sample, which timed out
/// in `render-settings` on the development host.
/// One read of the window list turned into bars, without images.
struct ModelRead {
    all: Vec<WindowRecord>,
    hidden: WindowRecord,
    icon: Option<WindowRecord>,
    chains: SectionChains,
    model: BarModel,
}

/// Reads the bars from the window list, or says why they are empty.
fn read_model(names: &ControlNames, frames: &ControlFrames) -> Result<ModelRead, String> {
    let all = windows::list_windows();
    let hidden =
        match find_divider_or_frame(&all, &names.hidden, &names.icon, frames.hidden.as_ref()) {
            DividerLookup::Found(d) => d,
            DividerLookup::NotFound => {
                return Err(format!(
                    "{} is not in the menu bar. Quit and reopen ice-rust",
                    names.hidden
                ));
            }
            DividerLookup::TitlesUnreadable => {
                return Err("Screen Recording permission needed".to_string());
            }
            DividerLookup::NoItemWindows => return Err(MSG_NO_ITEM_WINDOWS.to_string()),
        };
    let always = match find_divider_or_frame(
        &all,
        &names.always_hidden,
        &names.icon,
        frames.always_hidden.as_ref(),
    ) {
        DividerLookup::Found(d) => Some(d),
        _ => None,
    };
    let icon = find_titled(&all, &names.icon).cloned();
    let chains = section_chains(&all, &hidden, always.as_ref());
    let anchors = Anchors {
        icon: icon.as_ref().map(|w| w.id),
        hidden_divider: hidden.id,
        always_hidden_divider: always.as_ref().map(|d| d.id),
    };
    let model = BarModel::from_sections(&chains, names, anchors);
    Ok(ModelRead {
        all,
        hidden,
        icon,
        chains,
        model,
    })
}

/// Only the order of the bars, without capturing any image: cheap enough
/// to poll while the menu bar settles after a move (v1.7).
pub fn load_order(names: &ControlNames, frames: &ControlFrames) -> Option<BarModel> {
    read_model(names, frames).ok().map(|read| read.model)
}

pub fn load_snapshot(names: &ControlNames, frames: &ControlFrames) -> LayoutSnapshot {
    let ModelRead {
        all,
        hidden,
        icon,
        chains,
        model,
    } = match read_model(names, frames) {
        Ok(read) => read,
        Err(notice) => return LayoutSnapshot::empty(&notice),
    };
    item_names::look_up(&tile_bounds(&model));

    let displays = windows::displays();
    let icon_rect = icon.as_ref().map(|w| w.bounds).or(frames.icon);
    let place = menu_bar_display(
        &all,
        &displays,
        icon_rect.as_ref(),
        Some(&hidden.bounds),
        None,
    );
    let menu_bar_height = place.map_or(hidden.bounds.height, |p| p.menu_bar_height);

    let mut images = HashMap::new();
    let mut captures = Vec::new();
    let mut stale = HashSet::new();
    let mut glyph_images = Vec::new();
    for (section, chain) in [
        (Section::Hidden, &chains.hidden),
        (Section::AlwaysHidden, &chains.always_hidden),
        (Section::Visible, &chains.visible),
    ] {
        let tiles = model.ids(section);
        let records: Vec<WindowRecord> = chain
            .iter()
            .filter(|w| tiles.contains(&w.id))
            .cloned()
            .collect();
        if records.is_empty() {
            continue;
        }
        let result = capture::capture_items_bounded(&records);
        let shots = capture::images_or_placeholders(&records, result);
        glyph_images.extend(
            shots
                .iter()
                .filter(|s| !s.transparent)
                .filter_map(|s| s.image.clone()),
        );
        let timed_out = shots.iter().any(|s| s.method == CaptureMethod::TimedOut);
        let mut with_image = 0;
        for (window, shot) in records.iter().zip(&shots) {
            let tile = tile_image(window, shot);
            if tile.image.is_some() {
                with_image += 1;
            }
            if shot.method == CaptureMethod::TimedOut {
                stale.insert(window.id);
            }
            images.insert(window.id, tile);
        }
        captures.push((section, with_image, records.len() - with_image, timed_out));
    }

    let (bar_color, color_stale) = match place {
        Some(p) => match capture::menu_bar_color(&all, &p.display.bounds, p.menu_bar_height) {
            Ok(color) => (color, false),
            Err(_) => (None, true),
        },
        None => (None, false),
    };

    LayoutSnapshot {
        model: Some(model),
        images,
        bar_color,
        light_glyphs: capture::glyph_tone(glyph_images.iter().map(|image| &**image)),
        menu_bar_height,
        notice: None,
        captures,
        stale,
        color_stale,
    }
}

/// The macOS 27 menu bar on `display` split into sections (v1.8), or why
/// it cannot be.
pub fn read_agent(ours: &OurFrames, display: &Rect) -> Result<AgentLayout, String> {
    let windows = menu_bar_space::agent_windows()
        .ok_or("Accessibility permission needed to read the menu bar")?;
    let own = std::process::id() as i32;
    let slots = bar_slots(&windows, own, display);
    let layout = agent_layout(&slots, own, ours)
        .ok_or_else(|| "The hidden divider is not in the menu bar".to_string())?;
    item_names::note_owners(&layout.owners);
    Ok(layout)
}

/// Every tile with its bounds, for `item_names::look_up`.
fn tile_bounds(model: &BarModel) -> Vec<(WindowId, Rect)> {
    Section::ALL
        .iter()
        .flat_map(|&section| model.bar(section))
        .map(|tile| (tile.id, tile.bounds))
        .collect()
}

/// The name a tile shows: mostly its app's once known (`item_names`).
/// Apps choose names such as "Item-0" or a UUID for their items.
fn tile_name(model: &BarModel, tile: &Tile) -> String {
    item_names::name(tile.id, &model.display_name(tile))
}

/// `load_order` for the macOS 27 menu bar.
pub fn load_agent_order(
    names: &ControlNames,
    ours: &OurFrames,
    display: &Rect,
) -> Option<BarModel> {
    read_agent(ours, display)
        .ok()
        .map(|layout| layout.model(names))
}

/// `load_snapshot` for the macOS 27 menu bar: the bars from MenuBarAgent's
/// slots, each item cropped from MenuBarAgent's menu bar window, the
/// Window Server's menu bar window or the screen, whichever draws it.
pub fn load_agent_snapshot(
    names: &ControlNames,
    ours: &OurFrames,
    display: &Rect,
    menu_bar_height: f64,
) -> LayoutSnapshot {
    let layout = match read_agent(ours, display) {
        Ok(layout) => layout,
        Err(notice) => return LayoutSnapshot::empty(&notice),
    };
    if layout.folded > 0 {
        eprintln!(
            "ice-rust: {} items are folded into the system overflow and not shown in Menu Bar Layout",
            layout.folded
        );
    }
    let model = layout.model(names);
    let all = windows::list_windows();
    let agent_window = all.iter().find(|w| {
        w.owner_name.as_deref() == Some(MENU_BAR_AGENT_OWNER)
            && (w.bounds.x - display.x).abs() <= 1.0
            && (w.bounds.y - display.y).abs() <= 1.0
    });
    let region = agent_window.map_or(
        Rect::new(display.x, display.y, display.width, menu_bar_height),
        |w| w.bounds,
    );
    let mut sources: Vec<SlotSource> = agent_window
        .map(|w| SlotSource::Window(w.id))
        .into_iter()
        .collect();
    if let Some(menubar) = menu_bar_window(&all, display) {
        sources.push(SlotSource::Window(menubar.id));
    }
    sources.push(SlotSource::Screen);

    let mut images = HashMap::new();
    let mut captures = Vec::new();
    let mut stale = HashSet::new();
    let mut glyph_images = Vec::new();
    for section in [Section::Hidden, Section::AlwaysHidden, Section::Visible] {
        let ids = model.ids(section);
        let rects: Vec<Rect> = ids.iter().filter_map(|&id| layout.frame(id)).collect();
        if rects.len() != ids.len() || ids.is_empty() {
            continue;
        }
        let result = capture::capture_slots_bounded(sources.clone(), region, rects.clone());
        let timed_out = result.is_err();
        let shots = result.unwrap_or_else(|_| rects.iter().map(|_| None).collect());
        let mut with_image = 0;
        for ((&id, rect), shot) in ids.iter().zip(&rects).zip(shots) {
            let record = WindowRecord {
                id,
                layer: STATUS_LAYER,
                bounds: *rect,
                title: None,
                owner_name: None,
                on_screen: true,
            };
            let item = match shot {
                Some(image) => {
                    glyph_images.push(image.clone());
                    let scale = CGImage::width(Some(&image)) as f64 / rect.width.max(1.0);
                    ItemImage {
                        id,
                        image: Some(image),
                        method: CaptureMethod::Composite,
                        transparent: false,
                        scale,
                    }
                }
                None if timed_out => ItemImage::missing(id, CaptureMethod::TimedOut),
                None => ItemImage::missing(id, CaptureMethod::Failed),
            };
            if timed_out {
                stale.insert(id);
            }
            let tile = tile_image(&record, &item);
            if tile.image.is_some() {
                with_image += 1;
            }
            images.insert(id, tile);
        }
        captures.push((section, with_image, ids.len() - with_image, timed_out));
    }
    let (bar_color, color_stale) = match capture::menu_bar_color(&all, display, menu_bar_height) {
        Ok(color) => (color, false),
        Err(_) => (None, true),
    };
    LayoutSnapshot {
        model: Some(model),
        images,
        bar_color,
        light_glyphs: capture::glyph_tone(glyph_images.iter().map(|image| &**image)),
        menu_bar_height,
        notice: None,
        captures,
        stale,
        color_stale,
    }
}

/// Called with the dragged item, the section it was dropped on, and the
/// insertion index in that bar counted without the item.
pub type DropHandler = Box<dyn Fn(WindowId, Section, usize)>;

struct Drag {
    id: WindowId,
    /// Pointer offset inside the tile at mouse down.
    grab: (f64, f64),
    /// Canvas coordinates.
    pointer: (f64, f64),
    over: Option<(Section, usize)>,
}

pub struct CanvasState {
    snapshot: LayoutSnapshot,
    drag: Option<Drag>,
    busy: bool,
    /// Outcome of the last press or drop; stays until the next press.
    message: Option<(String, bool)>,
    /// Refresh status, shown while there is no message.
    note: Option<String>,
    /// Each bar's strip layout in `Section::ALL` order, in document
    /// coordinates.
    strips: [StripLayout; 3],
    on_drop: Option<DropHandler>,
}

/// One bar's scroll view and the strip it scrolls.
struct BarViews {
    scroll: Retained<BarScrollView>,
    strip: Retained<BarStrip>,
}

struct CanvasViews {
    /// In `Section::ALL` order.
    bars: Vec<BarViews>,
    overlay: Retained<DragLayer>,
}

pub struct CanvasIvars {
    state: RefCell<CanvasState>,
    views: OnceCell<CanvasViews>,
    autoscroll: RefCell<Option<Retained<NSTimer>>>,
}

/// One bar's geometry in canvas coordinates.
struct BarGeometry {
    section: Section,
    label: Rect,
    bar: Rect,
    /// The scroll view's frame: the bar without its rounded ends.
    strip: Rect,
}

/// The tile under a mouse down.
struct Hit {
    id: WindowId,
    locked: bool,
    name: String,
    /// Tile origin in canvas coordinates.
    origin: (f64, f64),
}

pub struct StripIvars {
    section: Section,
    canvas: Weak<LayoutCanvas>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `BarStrip::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = StripIvars]
    pub struct BarStrip;

    // SAFETY: argument and return types match NSView's declarations:
    // isFlipped and mouseDownCanMoveWindow -> BOOL, acceptsFirstMouse:
    // (NSEvent *) -> BOOL, drawRect: (NSRect) -> void, mouse and scroll
    // handlers (NSEvent *) -> void.
    impl BarStrip {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if let Some(canvas) = self.ivars().canvas.load() {
                canvas.draw_strip(self.ivars().section, self.bounds().size.height);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(canvas) = self.ivars().canvas.load() {
                canvas.press(canvas.local(event));
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if let Some(canvas) = self.ivars().canvas.load() {
                canvas.track(canvas.local(event));
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if let Some(canvas) = self.ivars().canvas.load() {
                canvas.release(canvas.local(event));
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let handled = self
                .ivars()
                .canvas
                .load()
                .is_some_and(|canvas| canvas.wheel(self.ivars().section, event));
            if !handled {
                // SAFETY: scrollWheel: takes an NSEvent and returns void;
                // NSView's implementation passes it to the scroll view.
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }
    }

    // SAFETY: the protocol's methods come from NSObject; none is overridden.
    unsafe impl NSObjectProtocol for BarStrip {}

    // SAFETY: view:stringForToolTip:point:userData: takes an NSView, an
    // NSToolTipTag, an NSPoint and a void pointer and returns an NSString,
    // as NSViewToolTipOwner declares. The user data is never read.
    unsafe impl NSViewToolTipOwner for BarStrip {
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn tool_tip(
            &self,
            _view: &NSView,
            _tag: NSToolTipTag,
            point: NSPoint,
            _data: *mut c_void,
        ) -> Retained<NSString> {
            let name = self
                .ivars()
                .canvas
                .load()
                .and_then(|canvas| canvas.name_in_strip(self.ivars().section, (point.x, point.y)));
            NSString::from_str(&name.unwrap_or_default())
        }
    }
);

impl BarStrip {
    fn new(mtm: MainThreadMarker, section: Section, canvas: Weak<LayoutCanvas>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(StripIvars { section, canvas });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    // SAFETY: NSScrollView has no subclassing requirements beyond using its
    // designated initializer, which `BarScrollView::new` does. No Drop impl.
    #[unsafe(super(NSScrollView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct BarScrollView;

    // SAFETY: setScrollerStyle: takes an NSScrollerStyle (NSInteger) and
    // returns void, as NSScrollView declares.
    impl BarScrollView {
        /// AppKit sends every scroll view the new preferred style when it
        /// changes, for example when a mouse connects while System Settings
        /// shows scroll bars automatically. A legacy scroller would take its
        /// height from the bar, so a bar always keeps the overlay style.
        #[unsafe(method(setScrollerStyle:))]
        fn set_scroller_style(&self, _style: NSScrollerStyle) {
            // SAFETY: as above; NSScrollView's implementation applies the
            // style to the scroll view and its scrollers.
            let _: () =
                unsafe { msg_send![super(self), setScrollerStyle: NSScrollerStyle::Overlay] };
        }
    }
);

impl BarScrollView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: initWithFrame: is NSScrollView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `DragLayer::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<LayoutCanvas>]
    pub struct DragLayer;

    // SAFETY: isFlipped returns BOOL, hitTest: takes an NSPoint and returns
    // an NSView or nil, drawRect: takes an NSRect, as NSView declares.
    impl DragLayer {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Never the target of a click: mouse events reach the bars below.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if let Some(canvas) = self.ivars().load() {
                canvas.draw_drag();
            }
        }
    }
);

impl DragLayer {
    fn new(mtm: MainThreadMarker, frame: NSRect, canvas: Weak<LayoutCanvas>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(canvas);
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `LayoutCanvas::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CanvasIvars]
    pub struct LayoutCanvas;

    // SAFETY: argument and return types match NSView's declarations:
    // isFlipped and mouseDownCanMoveWindow -> BOOL, acceptsFirstMouse:
    // (NSEvent *) -> BOOL, drawRect: (NSRect) -> void, mouse handlers
    // (NSEvent *) -> void, resizeSubviewsWithOldSize: (NSSize) -> void.
    impl LayoutCanvas {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(self.local(event));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.track(self.local(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.release(self.local(event));
        }

        /// The window resized: the bars stretch with the canvas.
        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, old_size: NSSize) {
            // SAFETY: resizeSubviewsWithOldSize: takes an NSSize and returns
            // void; NSView's implementation applies the autoresizing masks.
            let _: () = unsafe { msg_send![super(self), resizeSubviewsWithOldSize: old_size] };
            self.relayout();
        }
    }
);

fn text_attributes(
    font: &NSFont,
    color: &NSColor,
    line_break: NSLineBreakMode,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: the keys are immutable NSString constants exported by AppKit.
    let (font_key, color_key, paragraph_key) = unsafe {
        (
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        )
    };
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineBreakMode(line_break);
    let values: [&AnyObject; 3] = [font.as_ref(), color.as_ref(), paragraph.as_ref()];
    NSDictionary::from_slices(&[font_key, color_key, paragraph_key], &values)
}

/// Draws one line of text in `frame`, cut off with an ellipsis when it
/// does not fit.
fn draw_text(text: &str, frame: &Rect, font: &NSFont, color: &NSColor) {
    let attributes = text_attributes(font, color, NSLineBreakMode::ByTruncatingTail);
    // SAFETY: the attributes map NSAttributedStringKey values to an NSFont,
    // an NSColor and an NSParagraphStyle, the types AppKit expects.
    unsafe {
        NSString::from_str(text).drawInRect_withAttributes(ns(frame), Some(&attributes));
    }
}

/// Draws text in `frame`, wrapped at word boundaries; lines below the frame
/// are cut off.
fn draw_wrapped_text(text: &str, frame: &Rect, font: &NSFont, color: &NSColor) {
    let attributes = text_attributes(font, color, NSLineBreakMode::ByWordWrapping);
    // SAFETY: as in `draw_text`.
    unsafe {
        NSString::from_str(text).drawInRect_withAttributes(ns(frame), Some(&attributes));
    }
}

fn text_width(text: &str, font: &NSFont) -> f64 {
    let attributes = text_attributes(
        font,
        &NSColor::labelColor(),
        NSLineBreakMode::ByTruncatingTail,
    );
    // SAFETY: as in `draw_text`.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attributes)) }.width
}

fn ns(r: &Rect) -> NSRect {
    rect(r.x, r.y, r.width, r.height)
}

fn section_index(section: Section) -> usize {
    Section::ALL
        .iter()
        .position(|&s| s == section)
        .unwrap_or_default()
}

/// Tile sizes in points of one bar, left to right.
fn tile_sizes(snapshot: &LayoutSnapshot, section: Section) -> Vec<(f64, f64)> {
    snapshot
        .model
        .as_ref()
        .map(|m| {
            m.bar(section)
                .iter()
                .map(|t| {
                    snapshot
                        .images
                        .get(&t.id)
                        .map_or((t.bounds.width, t.bounds.height), |i| i.size)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn offset_of(bar: &BarViews) -> f64 {
    bar.scroll.contentView().bounds().origin.x
}

/// Scroller knob for a bar of the sampled `bar_color`: dark on a light bar,
/// light on a dark one. Without a sample the bar takes the fallback fill of
/// the window's appearance, and the default knob follows that appearance.
fn knob_style(bar_color: Option<[f64; 3]>) -> NSScrollerKnobStyle {
    match bar_color {
        Some(color) if prefers_dark_text(color) => NSScrollerKnobStyle::Dark,
        Some(_) => NSScrollerKnobStyle::Light,
        None => NSScrollerKnobStyle::Default,
    }
}

impl LayoutCanvas {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, snapshot: LayoutSnapshot) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasIvars {
            state: RefCell::new(CanvasState {
                snapshot,
                drag: None,
                busy: false,
                message: None,
                note: None,
                strips: Default::default(),
                on_drop: None,
            }),
            views: OnceCell::new(),
            autoscroll: RefCell::new(None),
        });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        let canvas: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        redraw_on_resize(&canvas);
        let weak = Weak::from_retained(&canvas);
        let bars = Section::ALL
            .iter()
            .map(|&section| {
                let scroll = BarScrollView::new(mtm);
                scroll.setHasHorizontalScroller(true);
                scroll.setHasVerticalScroller(false);
                scroll.setAutohidesScrollers(true);
                scroll.setScrollerStyle(NSScrollerStyle::Overlay);
                scroll.setBorderType(NSBorderType::NoBorder);
                scroll.setVerticalScrollElasticity(NSScrollElasticity::None);
                // Transparent, so the bar color drawn by the canvas shows.
                scroll.setDrawsBackground(false);
                scroll.contentView().setDrawsBackground(false);
                let strip = BarStrip::new(mtm, section, weak.clone());
                scroll.setDocumentView(Some(&strip));
                canvas.addSubview(&scroll);
                BarViews { scroll, strip }
            })
            .collect();
        let overlay = DragLayer::new(mtm, canvas.bounds(), weak);
        canvas.addSubview(&overlay);
        if canvas
            .ivars()
            .views
            .set(CanvasViews { bars, overlay })
            .is_err()
        {
            eprintln!("ice-rust: layout bars were set up twice");
        }
        canvas.relayout();
        canvas
    }

    pub fn set_on_drop(&self, handler: DropHandler) {
        self.ivars().state.borrow_mut().on_drop = Some(handler);
    }

    pub fn set_snapshot(&self, snapshot: LayoutSnapshot) {
        {
            let mut state = self.ivars().state.borrow_mut();
            state.snapshot = snapshot;
            state.drag = None;
        }
        self.stop_autoscroll();
        self.relayout();
        self.redraw();
    }

    pub fn with_snapshot<R>(&self, f: impl FnOnce(&LayoutSnapshot) -> R) -> R {
        f(&self.ivars().state.borrow().snapshot)
    }

    /// Shows `model` in place of the current bars, keeping the images, so
    /// a drop shows at once while the real item moves (v1.7).
    pub fn show_model(&self, model: BarModel) {
        self.ivars().state.borrow_mut().snapshot.model = Some(model);
        self.relayout();
        self.redraw();
    }

    /// Marks moves in flight. The bars stay as they are and keep taking
    /// drags, which queue behind the running move; the periodic refresh
    /// waits (`is_idle`).
    pub fn set_busy(&self, busy: bool) {
        self.ivars().state.borrow_mut().busy = busy;
    }

    pub fn is_idle(&self) -> bool {
        let state = self.ivars().state.borrow();
        !state.busy && state.drag.is_none()
    }

    /// Shows `text` under the bars until the next press; `error` draws it
    /// in red.
    pub fn set_message(&self, text: Option<String>, error: bool) {
        self.ivars().state.borrow_mut().message = text.map(|t| (t, error));
        self.setNeedsDisplay(true);
    }

    /// Shows `text` under the bars while there is no message.
    pub fn set_note(&self, text: Option<String>) {
        self.ivars().state.borrow_mut().note = text;
        self.setNeedsDisplay(true);
    }

    /// Removes the tooltip rectangles, whose owners are the strips.
    pub fn clear_tool_tips(&self) {
        for bar in self.bars() {
            bar.strip.removeAllToolTips();
        }
    }

    /// Each bar's scroll offset and the offset that shows its right end, in
    /// `Section::ALL` order.
    pub fn scroll_positions(&self) -> Vec<(Section, f64, f64)> {
        let state = self.ivars().state.borrow();
        Section::ALL
            .iter()
            .zip(self.bars())
            .zip(&state.strips)
            .map(|((&section, bar), strip)| (section, offset_of(bar), strip.max_offset))
            .collect()
    }

    fn bars(&self) -> &[BarViews] {
        self.ivars().views.get().map_or(&[], |v| v.bars.as_slice())
    }

    fn local(&self, event: &NSEvent) -> (f64, f64) {
        let p: NSPoint = self.convertPoint_fromView(event.locationInWindow(), None);
        (p.x, p.y)
    }

    /// The bars, top to bottom in `Section::ALL` order, for a canvas
    /// `width` wide.
    fn geometry(width: f64, menu_bar_height: f64) -> Vec<BarGeometry> {
        let bar_height = menu_bar_height + 2.0 * BAR_INSET;
        let mut y = 0.0;
        Section::ALL
            .iter()
            .map(|&section| {
                let label = Rect::new(2.0, y, width - 4.0, LABEL_HEIGHT);
                let bar = Rect::new(0.0, y + LABEL_HEIGHT, width, bar_height);
                y = bar.max_y() + BAR_GAP;
                let strip = Rect::new(
                    bar.x + BAR_INSET,
                    bar.y,
                    (bar.width - 2.0 * BAR_INSET).max(0.0),
                    bar.height,
                );
                BarGeometry {
                    section,
                    label,
                    bar,
                    strip,
                }
            })
            .collect()
    }

    fn current_geometry(&self, state: &CanvasState) -> Vec<BarGeometry> {
        Self::geometry(self.bounds().size.width, state.snapshot.menu_bar_height)
    }

    /// Height the canvas needs for the bars and the message line.
    pub fn content_height(menu_bar_height: f64) -> f64 {
        3.0 * (LABEL_HEIGHT + menu_bar_height + 2.0 * BAR_INSET + BAR_GAP) + 34.0
    }

    /// Places the scroll views over the bars and lays out each strip for
    /// the current snapshot and width. A bar keeps its distance from its
    /// right end, so a bar shows its right end until the user scrolls it.
    fn relayout(&self) {
        let Some(views) = self.ivars().views.get() else {
            return;
        };
        let (geometry, sizes, knob) = {
            let state = self.ivars().state.borrow();
            let sizes: Vec<Vec<(f64, f64)>> = Section::ALL
                .iter()
                .map(|&section| tile_sizes(&state.snapshot, section))
                .collect();
            (
                self.current_geometry(&state),
                sizes,
                knob_style(Some(self.bar_fill(&state))),
            )
        };
        for (index, ((g, bar), sizes)) in geometry.iter().zip(&views.bars).zip(&sizes).enumerate() {
            // Read before the frame changes: a clip view that grows clamps
            // its origin against the old document width, and that clamp
            // would count as a scroll away from the right end.
            let previous = offset_of(bar);
            bar.scroll.setFrame(ns(&g.strip));
            bar.scroll.setScrollerKnobStyle(knob);
            // The frame's height, not the clip view's, so a scroller can
            // never shrink the tiles.
            let visible_width = bar.scroll.contentSize().width;
            let layout = strip_layout(
                sizes,
                visible_width,
                g.strip.height,
                BAR_INSET,
                TILE_PADDING,
            );
            let old_max = self.ivars().state.borrow().strips[index].max_offset;
            let offset = keep_right_distance(previous, old_max, layout.max_offset);
            // A bar that fits does not bounce sideways.
            bar.scroll
                .setHorizontalScrollElasticity(if layout.max_offset > 0.0 {
                    NSScrollElasticity::Automatic
                } else {
                    NSScrollElasticity::None
                });
            bar.strip
                .setFrameSize(NSSize::new(layout.document_width, g.strip.height));
            self.ivars().state.borrow_mut().strips[index] = layout;
            self.scroll_bar(index, offset);
            bar.strip.setNeedsDisplay(true);
        }
        views.overlay.setFrame(self.bounds());
        self.update_tool_tips();
    }

    /// Scrolls bar `index` so `offset` is at its visible left edge.
    fn scroll_bar(&self, index: usize, offset: f64) {
        if let Some(bar) = self.bars().get(index) {
            let clip = bar.scroll.contentView();
            clip.scrollToPoint(NSPoint::new(offset, 0.0));
            bar.scroll.reflectScrolledClipView(&clip);
        }
    }

    fn redraw(&self) {
        self.setNeedsDisplay(true);
        if let Some(views) = self.ivars().views.get() {
            for bar in &views.bars {
                bar.strip.setNeedsDisplay(true);
            }
            views.overlay.setNeedsDisplay(true);
        }
    }

    /// Slots of `section` with the dragged tile taken out, paired with
    /// their index in the bar counted without it, in document coordinates.
    fn slots_without(
        state: &CanvasState,
        section: Section,
        dragged: WindowId,
    ) -> Vec<(usize, Rect)> {
        let Some(model) = &state.snapshot.model else {
            return Vec::new();
        };
        let tiles = model.bar(section);
        let dragged_at = tiles.iter().position(|t| t.id == dragged);
        state.strips[section_index(section)]
            .layout
            .slots
            .iter()
            .filter(|s| tiles.get(s.index).is_some_and(|t| t.id != dragged))
            .map(|s| {
                let index = match dragged_at {
                    Some(d) if s.index > d => s.index - 1,
                    _ => s.index,
                };
                (index, s.frame)
            })
            .collect()
    }

    /// The bars' fill: the logo's blue or its light tint, whichever
    /// contrasts with the item glyphs (`brand::bar_fill_for`, issue #3).
    fn bar_fill(&self, state: &CanvasState) -> [f64; 3] {
        brand::bar_fill_for(
            state.snapshot.light_glyphs,
            state.snapshot.bar_color,
            is_dark(&self.effectiveAppearance()),
        )
    }

    /// The bar fill and the color of text and placeholders on it.
    fn bar_colors(&self, state: &CanvasState) -> (Retained<NSColor>, Retained<NSColor>) {
        let color = self.bar_fill(state);
        let on_bar = if prefers_dark_text(color) {
            NSColor::blackColor().colorWithAlphaComponent(0.7)
        } else {
            NSColor::whiteColor().colorWithAlphaComponent(0.8)
        };
        (srgb(color, 1.0), on_bar)
    }

    /// Labels, bar backgrounds, empty-bar notes and the message line. The
    /// tiles are drawn by the strips on top.
    fn draw(&self) {
        let state = self.ivars().state.borrow();
        let geometry = self.current_geometry(&state);
        let (fill, on_bar) = self.bar_colors(&state);
        let label_font = font(12.0, Weight::Semibold);
        let body = font(12.0, Weight::Regular);

        for g in &geometry {
            draw_text(
                g.section.label(),
                &Rect::new(g.label.x, g.label.y + 3.0, g.label.width, 16.0),
                &label_font,
                &NSColor::secondaryLabelColor(),
            );
            let radius = (g.bar.height / 4.0).min(10.0);
            let path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(&g.bar), radius, radius);
            fill.setFill();
            path.fill();
            NSColor::separatorColor().setStroke();
            path.setLineWidth(1.0);
            path.stroke();

            let empty = state
                .snapshot
                .model
                .as_ref()
                .is_none_or(|m| m.bar(g.section).is_empty());
            if empty {
                let text = match (&state.snapshot.notice, &state.snapshot.model) {
                    (Some(notice), _) => notice.clone(),
                    (None, Some(m))
                        if g.section == Section::AlwaysHidden
                            && m.anchors.always_hidden_divider.is_none() =>
                    {
                        "Always-hidden divider not found".to_string()
                    }
                    _ => "No items".to_string(),
                };
                let w = text_width(&text, &body).min(g.bar.width - 16.0);
                draw_text(
                    &text,
                    &Rect::new(g.bar.mid_x() - w / 2.0, g.bar.mid_y() - 8.0, w + 2.0, 16.0),
                    &body,
                    &on_bar,
                );
            }
        }

        let line = match (&state.message, &state.note) {
            (Some((text, error)), _) => Some((text.as_str(), *error)),
            (None, Some(note)) => Some((note.as_str(), false)),
            (None, None) => None,
        };
        if let Some(last) = geometry.last()
            && let Some((text, error)) = line
        {
            let color = if error {
                NSColor::systemRedColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            // Up to three lines; a long error, such as not enough room in
            // the menu bar, wraps.
            draw_wrapped_text(
                text,
                &Rect::new(2.0, last.bar.max_y() + 8.0, last.bar.width - 4.0, 54.0),
                &body,
                &color,
            );
        }
    }

    /// One bar's tiles and, while a drag is over it, the insertion marker,
    /// in the strip's document coordinates.
    fn draw_strip(&self, section: Section, height: f64) {
        let state = self.ivars().state.borrow();
        let Some(model) = &state.snapshot.model else {
            return;
        };
        let (_, on_bar) = self.bar_colors(&state);
        let small = font(10.0, Weight::Regular);
        let tiles = model.bar(section);
        let strip = &state.strips[section_index(section)];
        let dragged = state.drag.as_ref().map(|d| d.id);
        for slot in &strip.layout.slots {
            let Some(tile) = tiles.get(slot.index) else {
                continue;
            };
            if Some(tile.id) == dragged {
                continue;
            }
            let image = state.snapshot.images.get(&tile.id);
            draw_tile(image, &tile.title, &slot.frame, 1.0, &small, &on_bar);
        }

        if let Some(drag) = &state.drag
            && let Some((over, index)) = drag.over
            && over == section
        {
            let slots = Self::slots_without(&state, section, drag.id);
            let document = Rect::new(0.0, 0.0, strip.document_width, height);
            let x = marker_x(&slots, index, &document, TILE_PADDING);
            let marker = Rect::new(x - 1.0, 4.0, 2.0, (height - 8.0).max(0.0));
            NSColor::controlAccentColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(&marker), 1.0, 1.0).fill();
        }
    }

    /// The dragged tile under the pointer, in canvas coordinates.
    fn draw_drag(&self) {
        let state = self.ivars().state.borrow();
        let Some(drag) = &state.drag else {
            return;
        };
        let Some(tile) = state.snapshot.model.as_ref().and_then(|m| m.tile(drag.id)) else {
            return;
        };
        let (fill, on_bar) = self.bar_colors(&state);
        let image = state.snapshot.images.get(&drag.id);
        let size = image.map_or((tile.bounds.width, tile.bounds.height), |i| i.size);
        let frame = Rect::new(
            drag.pointer.0 - drag.grab.0,
            drag.pointer.1 - drag.grab.1,
            size.0,
            size.1,
        );
        let backing = Rect::new(
            frame.x - 2.0,
            frame.y - 2.0,
            frame.width + 4.0,
            frame.height + 4.0,
        );
        fill.colorWithAlphaComponent(0.85).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(&backing), 5.0, 5.0).fill();
        let small = font(10.0, Weight::Regular);
        draw_tile(image, &tile.title, &frame, 0.9, &small, &on_bar);
    }

    /// The tile under `point` (canvas coordinates), looked up in the
    /// document coordinates of the bar it is over.
    fn hit(&self, point: (f64, f64)) -> Option<Hit> {
        let state = self.ivars().state.borrow();
        let model = state.snapshot.model.as_ref()?;
        let geometry = self.current_geometry(&state);
        let index = geometry
            .iter()
            .position(|g| g.strip.contains(point.0, point.1))?;
        let g = geometry.get(index)?;
        let offset = offset_of(self.bars().get(index)?);
        let x = document_x(point.0 - g.strip.x, offset);
        let y = point.1 - g.strip.y;
        let slot = state.strips[index]
            .layout
            .slots
            .iter()
            .find(|s| s.frame.contains(x, y))?;
        let tile = model.bar(g.section).get(slot.index)?;
        Some(Hit {
            id: tile.id,
            locked: tile.locked,
            name: tile_name(model, tile),
            origin: (
                g.strip.x + visible_x(slot.frame.x, offset),
                g.strip.y + slot.frame.y,
            ),
        })
    }

    /// The name of the tile at `point` in the strip of `section`, for its
    /// tooltip.
    fn name_in_strip(&self, section: Section, point: (f64, f64)) -> Option<String> {
        let state = self.ivars().state.borrow();
        let model = state.snapshot.model.as_ref()?;
        let slot = state.strips[section_index(section)]
            .layout
            .slots
            .iter()
            .find(|s| s.frame.contains(point.0, point.1))?;
        let tile = model.bar(section).get(slot.index)?;
        Some(tile_name(model, tile))
    }

    /// One tooltip rectangle per tile, owned by its strip. The rectangles
    /// are in document coordinates, so they scroll with the tiles.
    fn update_tool_tips(&self) {
        for (index, bar) in self.bars().iter().enumerate() {
            bar.strip.removeAllToolTips();
            let frames: Vec<Rect> = self.ivars().state.borrow().strips[index]
                .layout
                .slots
                .iter()
                .map(|s| s.frame)
                .collect();
            let owner: &AnyObject = &bar.strip;
            for frame in frames {
                // SAFETY: the owner is the strip, which implements
                // NSViewToolTipOwner. The user data is null and never read.
                // `clear_tool_tips` removes the rectangles when the window
                // closes.
                unsafe {
                    bar.strip
                        .addToolTipRect_owner_userData(ns(&frame), owner, ptr::null_mut());
                }
            }
        }
    }

    /// A mouse wheel without precise deltas over a bar that scrolls turns
    /// its vertical steps into horizontal scrolling. Returns false to let
    /// the scroll view handle the event.
    fn wheel(&self, section: Section, event: &NSEvent) -> bool {
        if event.hasPreciseScrollingDeltas() {
            return false;
        }
        let (dx, dy) = (event.scrollingDeltaX(), event.scrollingDeltaY());
        if dx != 0.0 || dy == 0.0 {
            return false;
        }
        let index = section_index(section);
        let max = self.ivars().state.borrow().strips[index].max_offset;
        let Some(bar) = self.bars().get(index) else {
            return false;
        };
        if max <= 0.0 {
            return false;
        }
        let offset = (offset_of(bar) - dy * LINE_SCROLL).clamp(0.0, max);
        self.scroll_bar(index, offset);
        true
    }

    fn press(&self, point: (f64, f64)) {
        match self.hit(point) {
            None => {}
            Some(Hit {
                locked: true, name, ..
            }) => {
                self.set_message(Some(format!("{name} cannot be moved")), false);
            }
            Some(Hit { id, origin, .. }) => {
                self.ivars().state.borrow_mut().drag = Some(Drag {
                    id,
                    grab: (point.0 - origin.0, point.1 - origin.1),
                    pointer: point,
                    over: None,
                });
                self.set_message(None, false);
                self.redraw();
            }
        }
    }

    /// Where a drop at `point` (canvas coordinates) would go: the bar under
    /// it and the insertion index in that bar's document coordinates,
    /// clamped like the drop rule.
    fn over_at(&self, point: (f64, f64)) -> Option<(Section, usize)> {
        let state = self.ivars().state.borrow();
        let drag = state.drag.as_ref()?;
        let model = state.snapshot.model.as_ref()?;
        let geometry = self.current_geometry(&state);
        let bars: Vec<(Section, Rect)> = geometry.iter().map(|g| (g.section, g.bar)).collect();
        let section = bar_at(&bars, point.1, BAR_SLACK)?;
        let index = section_index(section);
        let g = geometry.get(index)?;
        let offset = offset_of(self.bars().get(index)?);
        let slots = Self::slots_without(&state, section, drag.id);
        let at = insertion_at(&slots, point.0 - g.strip.x, offset);
        Some((section, clamp_insertion(model, drag.id, section, at)))
    }

    fn track(&self, point: (f64, f64)) {
        if self.ivars().state.borrow().drag.is_none() {
            return;
        }
        if let Some(drag) = self.ivars().state.borrow_mut().drag.as_mut() {
            drag.pointer = point;
        }
        let over = self.over_at(point);
        if let Some(drag) = self.ivars().state.borrow_mut().drag.as_mut() {
            drag.over = over;
        }
        self.update_autoscroll();
        self.redraw();
    }

    /// The bar to scroll and its next offset while the dragged tile is near
    /// the left or right edge of the bar under the pointer.
    fn autoscroll_target(&self) -> Option<(usize, f64)> {
        let state = self.ivars().state.borrow();
        let drag = state.drag.as_ref()?;
        let geometry = self.current_geometry(&state);
        let bars: Vec<(Section, Rect)> = geometry.iter().map(|g| (g.section, g.bar)).collect();
        let index = section_index(bar_at(&bars, drag.pointer.1, BAR_SLACK)?);
        let g = geometry.get(index)?;
        let offset = offset_of(self.bars().get(index)?);
        let next = autoscroll_offset(
            drag.pointer.0 - g.strip.x,
            g.strip.width,
            offset,
            state.strips[index].max_offset,
            AUTOSCROLL_EDGE,
            AUTOSCROLL_MAX_STEP,
        );
        ((next - offset).abs() > f64::EPSILON).then_some((index, next))
    }

    /// Runs the auto-scroll timer while a bar should scroll, and stops it
    /// otherwise. Pointer moves alone cannot scroll a bar while the pointer
    /// rests near an edge.
    fn update_autoscroll(&self) {
        if self.autoscroll_target().is_none() {
            self.stop_autoscroll();
            return;
        }
        let mut slot = self.ivars().autoscroll.borrow_mut();
        if slot.is_some() {
            return;
        }
        let me = Weak::new(self);
        *slot = Some(timer::schedule_common(
            self.mtm(),
            AUTOSCROLL_TICK,
            true,
            move |timer| match me.load() {
                Some(canvas) => canvas.autoscroll_tick(),
                None => timer.invalidate(),
            },
        ));
    }

    fn autoscroll_tick(&self) {
        let Some((index, offset)) = self.autoscroll_target() else {
            self.stop_autoscroll();
            return;
        };
        self.scroll_bar(index, offset);
        let pointer = self.ivars().state.borrow().drag.as_ref().map(|d| d.pointer);
        if let Some(pointer) = pointer {
            let over = self.over_at(pointer);
            if let Some(drag) = self.ivars().state.borrow_mut().drag.as_mut() {
                drag.over = over;
            }
        }
        self.redraw();
    }

    fn stop_autoscroll(&self) {
        if let Some(timer) = self.ivars().autoscroll.borrow_mut().take() {
            timer.invalidate();
        }
    }

    fn release(&self, point: (f64, f64)) {
        self.track(point);
        self.stop_autoscroll();
        let drop = {
            let mut state = self.ivars().state.borrow_mut();
            state
                .drag
                .take()
                .and_then(|d| d.over.map(|(section, index)| (d.id, section, index)))
        };
        self.redraw();
        let Some((id, section, index)) = drop else {
            return;
        };
        // Take the handler out while it runs, so it may call back into the
        // canvas.
        let handler = self.ivars().state.borrow_mut().on_drop.take();
        if let Some(handler) = handler {
            handler(id, section, index);
            let mut state = self.ivars().state.borrow_mut();
            if state.on_drop.is_none() {
                state.on_drop = Some(handler);
            }
        }
    }
}

fn draw_tile(
    image: Option<&TileImage>,
    title: &str,
    frame: &Rect,
    alpha: f64,
    font: &NSFont,
    text_color: &NSColor,
) {
    match image.and_then(|i| i.image.as_ref()) {
        Some(image) => {
            // SAFETY: a zero source rect draws the whole image; no hints.
            unsafe {
                image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                    ns(frame),
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    alpha,
                    true,
                    None,
                );
            }
        }
        None => {
            let inner = Rect::new(
                frame.x + 1.0,
                frame.y + 3.0,
                (frame.width - 2.0).max(0.0),
                (frame.height - 6.0).max(0.0),
            );
            text_color.colorWithAlphaComponent(0.18 * alpha).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(&inner), 4.0, 4.0).fill();
            // A neutral symbol; the tooltip names the item. The title is
            // the fallback when the symbol is missing.
            let side = (inner.height - 4.0).clamp(8.0, 14.0);
            match tinted_symbol(PLACEHOLDER_SYMBOL, side, text_color) {
                Some(symbol) => {
                    let size = symbol.size();
                    let at = Rect::new(
                        inner.mid_x() - size.width / 2.0,
                        inner.mid_y() - size.height / 2.0,
                        size.width,
                        size.height,
                    );
                    // SAFETY: as above.
                    unsafe {
                        symbol.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                            ns(&at),
                            NSRect::ZERO,
                            NSCompositingOperation::SourceOver,
                            alpha,
                            true,
                            None,
                        );
                    }
                }
                None => draw_text(
                    title,
                    &Rect::new(
                        inner.x + 2.0,
                        inner.mid_y() - 6.5,
                        (inner.width - 4.0).max(0.0),
                        13.0,
                    ),
                    font,
                    text_color,
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_contrasts_with_the_sampled_bar() {
        // The navy menu bar sampled on the development host.
        assert_eq!(
            knob_style(Some([0.070, 0.189, 0.328])),
            NSScrollerKnobStyle::Light
        );
        assert_eq!(
            knob_style(Some([0.95, 0.95, 0.95])),
            NSScrollerKnobStyle::Dark
        );
        assert_eq!(knob_style(None), NSScrollerKnobStyle::Default);
    }
}
