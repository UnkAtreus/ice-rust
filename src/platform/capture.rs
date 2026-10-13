//! Item images through `CGWindowListCreateImageFromArray` (M7), PNG output,
//! the transparency check, and the menu bar color sample.
//!
//! On macOS 26 the capture call can block forever (G5), so callers on the
//! main thread go through the `*_bounded` functions, which run the capture
//! on a worker thread and wait at most `CAPTURE_TIMEOUT`.

use std::ffi::c_void;
use std::io;
use std::path::Path;
use std::ptr;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::{CFArray, CFIndex, CFRetained, CGPoint, CGRect, CGSize};
#[allow(deprecated)]
use objc2_core_graphics::CGWindowListCreateImageFromArray;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGRectNull, CGWindowImageOption,
};
use objc2_foundation::NSDictionary;

use super::windows::rect_to_cg;
use crate::geometry::{composite_scale, crop_rect, menu_bar_window};
use crate::image_check::{PixelLayout, average_color, is_fully_transparent};
use crate::model::{
    CAPTURE_OPTION, CAPTURE_TIMEOUT, COLOR_CAPTURE_OPTION, Rect, WALLPAPER_OWNER,
    WALLPAPER_TITLE_PREFIX, WindowId, WindowRecord,
};
use crate::worker::{BoundedError, run_bounded};

/// Every capture shares this gate, since they all go through the same
/// window server call. While a timed-out capture is still running, further
/// captures fail fast with `Busy` instead of starting more stuck threads.
static CAPTURE_GATE: AtomicBool = AtomicBool::new(false);

/// How an item image was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMethod {
    /// Cropped from one composite capture of every item.
    Composite,
    /// Captured alone after the composite crop was transparent or unusable.
    Single,
    /// The window server returned no image.
    Failed,
    /// The capture did not finish within `CAPTURE_TIMEOUT` (G5), or the
    /// worker reached its deadline before capturing this item alone.
    TimedOut,
}

impl CaptureMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureMethod::Composite => "composite",
            CaptureMethod::Single => "single",
            CaptureMethod::Failed => "failed",
            CaptureMethod::TimedOut => "timed out",
        }
    }
}

pub struct ItemImage {
    pub id: WindowId,
    pub image: Option<CFRetained<CGImage>>,
    pub method: CaptureMethod,
    /// True when the best image found is fully transparent, or missing.
    pub transparent: bool,
    /// Pixels per point of `image`.
    pub scale: f64,
}

// `CGImage` is `Send + Sync` in objc2-core-graphics (thread_safety.rs), so
// `CFRetained<CGImage>` and `ItemImage` move from the worker thread as is.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ItemImage>();
};

impl ItemImage {
    pub fn pixel_size(&self) -> (usize, usize) {
        self.image.as_deref().map_or((0, 0), |img| {
            (CGImage::width(Some(img)), CGImage::height(Some(img)))
        })
    }

    /// An entry without an image, drawn as a placeholder tile.
    pub fn missing(id: WindowId, method: CaptureMethod) -> Self {
        Self {
            id,
            image: None,
            method,
            transparent: true,
            scale: 1.0,
        }
    }
}

/// Captures the given windows by id as one image of `bounds` (CG space),
/// or of their union when `bounds` is `None`. Returns `None` when the
/// window server refuses.
fn capture_windows_in(
    bounds: Option<&Rect>,
    ids: &[WindowId],
    option: u32,
) -> Option<CFRetained<CGImage>> {
    if ids.is_empty() {
        return None;
    }
    // The array holds raw window ids, not CF objects.
    let mut values: Vec<*const c_void> =
        ids.iter().map(|&id| id as usize as *const c_void).collect();
    // SAFETY: `values` holds `values.len()` pointer-sized entries. NULL
    // callbacks make CFArray store the raw values without retaining them,
    // which is the representation CGWindowListCreateImageFromArray expects.
    let array: CFRetained<CFArray> = unsafe {
        CFArray::new(
            None,
            values.as_mut_ptr(),
            values.len() as CFIndex,
            ptr::null(),
        )
    }?;
    let rect = match bounds {
        Some(r) => rect_to_cg(r),
        // SAFETY: CGRectNull is an immutable constant exported by CoreGraphics.
        None => unsafe { CGRectNull },
    };
    // SAFETY: `array` is a live CFArray of window ids. The function returns a
    // +1 CGImage, which the binding wraps in CFRetained.
    #[allow(deprecated)]
    unsafe {
        CGWindowListCreateImageFromArray(rect, &array, CGWindowImageOption(option))
    }
}

