//! The Ice Bar: a non-activating panel under the menu bar that shows images
//! of the hidden items (M8). Items wider than the display scroll sideways
//! (v1.10).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSBorderType, NSColor, NSEvent, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImage, NSPanel, NSResponder, NSScrollElasticity,
    NSScrollView, NSScrollerStyle, NSStringDrawing, NSToolTipTag, NSView, NSViewToolTipOwner,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSDictionary, NSObject, NSPoint, NSRect, NSString};

use super::item_names;
use super::layout_pane::{BarScrollView, knob_style};
use super::ui::srgb;
use crate::bar_layout::is_click;
use crate::bar_scroll::{StripLayout, wheel_offset};
use crate::image_check::prefers_dark_text;
use crate::model::{PANEL_COLLECTION_BEHAVIOR, PANEL_LEVEL, PANEL_STYLE_MASK, Rect, WindowId};

/// Called with the window id of the clicked item.
pub type ClickHandler = Rc<dyn Fn(WindowId)>;

/// One entry in the bar. `image` is `None` for a placeholder tile, which
/// shows `title` instead.
pub struct BarItem {
    pub id: WindowId,
    pub title: String,
    pub image: Option<Retained<NSImage>>,
    /// Whether clicking the tile should trigger `ClickHandler`.
    pub clickable: bool,
}

fn ns_rect(r: &Rect) -> NSRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height))
}

/// Corner radius of a bar `height` tall.
fn bar_radius(height: f64) -> f64 {
    height / 4.0
}

/// Clips `view` and its subviews to a rounded rectangle, so tiles that
/// scroll past a bar's end stay inside its rounded corners.
fn round_corners(view: &NSView, radius: f64) {
    view.setWantsLayer(true);
    // SAFETY: layer returns the view's CALayer, which exists once
    // wantsLayer is set; setCornerRadius: takes a CGFloat and
    // setMasksToBounds: a BOOL and both return void, as CALayer declares.
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![view, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: radius];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
}

/// Scrolls `scroll` so `offset` is at its visible left edge.
fn scroll_to(scroll: &NSScrollView, offset: f64) {
    let clip = scroll.contentView();
    clip.scrollToPoint(NSPoint::new(offset, 0.0));
    scroll.reflectScrolledClipView(&clip);
}

/// The bar fill, the placeholder tile fill and the placeholder text. The
/// fill is the one `brand::bar_fill_for` picks to contrast with the item
/// images captured from the menu bar: white text over a dark desktop
/// picture stayed white on the window background of light mode (issue #3).
/// Without a fill (a message tile), the window colors.
fn bar_colors(
    bar_color: Option<[f64; 3]>,
) -> (Retained<NSColor>, Retained<NSColor>, Retained<NSColor>) {
    match bar_color {
        Some(color) if prefers_dark_text(color) => (
            srgb(color, 1.0),
            NSColor::blackColor().colorWithAlphaComponent(0.1),
            NSColor::blackColor().colorWithAlphaComponent(0.7),
        ),
        Some(color) => (
            srgb(color, 1.0),
            NSColor::whiteColor().colorWithAlphaComponent(0.15),
            NSColor::whiteColor().colorWithAlphaComponent(0.8),
        ),
        None => (
            NSColor::windowBackgroundColor(),
            NSColor::quaternaryLabelColor(),
            NSColor::labelColor(),
        ),
    }
}

