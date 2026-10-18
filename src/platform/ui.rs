//! Small AppKit building blocks for the Settings window: a flipped
//! container view, closure-backed target and window delegate objects, a
//! rounded box, and label and symbol helpers.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions,
    NSBezierPath, NSColor, NSControl, NSFont, NSFontWeight, NSFontWeightBold, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration, NSImageView,
    NSLineBreakMode, NSResponder, NSTextField, NSView, NSViewLayerContentsRedrawPolicy,
    NSWindowDelegate,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSRect, NSString};

pub fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
}

/// How a view follows its parent when the Settings window resizes (v1.2).
/// The parents are flipped, so a view keeps its distance from the top
/// unless it follows the height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    /// Grows with the parent's width.
    Width,
    /// Keeps its distance from the parent's right edge.
    Right,
    /// Grows with the parent's height.
    Height,
    /// Grows with the parent's width and height.
    Both,
    /// Stays centered horizontally.
    Center,
}

/// Sets the autoresizing mask of `view` for `how`.
pub fn follow(view: &NSView, how: Follow) {
    let mask = match how {
        Follow::Width => NSAutoresizingMaskOptions::ViewWidthSizable,
        Follow::Right => NSAutoresizingMaskOptions::ViewMinXMargin,
        Follow::Height => NSAutoresizingMaskOptions::ViewHeightSizable,
        Follow::Both => {
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable
        }
        Follow::Center => {
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxXMargin
        }
    };
    view.setAutoresizingMask(mask);
}

/// Adds `view` to `parent`, following it as `how` says.
pub fn add(parent: &NSView, view: &NSView, how: Follow) {
    follow(view, how);
    parent.addSubview(view);
}

