//! What the empty-space test needs from the system (v1.5): where the
//! frontmost app's menus end, read through the Accessibility API, and the
//! notch of each display. Without Accessibility the menus cannot be read,
//! and then nothing counts as empty space.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, CGPoint, CGSize};

use crate::geometry::appkit_to_cg;
use crate::model::Rect;

// HIServices/AXUIElement.h and AXValue.h:
//   extern AXUIElementRef AXUIElementCreateApplication(pid_t pid);
//   extern AXError AXUIElementCopyAttributeValue(AXUIElementRef element,
//       CFStringRef attribute, CFTypeRef *value);
//   extern AXError AXUIElementSetMessagingTimeout(AXUIElementRef element,
//       float timeoutInSeconds);
//   extern Boolean AXValueGetValue(AXValueRef value, AXValueType theType,
//       void *valuePtr);
// AXUIElementRef and AXValueRef are CFTypeRefs, AXError is int32_t,
// AXValueType is uint32_t, Boolean is unsigned char.
// SAFETY: the declarations below match those headers.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut CFType;
    fn AXUIElementCopyAttributeValue(
        element: &CFType,
        attribute: &CFString,
        value: *mut *mut CFType,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: &CFType, timeout: f32) -> i32;
    fn AXValueGetValue(value: &CFType, value_type: u32, value_ptr: *mut c_void) -> u8;
}

const AX_SUCCESS: i32 = 0;
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;
/// Longest wait for one answer from the frontmost app, so a hung app
/// stalls the main thread for a moment at most.
const MESSAGING_TIMEOUT: f32 = 0.1;

fn attribute(element: &CFType, name: &str) -> Option<CFRetained<CFType>> {
    let name = CFString::from_str(name);
    let mut value: *mut CFType = ptr::null_mut();
    // SAFETY: `element` is a live AXUIElement and `value` a valid out
    // pointer, which receives a +1 reference on success.
    let err = unsafe { AXUIElementCopyAttributeValue(element, &name, &mut value) };
    if err != AX_SUCCESS {
        return None;
    }
    // SAFETY: the Copy rule makes us the owner of the returned reference.
    NonNull::new(value).map(|value| unsafe { CFRetained::from_raw(value) })
}

fn value<T: Default>(element: &CFType, name: &str, value_type: u32) -> Option<T> {
    let raw = attribute(element, name)?;
    let mut out = T::default();
    // SAFETY: `raw` is the AXValue the attribute returned; `value_type`
    // names the C type of `T` (CGPoint or CGSize), and AXValueGetValue
    // checks the value's type before it writes.
    let ok = unsafe { AXValueGetValue(&raw, value_type, (&raw mut out).cast()) };
    (ok != 0).then_some(out)
}

/// The process of the frontmost app.
pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

/// The frame of `pid`'s last menu bar menu (the rightmost, usually Help)
/// in CG space. Its right edge is where the app menus end. `None` without
/// Accessibility, for an app that does not answer in time, or for one
/// without menus.
pub fn last_menu_frame(pid: i32) -> Option<Rect> {
    // SAFETY: the Create rule makes us the owner; a null result is handled.
    let app = NonNull::new(unsafe { AXUIElementCreateApplication(pid) })
        .map(|app| unsafe { CFRetained::from_raw(app) })?;
    // SAFETY: `app` is a live AXUIElement.
    unsafe { AXUIElementSetMessagingTimeout(&app, MESSAGING_TIMEOUT) };
    let bar = attribute(&app, "AXMenuBar")?;
    let children = attribute(&bar, "AXChildren")?.downcast::<CFArray>().ok()?;
    // SAFETY: AXChildren is an array of AXUIElements, which are CFTypes.
    let children = unsafe { children.cast_unchecked::<CFType>() };
    let last = children.get(children.len().checked_sub(1)?)?;
    let origin: CGPoint = value(&last, "AXPosition", AX_VALUE_CG_POINT)?;
    let size: CGSize = value(&last, "AXSize", AX_VALUE_CG_SIZE)?;
    Some(Rect::new(origin.x, origin.y, size.width, size.height))
}

/// The notch of every display that has one, in CG space: the gap between
/// the two auxiliary top areas.
pub fn notches(mtm: MainThreadMarker, primary_height: f64) -> Vec<Rect> {
    NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let left = screen.auxiliaryTopLeftArea();
            let right = screen.auxiliaryTopRightArea();
            let x = left.origin.x + left.size.width;
            let width = right.origin.x - x;
            let height = left.size.height.max(right.size.height);
            (left.size.width > 0.0 && right.size.width > 0.0 && width > 0.0 && height > 0.0).then(
                || {
                    // AppKit origins are bottom edges.
                    let y = left.origin.y.min(right.origin.y);
                    appkit_to_cg(Rect::new(x, y, width, height), primary_height)
                },
            )
        })
        .collect()
}
