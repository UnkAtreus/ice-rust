//! What the empty-space test needs from the system (v1.5): where the
//! frontmost app's menus end, read through the Accessibility API, and the
//! notch of each display. Without Accessibility the menus cannot be read,
//! and then nothing counts as empty space. On macOS 27 the status items
//! come from MenuBarAgent's Accessibility tree too (v1.8).

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, CGPoint, CGSize};
use objc2_foundation::NSString;

use crate::agent_bar::AgentSlot;
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
    fn AXUIElementGetPid(element: &CFType, pid: *mut i32) -> i32;
    fn AXValueGetValue(value: &CFType, value_type: u32, value_ptr: *mut c_void) -> u8;
}

const AX_SUCCESS: i32 = 0;
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;
/// Longest wait for one answer from the frontmost app, so a hung app
/// stalls the main thread for a moment at most.
const MESSAGING_TIMEOUT: f32 = 0.1;
/// The process that draws the menu bar on macOS 27.
const MENU_BAR_AGENT: &str = "com.apple.MenuBarAgent";

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

fn application(pid: i32) -> Option<CFRetained<CFType>> {
    // SAFETY: the Create rule makes us the owner; a null result is handled.
    let app = NonNull::new(unsafe { AXUIElementCreateApplication(pid) })
        .map(|app| unsafe { CFRetained::from_raw(app) })?;
    // SAFETY: `app` is a live AXUIElement.
    unsafe { AXUIElementSetMessagingTimeout(&app, MESSAGING_TIMEOUT) };
    Some(app)
}

fn children(element: &CFType) -> Vec<CFRetained<CFType>> {
    let Some(children) =
        attribute(element, "AXChildren").and_then(|c| c.downcast::<CFArray>().ok())
    else {
        return Vec::new();
    };
    // SAFETY: AXChildren is an array of AXUIElements, which are CFTypes.
    let children = unsafe { children.cast_unchecked::<CFType>() };
    (0..children.len())
        .filter_map(|i| children.get(i))
        .collect()
}

fn frame(element: &CFType) -> Option<Rect> {
    let origin: CGPoint = value(element, "AXPosition", AX_VALUE_CG_POINT)?;
    let size: CGSize = value(element, "AXSize", AX_VALUE_CG_SIZE)?;
    Some(Rect::new(origin.x, origin.y, size.width, size.height))
}

fn string(element: &CFType, name: &str) -> Option<String> {
    let text = attribute(element, name)?
        .downcast::<CFString>()
        .ok()?
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn pid_of(element: &CFType) -> Option<i32> {
    let mut pid = 0;
    // SAFETY: `element` is a live AXUIElement and `pid` a valid out pointer.
    let err = unsafe { AXUIElementGetPid(element, &mut pid) };
    (err == AX_SUCCESS).then_some(pid)
}

/// The item's title, description or identifier, looking one level down
/// for Apple's items (a hosting view around the menu extra).
fn item_name(element: &CFType) -> Option<String> {
    ["AXTitle", "AXDescription", "AXIdentifier"]
        .iter()
        .find_map(|key| string(element, key))
        .or_else(|| children(element).first().and_then(|c| item_name(c)))
}

/// The first `AXIdentifier` at or below `element`, two levels at most.
fn identifier(element: &CFType, depth: usize) -> Option<String> {
    string(element, "AXIdentifier").or_else(|| {
        (depth > 0)
            .then(|| {
                children(element)
                    .iter()
                    .find_map(|c| identifier(c, depth - 1))
            })
            .flatten()
    })
}

fn menu_bar_agent() -> Option<CFRetained<CFType>> {
    let agent = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        MENU_BAR_AGENT,
    ));
    application(agent.iter().next()?.processIdentifier())
}

/// The frames of the status items of the app `pid`, in CG space, from its
/// extras menu bar (`item_names`). Empty without items, without
/// Accessibility, or without an answer in time. Runs on any thread; never
/// ask our own process off the main thread.
pub fn extras_item_frames(pid: i32) -> Vec<Rect> {
    let Some(bar) = application(pid).and_then(|app| attribute(&app, "AXExtrasMenuBar")) else {
        return Vec::new();
    };
    children(&bar)
        .iter()
        .filter_map(|item| frame(item))
        .collect()
}

/// How many status items the app `pid` has, folded ones included: its
/// extras menu bar lists them all on macOS 27 (v1.12). `None` without an
/// answer.
pub fn extras_item_count(pid: i32) -> Option<usize> {
    let bar = application(pid).and_then(|app| attribute(&app, "AXExtrasMenuBar"))?;
    Some(children(&bar).len())
}

/// The slots of each MenuBarAgent menu bar window, one list per window in
/// MenuBarAgent's order. `None` without MenuBarAgent or Accessibility.
pub fn agent_windows() -> Option<Vec<Vec<AgentSlot>>> {
    let app = menu_bar_agent()?;
    Some(
        children(&app)
            .iter()
            .filter(|window| string(window, "AXRole").as_deref() == Some("AXWindow"))
            .map(|window| window_slots(window))
            .collect(),
    )
}

/// Every slot of every MenuBarAgent menu bar window, for `probe`. `None`
/// without MenuBarAgent or Accessibility.
pub fn agent_dump() -> Option<Vec<AgentSlot>> {
    agent_windows().map(|windows| windows.into_iter().flatten().collect())
}

/// The slots of one MenuBarAgent menu bar window.
fn window_slots(window: &CFType) -> Vec<AgentSlot> {
    children(window)
        .iter()
        .filter_map(|slot| {
            let frame = frame(slot)?;
            let overflow_control = string(slot, "AXRole").as_deref() == Some("AXButton");
            let item = if overflow_control {
                Some(slot.clone())
            } else {
                children(slot).into_iter().next()
            };
            let pid = item.as_ref().and_then(|i| pid_of(i));
            Some(AgentSlot {
                frame,
                pid,
                app: pid.and_then(bundle_id),
                name: item.as_ref().and_then(|i| item_name(i)),
                identifier: item.as_ref().and_then(|i| identifier(i, 2)),
                overflow_control,
            })
        })
        .collect()
}

/// The bundle identifier of the app `pid`.
fn bundle_id(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?
        .bundleIdentifier()
        .map(|id| id.to_string())
}

/// Every slot of MenuBarAgent's menu bar window on `bar`, the system
/// overflow control included, in CG space (macOS 27, v1.8). Each status
/// item is a slot of that one window. `None` without Accessibility, without
/// an answer in time, or without a window on `bar`.
pub fn agent_slots(bar: &Rect) -> Option<Vec<AgentSlot>> {
    let app = menu_bar_agent()?;
    let window = children(&app).into_iter().find(|window| {
        frame(window).is_some_and(|f| {
            f.min_x() < bar.max_x()
                && bar.min_x() < f.max_x()
                && f.min_y() < bar.max_y()
                && bar.min_y() < f.max_y()
        })
    })?;
    Some(window_slots(&window))
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
    let app = application(pid)?;
    let bar = attribute(&app, "AXMenuBar")?;
    frame(children(&bar).last()?)
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