/// Redraws a custom-drawn view while its size changes, not only at the
/// end of a live resize.
pub fn redraw_on_resize(view: &NSView) {
    view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `FlippedView::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct FlippedView;

    // SAFETY: isFlipped returns BOOL, as NSView declares.
    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    /// A plain container with its origin at the top left.
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: initWithFrame: is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub struct ActionIvars {
    handler: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. No Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    // SAFETY: the action signature `-(void)fire:(id)sender`.
    impl ActionTarget {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            if let Some(sender) = sender {
                (self.ivars().handler)(sender);
            }
        }
    }
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, handler: impl Fn(&AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars {
            handler: Box::new(handler),
        });
        // SAFETY: init is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// Makes `control` send its action to this target. The caller keeps the
    /// target alive as long as the control.
    pub fn attach(&self, control: &NSControl) {
        // SAFETY: `fire:` has the `-(void)x:(id)sender` signature and the
        // caller keeps `self` alive while the control exists.
        unsafe {
            control.setTarget(Some(self));
            control.setAction(Some(sel!(fire:)));
        }
    }
}

pub struct DelegateIvars {
    will_close: Box<dyn Fn()>,
    /// Called with true when the window becomes key, false when it resigns.
    key_changed: Box<dyn Fn(bool)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. No Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    // SAFETY: windowWillClose:, windowDidBecomeKey: and windowDidResignKey:
    // take an NSNotification and return void, as NSWindowDelegate declares.
    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            (self.ivars().will_close)();
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            (self.ivars().key_changed)(true);
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            (self.ivars().key_changed)(false);
        }
    }
);

impl WindowDelegate {
    pub fn new(
        mtm: MainThreadMarker,
        will_close: impl Fn() + 'static,
        key_changed: impl Fn(bool) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            will_close: Box::new(will_close),
            key_changed: Box::new(key_changed),
        });
        // SAFETY: init is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    pub fn as_protocol(&self) -> &ProtocolObject<dyn NSWindowDelegate> {
        ProtocolObject::from_ref(self)
    }
}

/// Fill and border of a `RoundedBox`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxStyle {
    /// Grouped settings rows.
    Group,
    /// The tip box: subtle fill and border.
    Tip,
    /// A hairline between rows.
    Separator,
}

pub struct BoxIvars {
    style: BoxStyle,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond using its
    // designated initializer, which `RoundedBox::new` does. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = BoxIvars]
    pub struct RoundedBox;

    // SAFETY: isFlipped returns BOOL and drawRect: takes an NSRect, as
    // NSView declares.
    impl RoundedBox {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let b = self.bounds();
            if self.ivars().style == BoxStyle::Separator {
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(b);
                return;
            }
            let inner = rect(
                b.origin.x + 0.5,
                b.origin.y + 0.5,
                (b.size.width - 1.0).max(0.0),
                (b.size.height - 1.0).max(0.0),
            );
            let (fill, stroke, radius) = match self.ivars().style {
                BoxStyle::Group => (
                    NSColor::quaternarySystemFillColor(),
                    NSColor::separatorColor(),
                    10.0,
                ),
                BoxStyle::Tip | BoxStyle::Separator => (
                    NSColor::quinaryLabelColor(),
                    NSColor::separatorColor(),
                    8.0,
                ),
            };
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inner, radius, radius);
            fill.setFill();
            path.fill();
            stroke.setStroke();
            path.setLineWidth(1.0);
            path.stroke();
        }
    }
);

impl RoundedBox {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, style: BoxStyle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BoxIvars { style });
        // SAFETY: initWithFrame: is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        redraw_on_resize(&view);
        view
    }
}

/// Font weights from AppKit's constants.
#[derive(Clone, Copy, Debug)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

fn font_weight(weight: Weight) -> NSFontWeight {
    // SAFETY: the NSFontWeight constants are immutable values exported by
    // AppKit.
    unsafe {
        match weight {
            Weight::Regular => NSFontWeightRegular,
            Weight::Medium => NSFontWeightMedium,
            Weight::Semibold => NSFontWeightSemibold,
            Weight::Bold => NSFontWeightBold,
        }
    }
}

pub fn font(size: f64, weight: Weight) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, font_weight(weight))
}

/// A one-line, non-editable label.
pub fn label(
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    size: f64,
    weight: Weight,
    color: &NSColor,
) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(&font(size, weight)));
    field.setTextColor(Some(color));
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field.setFrame(frame);
    field
}

/// A multi-line, non-editable label.
pub fn wrapping_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    size: f64,
    color: &NSColor,
) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(&font(size, Weight::Regular)));
    field.setTextColor(Some(color));
    field.setSelectable(false);
    field.setPreferredMaxLayoutWidth(frame.size.width);
    field.setFrame(frame);
    field
}

/// An SF Symbol at `size` points, or `None` when the system lacks it.
pub fn symbol(name: &str, size: f64, weight: Weight) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let config =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(size, font_weight(weight));
    image.imageWithSymbolConfiguration(&config)
}

/// An SF Symbol at `size` points drawn in `color`, or `None` when the
/// system lacks it.
pub fn tinted_symbol(name: &str, size: f64, color: &NSColor) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        size,
        font_weight(Weight::Regular),
    )
    .configurationByApplyingConfiguration(
        &NSImageSymbolConfiguration::configurationWithHierarchicalColor(color),
    );
    image.imageWithSymbolConfiguration(&config)
}

/// An image view showing a tinted SF Symbol, centered in `frame`.
pub fn symbol_view(
    mtm: MainThreadMarker,
    name: &str,
    frame: NSRect,
    size: f64,
    tint: &NSColor,
) -> Option<Retained<NSImageView>> {
    let image = symbol(name, size, Weight::Regular)?;
    let view = NSImageView::imageViewWithImage(&image, mtm);
    view.setContentTintColor(Some(tint));
    view.setFrame(frame);
    Some(view)
}

/// Whether `appearance` is a dark one (Dark Aqua rather than Aqua).
pub fn is_dark(appearance: &NSAppearance) -> bool {
    // SAFETY: both names are immutable NSString constants exported by
    // AppKit.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let names = NSArray::from_slice(&[aqua, dark]);
    appearance
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|name| name.isEqualToString(dark))
}

/// sRGB color from 0..=1 components.
pub fn srgb(components: [f64; 3], alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(components[0], components[1], components[2], alpha)
}
