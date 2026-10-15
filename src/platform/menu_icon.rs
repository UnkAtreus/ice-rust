//! NSImages for the menu bar icon styles (`crate::menu_icon`), and the
//! conversion of a chosen image file into a stored custom icon.

use std::path::Path;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage, NSImageInterpolation,
};
use objc2_foundation::{NSData, NSDictionary, NSRect, NSSize, NSString};

use super::brand;
use super::ui::rect;
use crate::brand::png_size;
use crate::menu_icon::{
    CUSTOM_MAX_POINTS, CUSTOM_SCALE, IconStyle, fit, stored_pixels, suggests_tint,
};
use crate::settings::{self, KEY_CUSTOM_ICON_IMAGE};

/// The icon while items are hidden and while they are shown.
pub struct IconImages {
    pub hidden: Retained<NSImage>,
    pub shown: Retained<NSImage>,
}

const DESCRIPTION: &str = "ice-rust";

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(DESCRIPTION)),
    )?;
    image.setTemplate(true);
    Some(image)
}

fn logo() -> Option<IconImages> {
    Some(IconImages {
        hidden: brand::menu_glyph(false)?,
        shown: brand::menu_glyph(true)?,
    })
}

/// An NSImage from stored custom icon PNG data, sized in points. `tint`
/// makes it a template image that AppKit draws in the menu bar's color.
pub fn custom_image(png: &[u8], tint: bool) -> Option<Retained<NSImage>> {
    let (w, h) = png_size(png)?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png))?;
    image.setSize(NSSize::new(
        f64::from(w) / CUSTOM_SCALE,
        f64::from(h) / CUSTOM_SCALE,
    ));
    image.setTemplate(tint);
    image.setAccessibilityDescription(Some(&NSString::from_str(DESCRIPTION)));
    Some(image)
}

/// The custom icon stored in defaults, if any.
pub fn stored_custom(tint: bool) -> Option<Retained<NSImage>> {
    custom_image(&settings::read_data(KEY_CUSTOM_ICON_IMAGE)?, tint)
}

/// Images for `style`, or `None` when this macOS lacks its symbols or no
/// custom image is stored. The custom image looks the same in both states.
pub fn style_images(style: IconStyle, tint: bool) -> Option<IconImages> {
    match style {
        IconStyle::Ice => logo(),
        IconStyle::Custom => {
            let image = stored_custom(tint)?;
            Some(IconImages {
                hidden: image.clone(),
                shown: image,
            })
        }
        _ => {
            let (hidden, shown) = style.symbols()?;
            Some(IconImages {
                hidden: symbol(hidden)?,
                shown: symbol(shown)?,
            })
        }
    }
}

/// Images for the icon: `style`, or the logo when `style` has none here.
/// `None` only when even the logo fails to decode.
pub fn icon_images(style: IconStyle, tint: bool) -> Option<IconImages> {
    style_images(style, tint).or_else(logo)
}

/// A chosen image made ready to store.
pub struct CustomIcon {
    /// PNG data at `CUSTOM_SCALE`, fitted to `CUSTOM_MAX_POINTS`.
    pub png: Vec<u8>,
    /// Whether tinting suits it (`crate::menu_icon::suggests_tint`).
    pub tint: bool,
}

/// Reads the image file at `path`, in any format NSImage opens, and draws
/// it into a small RGBA bitmap at menu bar size.
pub fn prepare_custom(path: &Path) -> Result<CustomIcon, String> {
    let image = NSImage::initWithContentsOfFile(
        NSImage::alloc(),
        &NSString::from_str(&path.to_string_lossy()),
    )
    .ok_or("macOS cannot open this file as an image")?;
    let size = image.size();
    let points = fit((size.width, size.height), CUSTOM_MAX_POINTS).ok_or("the image is empty")?;
    let (w, h) = stored_pixels(points);
    let (wide, high) = (w as isize, h as isize);
    // SAFETY: null planes make the rep allocate its own buffer; the sizes
    // describe 8-bit RGBA without row padding.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            wide,
            high,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            wide * 4,
            32,
        )
    }
    .ok_or("could not create a bitmap")?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)
        .ok_or("could not draw into the bitmap")?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setImageInterpolation(NSImageInterpolation::High);
    image.drawInRect_fromRect_operation_fraction(
        rect(0.0, 0.0, w as f64, h as f64),
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let len = w * h * 4;
    let data = rep.bitmapData();
    if data.is_null() || rep.bytesPerRow() != wide * 4 {
        return Err("could not read the bitmap".into());
    }
    // SAFETY: the rep owns `len` bytes at `data` (no planes, no padding,
    // checked above) and lives until the end of this function.
    let pixels = unsafe { std::slice::from_raw_parts(data, len) };
    let tint = suggests_tint(pixels);

    let properties = NSDictionary::new();
    // SAFETY: an empty properties dictionary is a valid argument.
    let png =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }
            .ok_or("PNG encoding failed")?
            .to_vec();
    Ok(CustomIcon { png, tint })
}
