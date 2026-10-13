//! NSImages for the embedded artwork (`crate::brand`).

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSImage;
use objc2_foundation::{NSData, NSSize, NSString};

use crate::brand::{APP_ICON_PNG, MENU_GLYPH_ACTIVE_PNG, MENU_GLYPH_PNG, MENU_GLYPH_POINTS};

fn image(bytes: &[u8]) -> Option<Retained<NSImage>> {
    NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))
}

/// The app icon, for the Dock while Settings is open and the About page.
pub fn app_icon() -> Option<Retained<NSImage>> {
    image(APP_ICON_PNG)
}

/// The menu bar glyph as a template image, so AppKit tints it for the
/// menu bar's appearance. `active` picks the filled variant.
pub fn menu_glyph(active: bool) -> Option<Retained<NSImage>> {
    let glyph = image(if active {
        MENU_GLYPH_ACTIVE_PNG
    } else {
        MENU_GLYPH_PNG
    })?;
    glyph.setTemplate(true);
    glyph.setSize(NSSize::new(MENU_GLYPH_POINTS, MENU_GLYPH_POINTS));
    glyph.setAccessibilityDescription(Some(&NSString::from_str("ice-rust")));
    Some(glyph)
}
