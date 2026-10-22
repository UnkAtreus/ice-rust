//! Scrollable bars. Each bar of the Menu Bar Layout pane (v1.2, L3) and
//! the Ice Bar (v1.10) is a horizontal strip that scrolls. Positions in a
//! strip are document coordinates: x from the strip's left end, whatever
//! part is scrolled into view, and y from its top. The scroll offset is the
//! document x at the left edge of the visible part, from 0 (left end) to
//! `max_offset` (right end). Everything here is pure.

use crate::arrange::insertion_index;
use crate::bar_layout::{BarLayout, layout};
use crate::model::Rect;

/// Points per line for scroll wheels without precise deltas.
pub const LINE_SCROLL: f64 = 10.0;

/// One bar's tiles laid out in its strip.
#[derive(Clone, Debug, PartialEq)]
pub struct StripLayout {
    /// Tile frames in document coordinates. No tile is dropped for lack of
    /// room.
    pub layout: BarLayout,
    /// Width of the scrollable document: the visible width when every tile
    /// fits, otherwise the width of the row.
    pub document_width: f64,
    /// The offset that shows the right end; zero when every tile fits.
    pub max_offset: f64,
}

impl StripLayout {
    /// Width of the part in view.
    pub fn visible_width(&self) -> f64 {
        self.document_width - self.max_offset
    }
}

impl Default for StripLayout {
    fn default() -> Self {
        Self {
            layout: BarLayout {
                width: 0.0,
                height: 0.0,
                slots: Vec::new(),
            },
            document_width: 0.0,
            max_offset: 0.0,
        }
    }
}

/// Lays out tiles in a strip `visible_width` wide and `height` tall, `inset`
/// from its top and bottom, with `padding` around and between the tiles.
/// `sizes` are tile sizes in points, left to right; taller tiles are scaled
/// down. The row is right-aligned like the right end of the menu bar: when
/// it fits it ends at the strip's right edge, and when it does not the
/// document is as wide as the row and scrolls.
pub fn strip_layout(
    sizes: &[(f64, f64)],
    visible_width: f64,
    height: f64,
    inset: f64,
    padding: f64,
) -> StripLayout {
    let visible_width = visible_width.max(0.0);
    let content_height = (height - 2.0 * inset).max(0.0);
    let mut row = layout(sizes, content_height, padding);
    let document_width = visible_width.max(row.width);
    let dx = document_width - row.width;
    for slot in &mut row.slots {
        slot.frame.x += dx;
        slot.frame.y += inset;
    }
    StripLayout {
        layout: row,
        document_width,
        max_offset: document_width - visible_width,
    }
}

/// The Ice Bar's strip (v1.10): every item, in a strip as wide as the row
/// but no wider than `max_width`, the display. A wider row scrolls; like
/// Ice's, it opens at its right end (`max_offset`), where the items next to
/// the divider are.
pub fn ice_bar_strip(
    sizes: &[(f64, f64)],
    height: f64,
    padding: f64,
    max_width: f64,
) -> StripLayout {
    let row = layout(sizes, height, padding).width;
    strip_layout(sizes, row.min(max_width), height, 0.0, padding)
}

/// The offset after one step of a scroll wheel without precise deltas,
/// `dy` lines, over a strip that scrolls only sideways: a `dy` below zero
/// moves toward the right end, `LINE_SCROLL` points per line.
/// `None` when the strip fits or the event is not such a step, so the
/// scroll view handles it.
pub fn wheel_offset(precise: bool, dx: f64, dy: f64, offset: f64, max_offset: f64) -> Option<f64> {
    if precise || dx != 0.0 || dy == 0.0 || max_offset <= 0.0 {
        return None;
    }
    Some((offset - dy * LINE_SCROLL).clamp(0.0, max_offset))
}

