//! Window and display queries through the public CGWindowList API.

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayMirrorsDisplay, CGError, CGGetActiveDisplayList,
    CGGetOnlineDisplayList, CGMainDisplayID, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullDirectDisplay, kCGNullWindowID,
    kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName,
};

use crate::geometry::Display;
use crate::model::{Rect, WindowRecord};

type WindowInfo = CFDictionary<CFString, CFType>;

pub(crate) fn rect_from_cg(r: CGRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

pub(crate) fn rect_to_cg(r: &Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height))
}

/// Every window the window server knows about, on screen or not.
pub fn list_windows() -> Vec<WindowRecord> {
    list(CGWindowListOption::OptionAll)
}

/// Only windows that are currently on screen.
pub fn list_on_screen() -> Vec<WindowRecord> {
    list(CGWindowListOption::OptionOnScreenOnly)
}

fn list(option: CGWindowListOption) -> Vec<WindowRecord> {
    let Some(array) = CGWindowListCopyWindowInfo(option, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: CGWindowListCopyWindowInfo documents its result as an array of
    // CFDictionary values keyed by CFString.
    let array: CFRetained<CFArray<WindowInfo>> = unsafe { CFRetained::cast_unchecked(array) };
    array.iter().filter_map(|info| parse(&info)).collect()
}

fn number(info: &WindowInfo, key: &CFString) -> Option<CFRetained<CFNumber>> {
    info.get(key)?.downcast::<CFNumber>().ok()
}

fn string(info: &WindowInfo, key: &CFString) -> Option<String> {
    let value = info.get(key)?.downcast::<CFString>().ok()?;
    Some(value.to_string())
}

fn parse(info: &WindowInfo) -> Option<WindowRecord> {
    // SAFETY: these are immutable CFString constants exported by CoreGraphics.
    let (k_number, k_layer, k_bounds, k_name, k_owner, k_onscreen) = unsafe {
        (
            kCGWindowNumber,
            kCGWindowLayer,
            kCGWindowBounds,
            kCGWindowName,
            kCGWindowOwnerName,
            kCGWindowIsOnscreen,
        )
    };

    let id = u32::try_from(number(info, k_number)?.as_i64()?).ok()?;
    let layer = number(info, k_layer)?.as_i32()?;
    let bounds_dict = info.get(k_bounds)?.downcast::<CFDictionary>().ok()?;
    let mut cg = CGRect::ZERO;
    // SAFETY: `bounds_dict` is a live CFDictionary and `cg` is a valid out
    // pointer for the duration of the call.
    let ok = unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds_dict), &mut cg) };
    if !ok {
        return None;
    }
    let on_screen = info
        .get(k_onscreen)
        .and_then(|v| v.downcast::<CFBoolean>().ok())
        .is_some_and(|b| b.as_bool());

    Some(WindowRecord {
        id,
        layer,
        bounds: rect_from_cg(cg),
        title: string(info, k_name),
        owner_name: string(info, k_owner),
        on_screen,
    })
}

/// Display ids from `CGGetActiveDisplayList` or `CGGetOnlineDisplayList`,
/// which share one signature.
fn display_ids(
    list: unsafe extern "C-unwind" fn(u32, *mut CGDirectDisplayID, *mut u32) -> CGError,
) -> Vec<CGDirectDisplayID> {
    const MAX: usize = 32;
    let mut ids = [0 as CGDirectDisplayID; MAX];
    let mut count = 0u32;
    // SAFETY: `ids` has room for MAX entries and `count` is a valid out
    // pointer; `list` has the expected signature.
    let err = unsafe { list(MAX as u32, ids.as_mut_ptr(), &mut count) };
    if err != CGError::Success {
        return Vec::new();
    }
    ids[..(count as usize).min(MAX)].to_vec()
}

/// Displays with their bounds in CG space.
///
/// Uses the active (drawable) display list. Sleeping displays are not
/// active, yet their menu bars and status windows keep their geometry, so
/// when no display is active the online list is used instead, skipping
/// displays that mirror another.
pub fn displays() -> Vec<Display> {
    let mut ids = display_ids(CGGetActiveDisplayList);
    if ids.is_empty() {
        ids = display_ids(CGGetOnlineDisplayList)
            .into_iter()
            .filter(|&id| CGDisplayMirrorsDisplay(id) == kCGNullDirectDisplay)
            .collect();
    }
    ids.into_iter()
        .map(|id| Display {
            id,
            bounds: rect_from_cg(CGDisplayBounds(id)),
        })
        .collect()
}

/// Height of the primary display, the one at the CG origin. AppKit's
/// coordinate flip is relative to this display.
pub fn primary_height() -> f64 {
    CGDisplayBounds(CGMainDisplayID()).size.height
}
