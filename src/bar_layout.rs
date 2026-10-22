//! Placement of item images in a row: the Ice Bar's and each Menu Bar
//! Layout bar's (`bar_scroll`).

use crate::model::{CLICK_MAX_SECONDS, CLICK_MAX_TRAVEL, Rect};

/// Where one item image goes, in row-local coordinates (origin at the row's
/// left end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Index into the input item list.
    pub index: usize,
    pub frame: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub width: f64,
    pub height: f64,
    pub slots: Vec<Slot>,
}

/// Lays out items left to right with `padding` around and between them.
///
/// `sizes` are image sizes in points, left to right. Images taller than the
/// row are scaled down to fit, keeping their aspect ratio, and every image
/// is centered vertically. Images without a size are left out.
pub fn layout(sizes: &[(f64, f64)], height: f64, padding: f64) -> BarLayout {
    let mut slots = Vec::with_capacity(sizes.len());
    let mut x = padding;
    for (index, &(w, h)) in sizes.iter().enumerate() {
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let (w, h) = if h > height {
            (w * height / h, height)
        } else {
            (w, h)
        };
        slots.push(Slot {
            index,
            frame: Rect::new(x, (height - h) / 2.0, w, h),
        });
        x += w + padding;
    }
    BarLayout {
        width: x.max(2.0 * padding),
        height,
        slots,
    }
}

/// Size in points of a text tile, such as a message.
/// The width is an estimate for the 9 pt system font.
pub fn text_tile_size(text: &str) -> (f64, f64) {
    (6.0 * text.chars().count() as f64 + 12.0, 22.0)
}

/// Whether a press and release on an item image count as a click.
pub fn is_click(seconds: f64, dx: f64, dy: f64) -> bool {
    (0.0..CLICK_MAX_SECONDS).contains(&seconds) && dx.hypot(dy) < CLICK_MAX_TRAVEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_recognition() {
        assert!(is_click(0.1, 0.0, 0.0));
        assert!(is_click(0.49, 3.0, 3.9));
        assert!(!is_click(0.5, 0.0, 0.0));
        assert!(!is_click(0.1, 3.0, 4.0));
        assert!(!is_click(0.1, -6.0, 0.0));
        assert!(!is_click(-0.1, 0.0, 0.0));
    }

    #[test]
    fn lays_out_left_to_right_with_padding() {
        let l = layout(&[(16.0, 30.0), (17.0, 30.0), (19.0, 30.0)], 30.0, 4.0);
        assert_eq!(l.width, 4.0 + 16.0 + 4.0 + 17.0 + 4.0 + 19.0 + 4.0);
        assert_eq!(l.height, 30.0);
        let xs: Vec<f64> = l.slots.iter().map(|s| s.frame.x).collect();
        assert_eq!(xs, vec![4.0, 24.0, 45.0]);
        assert_eq!(l.slots[2].frame, Rect::new(45.0, 0.0, 19.0, 30.0));
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![0, 1, 2]);
    }

    #[test]
    fn keeps_every_item_however_wide_the_row() {
        let l = layout(&[(16.0, 30.0); 200], 30.0, 4.0);
        assert_eq!(l.slots.len(), 200);
        assert_eq!(l.width, 4.0 + 200.0 * 20.0);
        assert_eq!(l.slots[199].frame.max_x() + 4.0, l.width);
    }

    #[test]
    fn centers_short_images_vertically() {
        let l = layout(&[(20.0, 16.0)], 30.0, 4.0);
        assert_eq!(l.slots[0].frame, Rect::new(4.0, 7.0, 20.0, 16.0));
    }

    #[test]
    fn scales_tall_images_to_fit() {
        let l = layout(&[(20.0, 60.0)], 30.0, 4.0);
        assert_eq!(l.slots[0].frame, Rect::new(4.0, 0.0, 10.0, 30.0));
        assert_eq!(l.width, 18.0);
    }

    #[test]
    fn empty_input() {
        let l = layout(&[], 30.0, 4.0);
        assert!(l.slots.is_empty());
        assert_eq!(l.width, 8.0);
    }

    #[test]
    fn ignores_degenerate_sizes() {
        let l = layout(&[(0.0, 30.0), (10.0, 0.0), (12.0, 30.0)], 30.0, 4.0);
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![2]);
        assert_eq!(l.slots[0].frame.x, 4.0);
        assert_eq!(layout(&[(0.0, 30.0)], 30.0, 4.0).width, 8.0);
    }

    #[test]
    fn text_tiles() {
        assert_eq!(text_tile_size("Busy"), (36.0, 22.0));
    }
}
