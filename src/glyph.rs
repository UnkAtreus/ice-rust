//! Item images for the macOS 27 Ice Bar and search panel (v1.12). There an
//! item's image can only be cropped from a capture of the screen, which
//! shows the menu bar behind the glyph (the menu bar window's own capture is
//! empty, macOS 27.0.1). The background is cut away here so the glyph sits
//! on the panel's color like a macOS 26 item image. Pure.
//!
//! MenuBarAgent draws the glyphs in one color, white or black, over a
//! background that changes slowly along the bar. Each row's background is
//! taken from the tile's left and right edges, which are the item's
//! padding. A pixel's coverage is how far its brightness moved from the
//! background toward the glyph's color; its color is what is left once
//! that much background is taken out, so colored glyphs keep their color.

/// Coverage below this is background noise (the bar's blur and texture).
const MIN_COVERAGE: f64 = 0.08;
/// Brightness steps between the glyph color and the background below which
/// coverage is measured from the difference alone.
const MIN_CONTRAST: f64 = 40.0;
/// Edge columns averaged for each row's background.
const EDGE: usize = 2;

fn brightness([r, g, b]: [f64; 3]) -> f64 {
    0.299 * r + 0.587 * g + 0.114 * b
}

fn rgb(px: &[u8]) -> [f64; 3] {
    [f64::from(px[0]), f64::from(px[1]), f64::from(px[2])]
}

/// The background of row `y` at column `x`: the average of the row's
/// leftmost and rightmost `EDGE` columns, blended across.
fn background(rgba: &[u8], width: usize, y: usize, x: usize) -> [f64; 3] {
    let edge = EDGE.min(width);
    let average = |columns: std::ops::Range<usize>| {
        let mut sum = [0.0; 3];
        let count = columns.len().max(1) as f64;
        for cx in columns {
            let i = (y * width + cx) * 4;
            for (c, total) in sum.iter_mut().enumerate() {
                *total += f64::from(rgba[i + c]);
            }
        }
        sum.map(|v| v / count)
    };
    let left = average(0..edge);
    let right = average(width - edge..width);
    let t = if width > 1 {
        x as f64 / (width - 1) as f64
    } else {
        0.0
    };
    [0, 1, 2].map(|c| left[c] + (right[c] - left[c]) * t)
}

/// Whether the glyphs in `rgba` (opaque RGBA, `width` x `height`) are
/// lighter than the bar behind them; `None` when nothing stands out.
pub fn glyph_is_light(rgba: &[u8], width: usize, height: usize) -> Option<bool> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }
    let (mut lighter, mut darker) = (0.0, 0.0);
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let bg = background(rgba, width, y, x);
            let d = brightness(rgb(&rgba[i..i + 3])) - brightness(bg);
            if d > MIN_CONTRAST / 2.0 {
                lighter += d;
            } else if d < -MIN_CONTRAST / 2.0 {
                darker -= d;
            }
        }
    }
    if lighter == 0.0 && darker == 0.0 {
        None
    } else {
        Some(lighter >= darker)
    }
}

/// Cuts the bar out of `rgba` (opaque RGBA, `width` x `height`), leaving
/// the glyph in premultiplied RGBA on transparency. `light` is the glyph's
/// tone (`glyph_is_light`), usually decided over every tile of one capture
/// so a dark patch of wallpaper cannot pass for a glyph.
pub fn cut_background(rgba: &[u8], width: usize, height: usize, light: bool) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    if rgba.len() < width * height * 4 {
        return out;
    }
    let glyph = if light { 255.0 } else { 0.0 };
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let px = &rgba[i..i + 3];
            let bg = background(rgba, width, y, x);
            let (l, lb) = (brightness(rgb(px)), brightness(bg));
            let span = glyph - lb;
            let coverage = if span.abs() >= MIN_CONTRAST {
                (l - lb) / span
            } else {
                (l - lb).abs() / 255.0
            }
            .clamp(0.0, 1.0);
            if coverage < MIN_COVERAGE {
                continue;
            }
            // Observed = coverage * color + (1 - coverage) * background, so
            // the premultiplied color is the observed minus the background
            // part.
            for c in 0..3 {
                let value = f64::from(px[c]) - (1.0 - coverage) * bg[c];
                out[i + c] = value.clamp(0.0, 255.0 * coverage).round() as u8;
            }
            out[i + 3] = (coverage * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile with a horizontal background gradient and a square glyph of
    /// `glyph` in the middle.
    fn tile(width: usize, height: usize, glyph: [u8; 3]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let inside = (6..width - 6).contains(&x) && (4..height - 4).contains(&y);
                let bg = [60 + (x as u8), 90, 140];
                let px = if inside { glyph } else { bg };
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        rgba
    }

    fn pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * width + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn a_white_glyph_keeps_its_shape_on_transparency() {
        let (w, h) = (24, 16);
        let rgba = tile(w, h, [255, 255, 255]);
        assert_eq!(glyph_is_light(&rgba, w, h), Some(true));
        let out = cut_background(&rgba, w, h, true);
        assert_eq!(pixel(&out, w, 12, 8), [255, 255, 255, 255]);
        assert_eq!(pixel(&out, w, 1, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&out, w, 20, 8), [0, 0, 0, 0]);
    }

    #[test]
    fn a_black_glyph_on_a_light_bar() {
        let (w, h) = (24, 16);
        let mut rgba = tile(w, h, [0, 0, 0]);
        // A light bar.
        for px in rgba.chunks_mut(4) {
            if px[..3] != [0, 0, 0] {
                px[..3].copy_from_slice(&[220, 220, 225]);
            }
        }
        assert_eq!(glyph_is_light(&rgba, w, h), Some(false));
        let out = cut_background(&rgba, w, h, false);
        assert_eq!(pixel(&out, w, 12, 8), [0, 0, 0, 255]);
        assert_eq!(pixel(&out, w, 2, 8)[3], 0);
    }

    #[test]
    fn half_covered_pixels_are_half_transparent() {
        let (w, h) = (24, 16);
        let mut rgba = tile(w, h, [255, 255, 255]);
        // An antialiased edge: halfway between the bar and white.
        let i = (8 * w + 6) * 4;
        let bg = [66u8, 90, 140];
        for c in 0..3 {
            rgba[i + c] = ((u16::from(bg[c]) + 255) / 2) as u8;
        }
        let out = cut_background(&rgba, w, h, true);
        let edge = pixel(&out, w, 6, 8);
        assert!((120..=135).contains(&edge[3]), "{edge:?}");
        // Premultiplied white at that coverage.
        assert!(edge[0].abs_diff(edge[3]) <= 2, "{edge:?}");
    }

    #[test]
    fn a_plain_tile_has_no_glyph() {
        let (w, h) = (10, 6);
        let rgba: Vec<u8> = (0..w * h).flat_map(|_| [60, 90, 140, 255]).collect();
        assert_eq!(glyph_is_light(&rgba, w, h), None);
        assert!(cut_background(&rgba, w, h, true).iter().all(|&b| b == 0));
        assert_eq!(glyph_is_light(&[], 0, 0), None);
    }
}