/// The offset after the strip or its content changed size. It keeps the
/// distance from the right end, so a bar that showed its right end still
/// does, and a bar that showed nothing to scroll starts at its right end.
/// `offset` and `old_max` must both describe the strip before the change:
/// a scroll view that grows clamps its offset against the old document
/// width, so an offset read after the resize is already off by the growth.
pub fn keep_right_distance(offset: f64, old_max: f64, new_max: f64) -> f64 {
    if !offset.is_finite() {
        return new_max;
    }
    let distance = old_max - offset.clamp(0.0, old_max.max(0.0));
    (new_max - distance).clamp(0.0, new_max.max(0.0))
}

/// Document x of a point `x` from the strip's visible left edge.
pub fn document_x(x: f64, offset: f64) -> f64 {
    x + offset
}

/// x from the strip's visible left edge of a document x.
pub fn visible_x(document_x: f64, offset: f64) -> f64 {
    document_x - offset
}

/// Insertion index for a pointer `x` from the strip's visible left edge
/// while the strip is scrolled to `offset`. `slots` pairs each tile's index
/// with its frame in document coordinates (see `arrange::insertion_index`).
pub fn insertion_at(slots: &[(usize, Rect)], x: f64, offset: f64) -> usize {
    insertion_index(slots, document_x(x, offset))
}

