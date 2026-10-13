//! TCC permission checks (M10). `check` never prompts; the `request_*`
//! functions may show system prompts.

use std::ptr;

use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{
    CGPreflightPostEventAccess, CGPreflightScreenCaptureAccess, CGRequestPostEventAccess,
    CGRequestScreenCaptureAccess,
};

// HIServices/AXUIElement.h:
//   extern Boolean AXIsProcessTrustedWithOptions(CFDictionaryRef __nullable options);
//   extern CFStringRef kAXTrustedCheckOptionPrompt;
// Boolean is `unsigned char` (MacTypes.h).
// SAFETY: the declarations below match those headers exactly.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const CFDictionary) -> u8;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionState {
    /// Window titles of other processes and window images.
    pub screen_recording: bool,
    /// `CGPreflightPostEventAccess`: posting synthetic events.
    pub post_events: bool,
    /// Accessibility trust, which also covers event posting.
    pub accessibility: bool,
}

fn ax_trusted(prompt: bool) -> bool {
    if !prompt {
        // SAFETY: NULL options are documented as "no options", so no prompt.
        return unsafe { AXIsProcessTrustedWithOptions(ptr::null()) } != 0;
    }
    // SAFETY: kAXTrustedCheckOptionPrompt is an immutable CFString constant.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: `options` is a live CFDictionary for the duration of the call;
    // the function does not take ownership.
    unsafe { AXIsProcessTrustedWithOptions(options.as_opaque()) != 0 }
}

/// Reads the current grants without prompting.
pub fn check() -> PermissionState {
    PermissionState {
        screen_recording: CGPreflightScreenCaptureAccess(),
        post_events: CGPreflightPostEventAccess(),
        accessibility: ax_trusted(false),
    }
}

/// Asks for Screen Recording. Shows the system prompt the first time.
pub fn request_screen_recording() -> bool {
    CGRequestScreenCaptureAccess()
}

/// Asks for event posting and Accessibility. Shows system prompts when the
/// grants are missing.
pub fn request_event_access() -> bool {
    let post = CGRequestPostEventAccess();
    let ax = ax_trusted(true);
    post || ax
}