/// Captures the given windows by id, including off-screen ones, as one image
/// covering their union. Returns `None` when the window server refuses.
/// Blocks; see `capture_items_bounded`.
pub fn capture_windows(ids: &[WindowId]) -> Option<CFRetained<CGImage>> {
    capture_windows_in(None, ids, CAPTURE_OPTION)
}

/// Crops a sub-rectangle given in image pixels (origin top left).
pub fn crop(image: &CGImage, rect: &Rect) -> Option<CFRetained<CGImage>> {
    CGImage::with_image_in_rect(Some(image), rect_to_cg(rect))
}

/// Draws the image into a premultiplied RGBA buffer of `width` x `height`
/// pixels, scaling it to fit. `None` when the context cannot be created.
fn rgba_pixels(image: &CGImage, width: usize, height: usize) -> Option<Vec<u8>> {
    let layout = PixelLayout::rgba8(width, height);
    let mut buffer = vec![0u8; layout.bytes_per_row * height];
    let space = CGColorSpace::new_device_rgb();
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: `buffer` holds bytes_per_row * height bytes, outlives the
    // context (dropped below before the buffer is returned), and matches the
    // 8-bit, 4-byte RGBA format described by the arguments.
    let context = unsafe {
        CGBitmapContextCreate(
            buffer.as_mut_ptr().cast(),
            width,
            height,
            8,
            layout.bytes_per_row,
            space.as_deref(),
            info,
        )
    }?;
    let full = CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), full, Some(image));
    drop(context);
    Some(buffer)
}

/// True when every pixel of the image has zero alpha. The image is drawn
/// into an RGBA buffer first so any source format can be checked. When the
/// buffer cannot be created the image is reported as not transparent, so a
/// possibly valid image is not thrown away.
pub fn is_transparent(image: &CGImage) -> bool {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    if width == 0 || height == 0 {
        return true;
    }
    match rgba_pixels(image, width, height) {
        Some(buffer) => is_fully_transparent(&buffer, PixelLayout::rgba8(width, height)),
        None => false,
    }
}

/// Writes the image as PNG.
pub fn write_png(image: &CGImage, path: &Path) -> io::Result<()> {
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let properties = NSDictionary::new();
    // SAFETY: an empty properties dictionary is a valid argument.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }
            .ok_or_else(|| io::Error::other("PNG encoding failed"))?;
    std::fs::write(path, data.to_vec())
}

fn single(item: &WindowRecord) -> ItemImage {
    match capture_windows(&[item.id]) {
        Some(image) => {
            let transparent = is_transparent(&image);
            let scale = if item.bounds.width > 0.0 {
                CGImage::width(Some(&image)) as f64 / item.bounds.width
            } else {
                1.0
            };
            ItemImage {
                id: item.id,
                image: Some(image),
                method: CaptureMethod::Single,
                transparent,
                scale,
            }
        }
        None => ItemImage::missing(item.id, CaptureMethod::Failed),
    }
}

/// Captures each item: one composite of all items, cropped per item by its
/// bounds relative to the union; any crop that is fully transparent, or a
/// composite whose size does not match the union, falls back to capturing
/// that item alone. Blocks; see `capture_items_bounded`.
pub fn capture_items(items: &[WindowRecord]) -> Vec<ItemImage> {
    capture_items_until(items, None)
}