pub struct ItemIvars {
    id: WindowId,
    title: String,
    image: Option<Retained<NSImage>>,
    clickable: bool,
    pressed: Cell<Option<(f64, NSPoint)>>,
    on_click: ClickHandler,
    bar_color: Option<[f64; 3]>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `ItemView::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ItemIvars]
    pub struct ItemView;

    // SAFETY: argument and return types match NSView's declarations:
    // acceptsFirstMouse: (NSEvent *) -> BOOL, drawRect: (NSRect) -> void,
    // mouseDown: and mouseUp: (NSEvent *) -> void.
    impl ItemView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            match &self.ivars().image {
                Some(image) => image.drawInRect(bounds),
                None => self.draw_placeholder(bounds),
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars()
                .pressed
                .set(Some((event.timestamp(), event.locationInWindow())));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let Some((t0, p0)) = self.ivars().pressed.take() else {
                return;
            };
            let p1 = event.locationInWindow();
            let ivars = self.ivars();
            if ivars.clickable && is_click(event.timestamp() - t0, p1.x - p0.x, p1.y - p0.y) {
                (ivars.on_click)(ivars.id);
            }
        }
    }

    // SAFETY: the protocol's methods come from NSObject; none is overridden.
    unsafe impl NSObjectProtocol for ItemView {}

    // SAFETY: view:stringForToolTip:point:userData: takes an NSView, an
    // NSToolTipTag, an NSPoint and a void pointer and returns an NSString,
    // as NSViewToolTipOwner declares. The user data is never read.
    unsafe impl NSViewToolTipOwner for ItemView {
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn tool_tip(
            &self,
            _view: &NSView,
            _tag: NSToolTipTag,
            _point: NSPoint,
            _data: *mut c_void,
        ) -> Retained<NSString> {
            let ivars = self.ivars();
            NSString::from_str(&item_names::name(ivars.id, &ivars.title))
        }
    }
);

impl ItemView {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        item: &BarItem,
        on_click: ClickHandler,
        bar_color: Option<[f64; 3]>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ItemIvars {
            id: item.id,
            title: item.title.clone(),
            image: item.image.clone(),
            clickable: item.clickable,
            pressed: Cell::new(None),
            on_click,
            bar_color,
        });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// A tooltip with the name of the item's app, read when the pointer
    /// rests on the tile: the name may arrive after the bar opened
    /// (`item_names::look_up`). `IceBar::release_content` removes it.
    fn name_on_hover(&self) {
        let owner: &AnyObject = self;
        // SAFETY: the owner is this view, which implements
        // NSViewToolTipOwner. The user data is null and never read.
        unsafe { self.addToolTipRect_owner_userData(self.bounds(), owner, ptr::null_mut()) };
    }

    fn draw_placeholder(&self, bounds: NSRect) {
        let tile = CGRect::new(
            CGPoint::new(bounds.origin.x + 1.0, bounds.origin.y + 3.0),
            CGSize::new(
                (bounds.size.width - 2.0).max(0.0),
                (bounds.size.height - 6.0).max(0.0),
            ),
        );
        let (_, fill, color) = bar_colors(self.ivars().bar_color);
        fill.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(tile, 4.0, 4.0).fill();

        let font = NSFont::systemFontOfSize(9.0);
        // SAFETY: both keys are immutable NSString constants exported by AppKit.
        let (font_key, color_key) =
            unsafe { (NSFontAttributeName, NSForegroundColorAttributeName) };
        let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
        let attributes = NSDictionary::from_slices(&[font_key, color_key], &values);
        let text = NSString::from_str(&self.ivars().title);
        let text_rect = CGRect::new(
            CGPoint::new(
                tile.origin.x + 2.0,
                tile.origin.y + (tile.size.height - 11.0) / 2.0,
            ),
            CGSize::new((tile.size.width - 4.0).max(0.0), 11.0),
        );
        // SAFETY: the attributes map NSAttributedStringKey values to an NSFont
        // and an NSColor, the types AppKit expects for those keys.
        unsafe { text.drawInRect_withAttributes(text_rect, Some(&attributes)) };
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `BarView::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Option<[f64; 3]>]
    pub struct BarView;

    // SAFETY: argument and return types match NSView's declarations:
    // acceptsFirstMouse: (NSEvent *) -> BOOL, drawRect: (NSRect) -> void.
    impl BarView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let radius = bar_radius(bounds.size.height);
            bar_colors(*self.ivars()).0.setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, radius, radius).fill();
        }
    }
);

