//! Transparency test and average color on raw pixel buffers.

/// Byte layout of a pixel buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelLayout {
    pub width: usize,
    pub height: usize,
    pub bytes_per_row: usize,
    pub bytes_per_pixel: usize,
    /// Offset of the alpha byte inside one pixel.
    pub alpha_offset: usize,
}

impl PixelLayout {
    /// Tightly packed 8-bit RGBA.
    pub fn rgba8(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            bytes_per_row: width * 4,
            bytes_per_pixel: 4,
            alpha_offset: 3,
        }
    }
}

/// True when every pixel's alpha is zero. An image with no pixels counts as
/// transparent. A buffer too short for the layout is treated as transparent
/// because it cannot be trusted.
pub fn is_fully_transparent(pixels: &[u8], layout: PixelLayout) -> bool {
    let PixelLayout {
        width,
        height,
        bytes_per_row,
        bytes_per_pixel,
        alpha_offset,
    } = layout;
    if width == 0 || height == 0 {
        return true;
    }
    if alpha_offset >= bytes_per_pixel || bytes_per_row < width * bytes_per_pixel {
        return true;
    }
    let needed = (height - 1) * bytes_per_row + width * bytes_per_pixel;
    if pixels.len() < needed {
        return true;
    }
    (0..height).all(|row| {
        let base = row * bytes_per_row;
        (0..width).all(|col| pixels[base + col * bytes_per_pixel + alpha_offset] == 0)
    })
}

/// Average color of a tightly packed, premultiplied 8-bit RGBA buffer, as
/// unpremultiplied RGB in 0..=1. Pixels with alpha below one half are
/// skipped. `None` when fewer than half the pixels qualify (a mostly
/// transparent capture shows only what is drawn on the background, such as
/// the menu bar's text) or the buffer is too short.
pub fn average_color(pixels: &[u8], width: usize, height: usize) -> Option<[f64; 3]> {
    let count = width.checked_mul(height)?;
    if count == 0 || pixels.len() < count * 4 {
        return None;
    }
    let mut sum = [0.0f64; 3];
    let mut used = 0usize;
    for px in pixels[..count * 4].chunks_exact(4) {
        let alpha = f64::from(px[3]) / 255.0;
        if alpha < 0.5 {
            continue;
        }
        for (channel, total) in sum.iter_mut().enumerate() {
            *total += (f64::from(px[channel]) / 255.0 / alpha).min(1.0);
        }
        used += 1;
    }
    (used > 0 && used * 2 >= count).then(|| sum.map(|total| total / used as f64))
}

/// Tone of the glyphs in item images captured from the menu bar, so a bar
/// that shows the images can contrast with them (issue #3). Each image is a
/// tightly packed, premultiplied 8-bit RGBA buffer with its width and
/// height. An image whose opaque pixels cover nine tenths of it or more
/// brings its own background and is left out, as is a buffer too short for
/// its size. Among the other images' pixels with alpha of one half or more,
/// light ones (brightness 0.6 or more) and dark ones (0.4 or less) are
/// counted; colored mid tones are not. `Some(true)` when light pixels
/// outnumber dark ones, `Some(false)` the other way round, `None` on a tie.
pub fn light_glyphs(images: &[(&[u8], usize, usize)]) -> Option<bool> {
    let (mut light, mut dark) = (0usize, 0usize);
    for &(pixels, width, height) in images {
        let Some(count) = width.checked_mul(height) else {
            continue;
        };
        if count == 0 || pixels.len() < count * 4 {
            continue;
        }
        let opaque: Vec<[f64; 3]> = pixels[..count * 4]
            .chunks_exact(4)
            .filter_map(|px| {
                let alpha = f64::from(px[3]) / 255.0;
                (alpha >= 0.5)
                    .then(|| [0, 1, 2].map(|c| (f64::from(px[c]) / 255.0 / alpha).min(1.0)))
            })
            .collect();
        if opaque.len() * 10 >= count * 9 {
            continue;
        }
        for rgb in opaque {
            let value = brightness(rgb);
            if value >= 0.6 {
                light += 1;
            } else if value <= 0.4 {
                dark += 1;
            }
        }
    }
    match light.cmp(&dark) {
        std::cmp::Ordering::Greater => Some(true),
        std::cmp::Ordering::Less => Some(false),
        std::cmp::Ordering::Equal => None,
    }
}