/// `capture_items` that stops capturing items alone once `deadline` has
/// passed and marks the rest `TimedOut`. Each single capture can take
/// seconds on macOS 26, and the worker holds the capture gate until it
/// returns, so without a deadline one slow call keeps every other capture
/// out long after the caller stopped waiting.
pub fn capture_items_until(items: &[WindowRecord], deadline: Option<Instant>) -> Vec<ItemImage> {
    let ids: Vec<WindowId> = items.iter().map(|w| w.id).collect();
    let union = items
        .iter()
        .fold(Rect::default(), |u, w| u.union(&w.bounds));
    let composite = capture_windows(&ids);
    let scale = composite.as_deref().and_then(|img| {
        composite_scale(
            CGImage::width(Some(img)),
            CGImage::height(Some(img)),
            &union,
        )
    });

    items
        .iter()
        .map(|item| {
            if let (Some(image), Some(scale)) = (composite.as_deref(), scale) {
                let rect = crop_rect(&item.bounds, &union, scale);
                if let Some(cropped) = crop(image, &rect)
                    && !is_transparent(&cropped)
                {
                    return ItemImage {
                        id: item.id,
                        image: Some(cropped),
                        method: CaptureMethod::Composite,
                        transparent: false,
                        scale,
                    };
                }
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return ItemImage::missing(item.id, CaptureMethod::TimedOut);
            }
            single(item)
        })
        .collect()
}

/// Runs `capture` over `items` on a worker thread and waits at most
/// `timeout`. The capture function is a parameter so the timeout path can be
/// tested without the window server.
pub fn capture_items_with<F>(
    items: &[WindowRecord],
    timeout: Duration,
    gate: &'static AtomicBool,
    capture: F,
) -> Result<Vec<ItemImage>, BoundedError>
where
    F: FnOnce(&[WindowRecord]) -> Vec<ItemImage> + Send + 'static,
{
    let owned = items.to_vec();
    run_bounded(gate, timeout, move || capture(&owned))
}

/// `capture_items` on the worker thread, bounded by `CAPTURE_TIMEOUT` (G5).
/// The worker stops starting single captures at the same deadline, so it
/// ends, and opens the gate, soon after the caller gives up.
pub fn capture_items_bounded(items: &[WindowRecord]) -> Result<Vec<ItemImage>, BoundedError> {
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    capture_items_with(items, CAPTURE_TIMEOUT, &CAPTURE_GATE, move |items| {
        capture_items_until(items, Some(deadline))
    })
}

/// One entry per item: the captured images, or placeholders marked with
/// the reason when the bounded capture failed.
pub fn images_or_placeholders(
    items: &[WindowRecord],
    result: Result<Vec<ItemImage>, BoundedError>,
) -> Vec<ItemImage> {
    match result {
        Ok(images) if images.len() == items.len() => images,
        Ok(_) | Err(BoundedError::Failed) => items
            .iter()
            .map(|w| ItemImage::missing(w.id, CaptureMethod::Failed))
            .collect(),
        Err(BoundedError::TimedOut | BoundedError::Busy) => items
            .iter()
            .map(|w| ItemImage::missing(w.id, CaptureMethod::TimedOut))
            .collect(),
    }
}

/// The Dock's desktop picture window that covers `display` (CG space).
fn wallpaper_window<'a>(windows: &'a [WindowRecord], display: &Rect) -> Option<&'a WindowRecord> {
    const EDGE: f64 = 1.0;
    windows.iter().find(|w| {
        w.owner_name.as_deref() == Some(WALLPAPER_OWNER)
            && w.title_str()
                .is_some_and(|t| t.starts_with(WALLPAPER_TITLE_PREFIX))
            && (w.bounds.x - display.x).abs() <= EDGE
            && (w.bounds.y - display.y).abs() <= EDGE
            && (w.bounds.width - display.width).abs() <= EDGE
    })
}