impl BarView {
    fn new(mtm: MainThreadMarker, frame: NSRect, bar_color: Option<[f64; 3]>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(bar_color);
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `ItemStrip::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = f64]
    pub struct ItemStrip;

    // SAFETY: scrollWheel: takes an NSEvent and returns void, as
    // NSResponder declares.
    impl ItemStrip {
        /// A scroll wheel's vertical steps scroll the tiles sideways; the
        /// scroll view handles everything else, a trackpad included.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !self.wheel(event) {
                // SAFETY: as above; NSView's implementation passes the
                // event to the scroll view.
                let _: () = unsafe { msg_send![super(self), scrollWheel: event] };
            }
        }
    }
);

impl ItemStrip {
    /// The tiles' document view, `max_offset` the offset that shows its
    /// right end.
    fn new(mtm: MainThreadMarker, frame: NSRect, max_offset: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(max_offset);
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn wheel(&self, event: &NSEvent) -> bool {
        let Some(scroll) = self.enclosingScrollView() else {
            return false;
        };
        let Some(offset) = wheel_offset(
            event.hasPreciseScrollingDeltas(),
            event.scrollingDeltaX(),
            event.scrollingDeltaY(),
            scroll.contentView().bounds().origin.x,
            *self.ivars(),
        ) else {
            return false;
        };
        scroll_to(&scroll, offset);
        true
    }
}

pub struct IceBar {
    mtm: MainThreadMarker,
    panel: Retained<NSPanel>,
    /// The tiles of the open bar, which own tooltip rectangles.
    strip: RefCell<Option<Retained<ItemStrip>>>,
}

impl IceBar {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::ZERO,
            NSWindowStyleMask(PANEL_STYLE_MASK),
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is owned by `IceBar` and must survive orderOut.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setLevel(PANEL_LEVEL);
        panel.setCollectionBehavior(NSWindowCollectionBehavior(PANEL_COLLECTION_BEHAVIOR));
        panel.setHasShadow(false);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        // ice-rust is not the active app while the bar is open.
        panel.setAllowsToolTipsWhenApplicationIsInactive(true);
        Self {
            mtm,
            panel,
            strip: RefCell::new(None),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.panel.isVisible()
    }

    /// Whether window number `number` is the panel's.
    pub fn owns_window(&self, number: isize) -> bool {
        number > 0 && self.panel.windowNumber() == number
    }

    /// The panel's frame in AppKit space while it is visible.
    pub fn frame(&self) -> Option<Rect> {
        if !self.panel.isVisible() {
            return None;
        }
        let f = self.panel.frame();
        Some(Rect::new(
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
        ))
    }

    /// Fills the panel and orders it front without activating the app.
    /// `frame` is in AppKit space; tile frames are in `strip`'s document
    /// coordinates. A strip wider than the panel scrolls and opens at its
    /// right end. `bar_color` is the bar fill, if any (see `bar_colors`).
    pub fn show(
        &self,
        frame: &Rect,
        strip: &StripLayout,
        items: &[BarItem],
        on_click: &ClickHandler,
        bar_color: Option<[f64; 3]>,
    ) {
        let bounds = ns_rect(&Rect::new(0.0, 0.0, frame.width, frame.height));
        let content = BarView::new(self.mtm, bounds, bar_color);
        round_corners(&content, bar_radius(frame.height));
        let tiles = ItemStrip::new(
            self.mtm,
            ns_rect(&Rect::new(0.0, 0.0, strip.document_width, frame.height)),
            strip.max_offset,
        );
        for slot in &strip.layout.slots {
            let Some(item) = items.get(slot.index) else {
                continue;
            };
            let view = ItemView::new(
                self.mtm,
                ns_rect(&slot.frame),
                item,
                on_click.clone(),
                bar_color,
            );
            view.name_on_hover();
            tiles.addSubview(&view);
        }
        let scrolls = strip.max_offset > 0.0;
        let scroll = BarScrollView::new(self.mtm);
        scroll.setFrame(bounds);
        scroll.setHasHorizontalScroller(true);
        scroll.setHasVerticalScroller(false);
        scroll.setAutohidesScrollers(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setScrollerKnobStyle(knob_style(bar_color));
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.contentView().setDrawsBackground(false);
        scroll.setVerticalScrollElasticity(NSScrollElasticity::None);
        // A bar that fits does not bounce sideways.
        scroll.setHorizontalScrollElasticity(if scrolls {
            NSScrollElasticity::Automatic
        } else {
            NSScrollElasticity::None
        });
        scroll.setDocumentView(Some(&tiles));
        content.addSubview(&scroll);
        scroll_to(&scroll, strip.max_offset);
        self.release_content();
        *self.strip.borrow_mut() = Some(tiles);
        self.panel.setContentView(Some(&content));
        self.panel.setFrame_display(ns_rect(frame), true);
        self.panel.orderFrontRegardless();
        if scrolls {
            // The scroller shows for a moment: there is more to the left.
            scroll.flashScrollers();
        }
    }

    pub fn close(&self) {
        self.panel.orderOut(None);
        self.release_content();
        self.panel.setContentView(None);
    }

    /// Removes the tiles' tooltip rectangles, which name the tiles
    /// themselves as owners (`ItemView::name_on_hover`), before the tiles go.
    fn release_content(&self) {
        if let Some(strip) = self.strip.take() {
            for view in strip.subviews() {
                view.removeAllToolTips();
            }
        }
    }
}