/// The offset after one auto-scroll tick while a drag hovers at `x`, from
/// the strip's visible left edge. Within `edge` of either side the strip
/// scrolls toward that side, faster closer to the side, by at most
/// `max_step`; a pointer past the side scrolls at `max_step`. Elsewhere the
/// offset does not change. The result stays within `0..=max_offset`.
pub fn autoscroll_offset(
    x: f64,
    visible_width: f64,
    offset: f64,
    max_offset: f64,
    edge: f64,
    max_step: f64,
) -> f64 {
    let max_offset = max_offset.max(0.0);
    let offset = if offset.is_finite() {
        offset.clamp(0.0, max_offset)
    } else {
        max_offset
    };
    if edge <= 0.0 {
        return offset;
    }
    let from_left = x;
    let from_right = visible_width - x;
    let (distance, direction) = if from_left <= from_right {
        (from_left, -1.0)
    } else {
        (from_right, 1.0)
    };
    if distance >= edge {
        return offset;
    }
    let depth = ((edge - distance) / edge).clamp(0.0, 1.0);
    (offset + direction * max_step * depth).clamp(0.0, max_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(strip: &StripLayout) -> Vec<Rect> {
        strip.layout.slots.iter().map(|s| s.frame).collect()
    }

    fn slots(strip: &StripLayout) -> Vec<(usize, Rect)> {
        strip
            .layout
            .slots
            .iter()
            .map(|s| (s.index, s.frame))
            .collect()
    }

    #[test]
    fn fitting_row_is_right_aligned_in_a_strip_as_wide_as_the_bar() {
        // 2 + 30 + 2 + 20 + 2 = 56 wide in a strip 200 wide.
        let strip = strip_layout(&[(30.0, 30.0), (20.0, 30.0)], 200.0, 42.0, 6.0, 2.0);
        assert_eq!(strip.document_width, 200.0);
        assert_eq!(strip.max_offset, 0.0);
        assert_eq!(strip.layout.width, 56.0);
        assert_eq!(
            frames(&strip),
            vec![
                Rect::new(146.0, 6.0, 30.0, 30.0),
                Rect::new(178.0, 6.0, 20.0, 30.0)
            ]
        );
    }

    #[test]
    fn wide_row_keeps_every_tile_and_the_document_grows() {
        // Ice's always-hidden section on the evidence host: 15 items.
        let sizes = vec![(34.0, 30.0); 15];
        let strip = strip_layout(&sizes, 300.0, 42.0, 6.0, 2.0);
        // 15 x 34 + 16 x 2 = 542.
        assert_eq!(strip.document_width, 542.0);
        assert_eq!(strip.max_offset, 242.0);
        assert_eq!(strip.layout.slots.len(), 15);
        let f = frames(&strip);
        assert_eq!(f[0], Rect::new(2.0, 6.0, 34.0, 30.0));
        assert_eq!(f[14].max_x(), 540.0);
        // The initial offset shows the right end: the last tile ends 2 pt
        // inside the visible right edge, the first is out of view.
        assert_eq!(visible_x(f[14].max_x(), strip.max_offset), 298.0);
        assert!(visible_x(f[0].max_x(), strip.max_offset) < 0.0);
    }

    #[test]
    fn exact_fit_does_not_scroll() {
        let strip = strip_layout(&[(46.0, 30.0), (46.0, 30.0)], 98.0, 42.0, 6.0, 2.0);
        assert_eq!(strip.document_width, 98.0);
        assert_eq!(strip.max_offset, 0.0);
        assert_eq!(frames(&strip)[0].x, 2.0);
    }

    #[test]
    fn tall_tiles_are_scaled_and_centered() {
        let strip = strip_layout(&[(40.0, 60.0), (10.0, 20.0)], 200.0, 42.0, 6.0, 2.0);
        let f = frames(&strip);
        assert_eq!((f[0].width, f[0].height, f[0].y), (20.0, 30.0, 6.0));
        assert_eq!((f[1].height, f[1].y), (20.0, 11.0));
    }

    #[test]
    fn empty_and_degenerate_strips() {
        let strip = strip_layout(&[], 200.0, 42.0, 6.0, 2.0);
        assert_eq!(strip.document_width, 200.0);
        assert_eq!(strip.max_offset, 0.0);
        assert!(strip.layout.slots.is_empty());
        // A strip with no width still holds the row.
        let strip = strip_layout(&[(30.0, 30.0)], -5.0, 42.0, 6.0, 2.0);
        assert_eq!(strip.document_width, 34.0);
        assert_eq!(strip.max_offset, 34.0);
    }

    #[test]
    fn ice_bar_that_fits_is_as_wide_as_its_row() {
        let strip = ice_bar_strip(
            &[(16.0, 30.0), (17.0, 30.0), (19.0, 30.0)],
            30.0,
            4.0,
            2560.0,
        );
        assert_eq!(strip.document_width, 68.0);
        assert_eq!(strip.visible_width(), 68.0);
        assert_eq!(strip.max_offset, 0.0);
        let xs: Vec<f64> = frames(&strip).iter().map(|f| f.x).collect();
        assert_eq!(xs, vec![4.0, 24.0, 45.0]);
        assert_eq!(frames(&strip)[0].y, 0.0);
    }

    #[test]
    fn ice_bar_wider_than_the_display_scrolls_from_its_right_end() {
        // 100 items of 16 pt with 4 pt padding: 4 + 100 x 20 = 2004 pt.
        let strip = ice_bar_strip(&[(16.0, 30.0); 100], 30.0, 4.0, 1512.0);
        assert_eq!(strip.document_width, 2004.0);
        assert_eq!(strip.visible_width(), 1512.0);
        assert_eq!(strip.max_offset, 492.0);
        assert_eq!(strip.layout.slots.len(), 100);
        // Opened at the right end, the last item ends 4 pt inside the
        // panel and the first is out of view.
        let f = frames(&strip);
        assert_eq!(visible_x(f[99].max_x(), strip.max_offset), 1508.0);
        assert!(visible_x(f[0].max_x(), strip.max_offset) < 0.0);
        // Scrolled to the left end, the first item shows.
        assert_eq!(visible_x(f[0].x, 0.0), 4.0);
    }

    #[test]
    fn ice_bar_exactly_as_wide_as_the_display_does_not_scroll() {
        let strip = ice_bar_strip(&[(16.0, 30.0); 100], 30.0, 4.0, 2004.0);
        assert_eq!(strip.visible_width(), 2004.0);
        assert_eq!(strip.max_offset, 0.0);
    }

    #[test]
    fn ice_bar_scales_tall_images_and_holds_a_message() {
        let strip = ice_bar_strip(&[(20.0, 60.0)], 30.0, 4.0, 2560.0);
        assert_eq!(frames(&strip), vec![Rect::new(4.0, 0.0, 10.0, 30.0)]);
        let empty = ice_bar_strip(&[], 30.0, 4.0, 2560.0);
        assert_eq!((empty.document_width, empty.max_offset), (8.0, 0.0));
    }

    #[test]
    fn wheel_steps_scroll_sideways() {
        // Three lines toward the right end, then back.
        assert_eq!(wheel_offset(false, 0.0, -3.0, 100.0, 492.0), Some(130.0));
        assert_eq!(wheel_offset(false, 0.0, 3.0, 100.0, 492.0), Some(70.0));
        // Clamped at both ends.
        assert_eq!(wheel_offset(false, 0.0, -3.0, 480.0, 492.0), Some(492.0));
        assert_eq!(wheel_offset(false, 0.0, 3.0, 20.0, 492.0), Some(0.0));
        // A trackpad, a sideways step, no step, or a strip that fits:
        // the scroll view handles them.
        assert_eq!(wheel_offset(true, 0.0, -3.0, 100.0, 492.0), None);
        assert_eq!(wheel_offset(false, 2.0, -3.0, 100.0, 492.0), None);
        assert_eq!(wheel_offset(false, 0.0, 0.0, 100.0, 492.0), None);
        assert_eq!(wheel_offset(false, 0.0, -3.0, 0.0, 0.0), None);
    }

    #[test]
    fn offsets_keep_their_distance_from_the_right_end() {
        // First load: nothing to scroll before, so the right end shows.
        assert_eq!(keep_right_distance(0.0, 0.0, 242.0), 242.0);
        // At the right end, still at the right end after the row grew.
        assert_eq!(keep_right_distance(242.0, 242.0, 300.0), 300.0);
        // Scrolled 100 pt left of the right end, still 100 pt left of it.
        assert_eq!(keep_right_distance(142.0, 242.0, 300.0), 200.0);
        assert_eq!(keep_right_distance(100.0, 242.0, 242.0), 100.0);
        // Clamped when the strip grew wider or the row shrank.
        assert_eq!(keep_right_distance(0.0, 242.0, 100.0), 0.0);
        assert_eq!(keep_right_distance(50.0, 100.0, 0.0), 0.0);
        // Elastic overscroll past either end is clamped too.
        assert_eq!(keep_right_distance(-20.0, 100.0, 100.0), 0.0);
        assert_eq!(keep_right_distance(130.0, 100.0, 100.0), 100.0);
        // An invalid offset falls back to the right end.
        assert_eq!(keep_right_distance(f64::NAN, 100.0, 80.0), 80.0);
    }

    #[test]
    fn widening_strip_that_still_overflows_keeps_the_right_end() {
        // Ice's hidden section on the development host: 16 tiles, 609 pt
        // of row, in a strip that grows from 544 to 574 pt.
        let mut sizes = vec![(36.0, 30.0); 15];
        sizes.push((35.0, 30.0));
        let before = strip_layout(&sizes, 544.0, 42.0, 6.0, 2.0);
        assert_eq!(before.document_width, 609.0);
        assert_eq!(before.max_offset, 65.0);
        let after = strip_layout(&sizes, 574.0, 42.0, 6.0, 2.0);
        assert_eq!(after.max_offset, 35.0);
        // The offset read before the resize, at the right end, stays there:
        // the last tile ends 2 pt inside the wider strip's right edge.
        let offset = keep_right_distance(before.max_offset, before.max_offset, after.max_offset);
        assert_eq!(offset, 35.0);
        let last = after.layout.slots[15].frame;
        assert_eq!(visible_x(last.max_x(), offset), 572.0);
        // Scrolled 20 pt from the right end, still 20 pt from it.
        assert_eq!(keep_right_distance(45.0, 65.0, 35.0), 15.0);
        // Growing to 640 pt fits every tile.
        let fits = strip_layout(&sizes, 640.0, 42.0, 6.0, 2.0);
        assert_eq!(keep_right_distance(65.0, 65.0, fits.max_offset), 0.0);
    }

    #[test]
    fn document_and_visible_x_convert_with_the_offset() {
        assert_eq!(document_x(10.0, 242.0), 252.0);
        assert_eq!(visible_x(252.0, 242.0), 10.0);
        assert_eq!(document_x(10.0, 0.0), 10.0);
    }

    #[test]
    fn insertion_uses_document_coordinates() {
        let sizes = vec![(34.0, 30.0); 15];
        let strip = strip_layout(&sizes, 300.0, 42.0, 6.0, 2.0);
        let s = slots(&strip);
        // Tile i spans 2 + 36 i .. 36 + 36 i; its center is 19 + 36 i.
        // Unscrolled, a pointer 20 pt into the strip is past tile 0's center.
        assert_eq!(insertion_at(&s, 20.0, 0.0), 1);
        // Scrolled to the right end (242), the same pointer is at document
        // x 262: past tile 6's center (235), before tile 7's (271).
        assert_eq!(insertion_at(&s, 20.0, 242.0), 7);
        // The visible right edge at the right end is past every tile.
        assert_eq!(insertion_at(&s, 300.0, 242.0), 15);
        // The visible left edge at offset 100 is document x 100: before
        // tile 2's center (91) is passed, tile 3's (127) is not.
        assert_eq!(insertion_at(&s, 0.0, 100.0), 3);
    }

    #[test]
    fn autoscroll_moves_only_near_the_edges() {
        let tick = |x: f64, offset: f64| autoscroll_offset(x, 300.0, offset, 242.0, 24.0, 8.0);
        // Away from both edges nothing moves.
        assert_eq!(tick(150.0, 100.0), 100.0);
        assert_eq!(tick(24.0, 100.0), 100.0);
        assert_eq!(tick(276.0, 100.0), 100.0);
        // Near the left edge the strip scrolls left (the offset shrinks),
        // faster closer to the edge.
        let near = tick(18.0, 100.0);
        let nearer = tick(6.0, 100.0);
        assert!(near < 100.0 && nearer < near, "{near} {nearer}");
        assert_eq!(tick(0.0, 100.0), 92.0);
        // Near the right edge it scrolls right.
        assert!(tick(290.0, 100.0) > 100.0);
        assert_eq!(tick(300.0, 100.0), 108.0);
    }

    #[test]
    fn autoscroll_step_is_bounded_and_clamped() {
        let tick = |x: f64, offset: f64| autoscroll_offset(x, 300.0, offset, 242.0, 24.0, 8.0);
        // Past either edge the step is the maximum, never more.
        assert_eq!(tick(-500.0, 100.0), 92.0);
        assert_eq!(tick(900.0, 100.0), 108.0);
        // Clamped at both ends.
        assert_eq!(tick(0.0, 3.0), 0.0);
        assert_eq!(tick(0.0, 0.0), 0.0);
        assert_eq!(tick(300.0, 240.0), 242.0);
        assert_eq!(tick(300.0, 242.0), 242.0);
        // A strip that fits never scrolls.
        assert_eq!(autoscroll_offset(0.0, 300.0, 0.0, 0.0, 24.0, 8.0), 0.0);
        assert_eq!(autoscroll_offset(300.0, 300.0, 0.0, 0.0, 24.0, 8.0), 0.0);
        // A narrow strip whose edge zones overlap scrolls toward the closer
        // side.
        assert!(autoscroll_offset(10.0, 30.0, 50.0, 100.0, 24.0, 8.0) < 50.0);
        assert!(autoscroll_offset(20.0, 30.0, 50.0, 100.0, 24.0, 8.0) > 50.0);
    }
}
