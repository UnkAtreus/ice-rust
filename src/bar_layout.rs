//! Placement of item images inside the Ice Bar panel.

use crate::model::{CLICK_MAX_SECONDS, CLICK_MAX_TRAVEL, Rect};

/// Where one item image goes, in panel-local AppKit coordinates (origin at
/// the panel's bottom left).
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
    /// Number of leftmost items that did not fit.
    pub dropped: usize,
    /// Leftmost tile that stands in for the dropped items. `None` when
    /// nothing was dropped or the tile itself does not fit.
    pub overflow: Option<Rect>,
}

/// Lays out items left to right with `padding` around and between them.
///
/// `sizes` are image sizes in points, left to right. Images taller than the
/// panel are scaled down to fit, keeping their aspect ratio, and every image
/// is centered vertically. When the row is wider than `max_width`, the
/// leftmost items are dropped so that the items nearest the divider stay,
/// and a tile of `overflow_size` takes the leftmost place to show that
/// items are missing. The tile's height is clamped to the panel and its
/// width is kept, since it holds text. A zero `overflow_size` disables it.
pub fn layout(
    sizes: &[(f64, f64)],
    height: f64,
    padding: f64,
    max_width: f64,
    overflow_size: (f64, f64),
) -> BarLayout {
    let fitted: Vec<(usize, f64, f64)> = sizes
        .iter()
        .enumerate()
        .filter(|(_, (w, h))| *w > 0.0 && *h > 0.0)
        .map(|(i, &(w, h))| {
            if h > height {
                let s = height / h;
                (i, w * s, height)
            } else {
                (i, w, h)
            }
        })
        .collect();

    let (mut first_kept, mut width) =
        fit_rightmost(&fitted, padding * 2.0, false, padding, max_width);
    let mut overflow = None;
    let (tile_w, tile_h) = overflow_size;
    let with_tile = padding * 2.0 + tile_w;
    if first_kept > 0 && tile_w > 0.0 && tile_h > 0.0 && with_tile <= max_width {
        (first_kept, width) = fit_rightmost(&fitted, with_tile, true, padding, max_width);
        let tile_h = tile_h.min(height);
        overflow = Some(Rect::new(padding, (height - tile_h) / 2.0, tile_w, tile_h));
    }

    let mut slots = Vec::with_capacity(fitted.len() - first_kept);
    let mut x = overflow.map_or(padding, |tile| tile.max_x() + padding);
    for &(index, w, h) in &fitted[first_kept..] {
        slots.push(Slot {
            index,
            frame: Rect::new(x, (height - h) / 2.0, w, h),
        });
        x += w + padding;
    }

    BarLayout {
        width,
        height,
        slots,
        dropped: first_kept,
        overflow,
    }
}

/// Keeps the longest run of rightmost items that fits in `max_width`.
/// `start` is the width already taken. `gap_first` adds `padding` left of
/// the leftmost kept item too, for when something sits left of the run.
/// Returns the position of the first kept item and the total width.
fn fit_rightmost(
    fitted: &[(usize, f64, f64)],
    start: f64,
    gap_first: bool,
    padding: f64,
    max_width: f64,
) -> (usize, f64) {
    let mut width = start;
    let mut first_kept = fitted.len();
    for (pos, &(_, w, _)) in fitted.iter().enumerate().rev() {
        let gap = if gap_first || first_kept < fitted.len() {
            padding
        } else {
            0.0
        };
        if width + w + gap > max_width {
            break;
        }
        width += w + gap;
        first_kept = pos;
    }
    (first_kept, width)
}

/// Text of the tile that stands in for `count` dropped items.
pub fn overflow_label(count: usize) -> String {
    format!("+{count}")
}