/// Average color of the menu bar on `display` (CG space): the Window Server
/// "Menubar" window composited over the desktop picture, captured on the
/// worker thread at nominal resolution. `Ok(None)` when the windows are not
/// found or the image has no opaque pixels.
pub fn menu_bar_color(
    windows: &[WindowRecord],
    display: &Rect,
    menu_bar_height: f64,
) -> Result<Option<[f64; 3]>, BoundedError> {
    let mut ids = Vec::new();
    if let Some(bar) = menu_bar_window(windows, display) {
        ids.push(bar.id);
    }
    if let Some(wallpaper) = wallpaper_window(windows, display) {
        ids.push(wallpaper.id);
    }
    if ids.is_empty() {
        return Ok(None);
    }
    let strip = Rect::new(display.x, display.y, display.width, menu_bar_height);
    run_bounded(&CAPTURE_GATE, CAPTURE_TIMEOUT, move || {
        let image = capture_windows_in(Some(&strip), &ids, COLOR_CAPTURE_OPTION)?;
        let width = CGImage::width(Some(&image)).clamp(1, 64);
        let height = CGImage::height(Some(&image)).clamp(1, 4);
        let pixels = rgba_pixels(&image, width, height)?;
        average_color(&pixels, width, height)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::STATUS_LAYER;
    use std::time::Instant;

    fn item(id: WindowId) -> WindowRecord {
        WindowRecord {
            id,
            layer: STATUS_LAYER,
            bounds: Rect::new(0.0, 0.0, 30.0, 30.0),
            title: Some("Item-0".into()),
            owner_name: None,
            on_screen: false,
        }
    }

    fn fake_capture(items: &[WindowRecord]) -> Vec<ItemImage> {
        items
            .iter()
            .map(|w| ItemImage::missing(w.id, CaptureMethod::Single))
            .collect()
    }

    fn stuck_capture(items: &[WindowRecord]) -> Vec<ItemImage> {
        std::thread::sleep(Duration::from_millis(500));
        fake_capture(items)
    }

    #[test]
    fn bounded_capture_returns_images() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let items = vec![item(1), item(2)];
        let result = capture_items_with(&items, Duration::from_secs(2), &GATE, fake_capture);
        let images = images_or_placeholders(&items, result);
        let methods: Vec<CaptureMethod> = images.iter().map(|i| i.method).collect();
        assert_eq!(methods, vec![CaptureMethod::Single, CaptureMethod::Single]);
    }

    #[test]
    fn stuck_capture_times_out_into_placeholders() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let items = vec![item(1), item(2), item(3)];
        let start = Instant::now();
        let result = capture_items_with(&items, Duration::from_millis(50), &GATE, stuck_capture);
        assert!(start.elapsed() < Duration::from_millis(400));
        assert!(matches!(result, Err(BoundedError::TimedOut)));
        let images = images_or_placeholders(&items, result);
        assert_eq!(images.len(), 3);
        assert!(
            images
                .iter()
                .all(|i| i.method == CaptureMethod::TimedOut && i.image.is_none() && i.transparent)
        );
        // While the stuck worker runs, the next capture fails fast.
        let again = capture_items_with(&items, Duration::from_secs(2), &GATE, fake_capture);
        assert!(matches!(again, Err(BoundedError::Busy)));
        assert_eq!(
            images_or_placeholders(&items, again)[0].method,
            CaptureMethod::TimedOut
        );
    }

    #[test]
    fn wrong_count_becomes_failed_placeholders() {
        let items = vec![item(1), item(2)];
        let images = images_or_placeholders(&items, Ok(Vec::new()));
        assert_eq!(images.len(), 2);
        assert!(images.iter().all(|i| i.method == CaptureMethod::Failed));
    }

    #[test]
    fn method_names() {
        assert_eq!(CaptureMethod::TimedOut.as_str(), "timed out");
        assert_eq!(CaptureMethod::Composite.as_str(), "composite");
    }
}
