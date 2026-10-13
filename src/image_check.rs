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
/// skipped. `None` when no pixel qualifies or the buffer is too short.
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
    (used > 0).then(|| sum.map(|total| total / used as f64))
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
}