/// Size in points of a text tile, such as a message or the overflow tile.
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

    const NO_TILE: (f64, f64) = (0.0, 0.0);
    const TILE: (f64, f64) = (10.0, 22.0);

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
        let l = layout(
            &[(16.0, 30.0), (17.0, 30.0), (19.0, 30.0)],
            30.0,
            4.0,
            1000.0,
            TILE,
        );
        assert_eq!(l.width, 4.0 + 16.0 + 4.0 + 17.0 + 4.0 + 19.0 + 4.0);
        assert_eq!(l.height, 30.0);
        assert_eq!(l.dropped, 0);
        assert_eq!(l.overflow, None);
        let xs: Vec<f64> = l.slots.iter().map(|s| s.frame.x).collect();
        assert_eq!(xs, vec![4.0, 24.0, 45.0]);
        assert_eq!(l.slots[2].frame, Rect::new(45.0, 0.0, 19.0, 30.0));
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![0, 1, 2]);
    }

    #[test]
    fn centers_short_images_vertically() {
        let l = layout(&[(20.0, 16.0)], 30.0, 4.0, 1000.0, NO_TILE);
        assert_eq!(l.slots[0].frame, Rect::new(4.0, 7.0, 20.0, 16.0));
    }

    #[test]
    fn scales_tall_images_to_fit() {
        let l = layout(&[(20.0, 60.0)], 30.0, 4.0, 1000.0, NO_TILE);
        assert_eq!(l.slots[0].frame, Rect::new(4.0, 0.0, 10.0, 30.0));
        assert_eq!(l.width, 18.0);
    }

    #[test]
    fn drops_leftmost_items_on_overflow() {
        let l = layout(
            &[(16.0, 30.0), (17.0, 30.0), (19.0, 30.0)],
            30.0,
            4.0,
            50.0,
            NO_TILE,
        );
        assert_eq!(l.dropped, 1);
        assert_eq!(l.overflow, None);
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![1, 2]);
        assert_eq!(l.width, 48.0);
        assert_eq!(l.slots[0].frame.x, 4.0);
    }

    #[test]
    fn overflow_tile_takes_leftmost_place() {
        // Items 1 and 2 fit without the tile (48 pt). The tile and its gap
        // push item 1 out as well.
        let l = layout(
            &[(16.0, 30.0), (17.0, 30.0), (19.0, 30.0)],
            30.0,
            4.0,
            50.0,
            TILE,
        );
        assert_eq!(l.dropped, 2);
        assert_eq!(l.overflow, Some(Rect::new(4.0, 4.0, 10.0, 22.0)));
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![2]);
        assert_eq!(l.slots[0].frame, Rect::new(18.0, 0.0, 19.0, 30.0));
        assert_eq!(l.width, 4.0 + 10.0 + 4.0 + 19.0 + 4.0);
    }

    #[test]
    fn overflow_tile_alone_when_no_item_fits() {
        let l = layout(&[(100.0, 30.0)], 30.0, 4.0, 50.0, TILE);
        assert_eq!(l.dropped, 1);
        assert!(l.slots.is_empty());
        assert_eq!(l.overflow, Some(Rect::new(4.0, 4.0, 10.0, 22.0)));
        assert_eq!(l.width, 18.0);
    }

    #[test]
    fn overflow_tile_skipped_when_it_does_not_fit() {
        let l = layout(&[(100.0, 30.0)], 30.0, 4.0, 15.0, TILE);
        assert_eq!(l.dropped, 1);
        assert!(l.slots.is_empty());
        assert_eq!(l.overflow, None);
        assert_eq!(l.width, 8.0);
    }

    #[test]
    fn overflow_tile_height_clamped_to_panel() {
        let l = layout(&[(100.0, 20.0)], 20.0, 4.0, 50.0, TILE);
        assert_eq!(l.overflow, Some(Rect::new(4.0, 0.0, 10.0, 20.0)));
    }

    #[test]
    fn overflowing_row_stays_within_max_width() {
        let sizes = [(16.0, 30.0); 20];
        for max_width in [20.0, 30.0, 50.0, 100.0, 200.0, 400.0] {
            let l = layout(&sizes, 30.0, 4.0, max_width, (18.0, 22.0));
            assert!(l.width <= max_width, "width {} > {max_width}", l.width);
            assert_eq!(l.dropped + l.slots.len(), sizes.len());
            let right_edge = l
                .slots
                .last()
                .map(|s| s.frame.max_x())
                .or(l.overflow.map(|t| t.max_x()))
                .unwrap_or(0.0);
            if right_edge > 0.0 {
                assert_eq!(right_edge + 4.0, l.width);
            }
        }
    }

    #[test]
    fn nothing_fits() {
        let l = layout(&[(100.0, 30.0)], 30.0, 4.0, 50.0, NO_TILE);
        assert_eq!(l.dropped, 1);
        assert!(l.slots.is_empty());
        assert_eq!(l.width, 8.0);
    }

    #[test]
    fn empty_input() {
        let l = layout(&[], 30.0, 4.0, 1000.0, TILE);
        assert!(l.slots.is_empty());
        assert_eq!(l.dropped, 0);
        assert_eq!(l.overflow, None);
        assert_eq!(l.width, 8.0);
    }

    #[test]
    fn ignores_degenerate_sizes() {
        let l = layout(
            &[(0.0, 30.0), (10.0, 0.0), (12.0, 30.0)],
            30.0,
            4.0,
            1000.0,
            NO_TILE,
        );
        let idx: Vec<usize> = l.slots.iter().map(|s| s.index).collect();
        assert_eq!(idx, vec![2]);
    }

    #[test]
    fn text_tiles() {
        assert_eq!(overflow_label(12), "+12");
        assert_eq!(text_tile_size("+12"), (30.0, 22.0));
    }
}