/// W3C perceived brightness of an RGB color in 0..=1.
pub fn brightness(rgb: [f64; 3]) -> f64 {
    (299.0 * rgb[0] + 587.0 * rgb[1] + 114.0 * rgb[2]) / 1000.0
}

/// Whether text on a background of this color should be dark (ledger
/// section 5: dark text above brightness 0.67).
pub fn prefers_dark_text(rgb: [f64; 3]) -> bool {
    brightness(rgb) > 0.67
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_contrast() {
        assert!(prefers_dark_text([1.0, 1.0, 1.0]));
        assert!(!prefers_dark_text([0.2, 0.2, 0.2]));
        assert!((brightness([1.0, 0.0, 0.0]) - 0.299).abs() < 1e-9);
        // Pure green is bright enough for dark text, pure blue is not.
        assert!(!prefers_dark_text([0.0, 1.0, 0.0]));
        assert!(!prefers_dark_text([0.0, 0.0, 1.0]));
    }

    #[test]
    fn average_of_opaque_pixels() {
        let px = [255, 0, 0, 255, 0, 0, 255, 255];
        assert_eq!(average_color(&px, 2, 1), Some([0.5, 0.0, 0.5]));
    }

    #[test]
    fn average_unpremultiplies_and_skips_faint_pixels() {
        // Half-transparent mid gray (premultiplied 64 at alpha 128) counts
        // as about 0.5; a nearly transparent pixel is skipped.
        let px = [64, 64, 64, 128, 255, 255, 255, 10];
        let [r, g, b] = average_color(&px, 2, 1).unwrap();
        for c in [r, g, b] {
            assert!((c - 0.5).abs() < 0.01, "{c}");
        }
    }

    #[test]
    fn average_needs_half_the_pixels_opaque() {
        // The menu bar window captured alone is transparent but for its
        // white text (4 of 256 sampled pixels on macOS 26.7.1): the text
        // color, not the bar's (issue #3).
        let mut text_only = vec![0u8; 16 * 4];
        text_only[..4].copy_from_slice(&[255, 255, 255, 255]);
        assert_eq!(average_color(&text_only, 4, 4), None);
        // Half the pixels opaque is enough.
        let mut half = vec![0u8; 4 * 4];
        half[..8].copy_from_slice(&[51, 102, 153, 255, 51, 102, 153, 255]);
        let [r, g, b] = average_color(&half, 2, 2).unwrap();
        assert!((r - 0.2).abs() < 0.01 && (g - 0.4).abs() < 0.01 && (b - 0.6).abs() < 0.01);
    }

    #[test]
    fn average_without_usable_pixels() {
        assert_eq!(average_color(&[0, 0, 0, 0], 1, 1), None);
        assert_eq!(average_color(&[], 0, 0), None);
        assert_eq!(average_color(&[1, 2, 3], 1, 1), None);
    }

    #[test]
    fn all_zero_alpha_is_transparent() {
        let px = vec![0u8; 4 * 3 * 2];
        assert!(is_fully_transparent(&px, PixelLayout::rgba8(3, 2)));
    }

    #[test]
    fn color_without_alpha_is_still_transparent() {
        // Premultiplied buffers should never hold color at alpha 0, but only
        // alpha decides.
        let px = vec![255, 255, 255, 0, 10, 20, 30, 0];
        assert!(is_fully_transparent(&px, PixelLayout::rgba8(2, 1)));
    }

    #[test]
    fn single_visible_pixel_is_not_transparent() {
        let mut px = vec![0u8; 4 * 3 * 2];
        // Last pixel of the second row.
        px[4 * 5 + 3] = 1;
        assert!(!is_fully_transparent(&px, PixelLayout::rgba8(3, 2)));
    }

    #[test]
    fn row_padding_is_ignored() {
        // Two pixels per row, 12 bytes per row: 4 padding bytes set to 0xFF.
        let layout = PixelLayout {
            width: 2,
            height: 2,
            bytes_per_row: 12,
            bytes_per_pixel: 4,
            alpha_offset: 3,
        };
        let mut px = vec![0u8; 24];
        for row in 0..2 {
            for b in 8..12 {
                px[row * 12 + b] = 0xFF;
            }
        }
        assert!(is_fully_transparent(&px, layout));
        px[12 + 4 + 3] = 7;
        assert!(!is_fully_transparent(&px, layout));
    }

    #[test]
    fn alpha_first_layout() {
        let layout = PixelLayout {
            width: 2,
            height: 1,
            bytes_per_row: 8,
            bytes_per_pixel: 4,
            alpha_offset: 0,
        };
        assert!(is_fully_transparent(&[0, 9, 9, 9, 0, 9, 9, 9], layout));
        assert!(!is_fully_transparent(&[0, 9, 9, 9, 3, 9, 9, 9], layout));
    }

    #[test]
    fn empty_image_is_transparent() {
        assert!(is_fully_transparent(&[], PixelLayout::rgba8(0, 0)));
        assert!(is_fully_transparent(&[], PixelLayout::rgba8(5, 0)));
    }

    #[test]
    fn short_buffer_is_transparent() {
        let px = vec![255u8; 7];
        assert!(is_fully_transparent(&px, PixelLayout::rgba8(2, 1)));
    }

    /// An RGBA image `w` x `h`: `glyph` pixels of `color` (opaque), the rest
    /// of the first `cover` pixels a dark background, the others clear.
    fn image(w: usize, h: usize, glyph: usize, color: u8, cover: usize) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        for i in 0..cover.min(w * h) {
            let c = if i < glyph { color } else { 20 };
            px[i * 4..i * 4 + 4].copy_from_slice(&[c, c, c, 255]);
        }
        px
    }

    #[test]
    fn glyph_tone_from_item_images() {
        // White text on clear, as captured from a dark menu bar (issue #3).
        let white = image(10, 10, 12, 255, 12);
        assert_eq!(light_glyphs(&[(&white, 10, 10)]), Some(true));
        // Black text on clear, from a light menu bar.
        let black = image(10, 10, 12, 0, 12);
        assert_eq!(light_glyphs(&[(&black, 10, 10)]), Some(false));
        // The majority across images wins.
        assert_eq!(
            light_glyphs(&[(&white, 10, 10), (&white, 10, 10), (&black, 10, 10)]),
            Some(true)
        );
        // Nothing to go by.
        let clear = image(10, 10, 0, 0, 0);
        assert_eq!(light_glyphs(&[(&clear, 10, 10)]), None);
        assert_eq!(light_glyphs(&[]), None);
    }

    #[test]
    fn glyph_tone_skips_mid_tones_and_opaque_images() {
        // A colored icon (mid gray brightness 0.5) says nothing.
        let colored = image(10, 10, 12, 128, 12);
        assert_eq!(light_glyphs(&[(&colored, 10, 10)]), None);
        // An image that covers its whole area brings its own background, so
        // its dark pixels do not outvote the white text of the others.
        let opaque = image(10, 10, 5, 255, 100);
        let white = image(10, 10, 12, 255, 12);
        assert_eq!(light_glyphs(&[(&opaque, 10, 10)]), None);
        assert_eq!(
            light_glyphs(&[(&opaque, 10, 10), (&white, 10, 10)]),
            Some(true)
        );
        // A buffer too short for its size is left out.
        assert_eq!(light_glyphs(&[(&white[..8], 10, 10)]), None);
    }
}
