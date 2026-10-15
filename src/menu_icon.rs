//! Menu bar icon styles (v1.4): the ice-rust logo, a pair of SF Symbols,
//! three drawn dots, or an image the user chose. Pure; `platform::menu_icon`
//! makes the NSImages.

/// One look for the icon that shows and hides the sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconStyle {
    #[default]
    Ice,
    Dot,
    Ellipsis,
    EllipsisCircle,
    Chevron,
    Arrow,
    Eye,
    Door,
    Sunglasses,
    Snowflake,
    Grid,
    Star,
    Custom,
}

impl IconStyle {
    /// Every style, in the order the Settings window lists them.
    pub const ALL: [IconStyle; 13] = [
        IconStyle::Ice,
        IconStyle::Dot,
        IconStyle::Ellipsis,
        IconStyle::EllipsisCircle,
        IconStyle::Chevron,
        IconStyle::Arrow,
        IconStyle::Eye,
        IconStyle::Door,
        IconStyle::Sunglasses,
        IconStyle::Snowflake,
        IconStyle::Grid,
        IconStyle::Star,
        IconStyle::Custom,
    ];

    /// The value stored in defaults.
    pub fn key(self) -> &'static str {
        match self {
            IconStyle::Ice => "ice",
            IconStyle::Dot => "dot",
            IconStyle::Ellipsis => "ellipsis",
            IconStyle::EllipsisCircle => "ellipsis-circle",
            IconStyle::Chevron => "chevron",
            IconStyle::Arrow => "arrow",
            IconStyle::Eye => "eye",
            IconStyle::Door => "door",
            IconStyle::Sunglasses => "sunglasses",
            IconStyle::Snowflake => "snowflake",
            IconStyle::Grid => "grid",
            IconStyle::Star => "star",
            IconStyle::Custom => "custom",
        }
    }

    pub fn from_key(key: &str) -> Option<IconStyle> {
        IconStyle::ALL.into_iter().find(|style| style.key() == key)
    }

    /// The name in the Settings window.
    pub fn title(self) -> &'static str {
        match self {
            IconStyle::Ice => "ice-rust Logo",
            IconStyle::Dot => "Dot",
            IconStyle::Ellipsis => "Ellipsis",
            IconStyle::EllipsisCircle => "Ellipsis Circle",
            IconStyle::Chevron => "Chevron",
            IconStyle::Arrow => "Arrow",
            IconStyle::Eye => "Eye",
            IconStyle::Door => "Door",
            IconStyle::Sunglasses => "Sunglasses",
            IconStyle::Snowflake => "Snowflake",
            IconStyle::Grid => "Grid",
            IconStyle::Star => "Star",
            IconStyle::Custom => "Custom Image",
        }
    }

    /// Where the style's images come from. Like the logo, most styles fill
    /// in while items are shown; Chevron and Arrow turn around, Eye opens,
    /// Door opens and Sunglasses come off.
    pub fn art(self) -> IconArt {
        let (hidden, shown) = match self {
            IconStyle::Ice => return IconArt::Logo,
            IconStyle::Ellipsis => return IconArt::Dots,
            IconStyle::Custom => return IconArt::Custom,
            IconStyle::Dot => ("circlebadge", "circlebadge.fill"),
            IconStyle::EllipsisCircle => ("ellipsis.circle", "ellipsis.circle.fill"),
            IconStyle::Chevron => ("chevron.left", "chevron.right"),
            IconStyle::Arrow => ("arrow.left.circle", "arrow.right.circle.fill"),
            IconStyle::Eye => ("eye.slash", "eye"),
            IconStyle::Door => ("door.left.hand.closed", "door.left.hand.open"),
            IconStyle::Sunglasses => ("sunglasses.fill", "sunglasses"),
            IconStyle::Snowflake => ("snowflake.circle", "snowflake.circle.fill"),
            IconStyle::Grid => ("square.grid.2x2", "square.grid.2x2.fill"),
            IconStyle::Star => ("star", "star.fill"),
        };
        IconArt::Symbols { hidden, shown }
    }

    /// SF Symbols for (items hidden, items shown), or `None` for a style
    /// with other art.
    pub fn symbols(self) -> Option<(&'static str, &'static str)> {
        match self.art() {
            IconArt::Symbols { hidden, shown } => Some((hidden, shown)),
            _ => None,
        }
    }
}

/// Where a style's two images come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconArt {
    /// The embedded ice-rust glyphs.
    Logo,
    /// SF Symbols for items hidden and items shown.
    Symbols {
        hidden: &'static str,
        shown: &'static str,
    },
    /// Three dots drawn by ice-rust: solid while items are hidden, outlined
    /// while they are shown, like Ice's Ellipsis. SF Symbols has no
    /// outlined `ellipsis`.
    Dots,
    /// The stored custom image, the same in both states.
    Custom,
}

/// Diameter of each drawn dot, in points.
pub const DOT_DIAMETER: f64 = 4.0;
/// Space between two drawn dots, in points.
pub const DOT_GAP: f64 = 2.5;
/// Line width of an outlined dot, in points.
pub const DOT_LINE: f64 = 1.1;
// An outlined dot keeps a hole of at least 1 pt.
const _: () = assert!(DOT_DIAMETER - 2.0 * DOT_LINE > 1.0);

/// Size of the three-dot image, in points.
pub fn dots_size() -> (f64, f64) {
    (3.0 * DOT_DIAMETER + 2.0 * DOT_GAP, DOT_DIAMETER)
}

/// Frames (x, y, width, height) of the three dots, left to right. An
/// outlined dot strokes this frame inset by half the line width, so its
/// line stays inside the image.
pub fn dot_frames() -> [(f64, f64, f64, f64); 3] {
    [0.0, 1.0, 2.0].map(|i| {
        (
            i * (DOT_DIAMETER + DOT_GAP),
            0.0,
            DOT_DIAMETER,
            DOT_DIAMETER,
        )
    })
}

/// Largest size of a custom image in the menu bar, in points: as tall as
/// the logo, a little wider.
pub const CUSTOM_MAX_POINTS: (f64, f64) = (24.0, 18.0);
/// Scale a custom image is stored at.
pub const CUSTOM_SCALE: f64 = 3.0;

/// The largest size with the aspect ratio of `size` that fits in `max`.
/// Small images grow to fill it. `None` for an empty or non-finite size.
pub fn fit(size: (f64, f64), max: (f64, f64)) -> Option<(f64, f64)> {
    let (w, h) = size;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = (max.0 / w).min(max.1 / h);
    Some((w * scale, h * scale))
}

/// Pixel size to store a custom image of `points` at, at least 1 x 1.
pub fn stored_pixels(points: (f64, f64)) -> (usize, usize) {
    let px = |v: f64| ((v * CUSTOM_SCALE).round() as usize).max(1);
    (px(points.0), px(points.1))
}

/// Whether a custom image looks like a menu bar glyph, so tinting it with
/// the menu bar's color is the better default: part of it is clear and
/// what is drawn is black, white or gray. A photo or a colored logo keeps
/// its colors. `rgba` is 8-bit RGBA, premultiplied or not.
pub fn suggests_tint(rgba: &[u8]) -> bool {
    const CLEAR: u8 = 16;
    const VISIBLE: u8 = 64;
    /// Largest channel spread of a gray pixel, unpremultiplied.
    const GRAY_SPREAD: u32 = 40;
    let mut clear = 0usize;
    let mut visible = 0usize;
    let mut colored = 0usize;
    for px in rgba.chunks_exact(4) {
        let alpha = px[3];
        if alpha < CLEAR {
            clear += 1;
        }
        if alpha < VISIBLE {
            continue;
        }
        visible += 1;
        let lo = px[0].min(px[1]).min(px[2]);
        let hi = px[0].max(px[1]).max(px[2]);
        let spread = u32::from(hi - lo);
        // Premultiplied channels shrink with alpha, so the spread is
        // compared at full opacity.
        if spread * 255 > GRAY_SPREAD * u32::from(alpha) {
            colored += 1;
        }
    }
    // A few colored pixels are edge noise; more than 2 % is color.
    clear > 0 && visible > 0 && colored * 50 <= visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_are_unique() {
        for style in IconStyle::ALL {
            assert_eq!(IconStyle::from_key(style.key()), Some(style));
        }
        let mut keys: Vec<_> = IconStyle::ALL.iter().map(|s| s.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), IconStyle::ALL.len());
        assert_eq!(IconStyle::from_key("Dot"), None);
        assert_eq!(IconStyle::from_key(""), None);
    }

    #[test]
    fn default_is_the_logo() {
        assert_eq!(IconStyle::default(), IconStyle::Ice);
        assert_eq!(IconStyle::ALL[0], IconStyle::Ice);
    }

    #[test]
    fn art_of_each_style() {
        assert_eq!(IconStyle::Ice.art(), IconArt::Logo);
        assert_eq!(IconStyle::Ellipsis.art(), IconArt::Dots);
        assert_eq!(IconStyle::Custom.art(), IconArt::Custom);
        assert_eq!(
            IconStyle::EllipsisCircle.art(),
            IconArt::Symbols {
                hidden: "ellipsis.circle",
                shown: "ellipsis.circle.fill"
            }
        );
        for style in IconStyle::ALL {
            let drawn = matches!(
                style,
                IconStyle::Ice | IconStyle::Ellipsis | IconStyle::Custom
            );
            assert_eq!(style.symbols().is_none(), drawn, "{style:?}");
            if let Some((hidden, shown)) = style.symbols() {
                assert_eq!(style.art(), IconArt::Symbols { hidden, shown });
            }
        }
    }

    #[test]
    fn ellipsis_is_three_dots_and_ellipsis_circle_is_a_symbol() {
        assert_eq!(IconStyle::from_key("ellipsis"), Some(IconStyle::Ellipsis));
        assert_eq!(
            IconStyle::from_key("ellipsis-circle"),
            Some(IconStyle::EllipsisCircle)
        );
        assert_eq!(IconStyle::Ellipsis.title(), "Ellipsis");
        assert_eq!(IconStyle::EllipsisCircle.title(), "Ellipsis Circle");
    }

    #[test]
    fn dots_fill_their_image_without_touching() {
        let (w, h) = dots_size();
        assert_eq!((w, h), (17.0, 4.0));
        let frames = dot_frames();
        assert_eq!(frames[0].0, 0.0);
        let (x, _, fw, _) = frames[2];
        assert_eq!(x + fw, w);
        for pair in frames.windows(2) {
            let gap = pair[1].0 - (pair[0].0 + pair[0].2);
            assert_eq!(gap, DOT_GAP);
        }
        for (_, y, fw, fh) in frames {
            assert_eq!((y, fw, fh), (0.0, DOT_DIAMETER, DOT_DIAMETER));
        }
    }

    #[test]
    fn hidden_and_shown_symbols_differ() {
        for (hidden, shown) in IconStyle::ALL.iter().filter_map(|s| s.symbols()) {
            assert_ne!(hidden, shown);
        }
    }

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        assert_eq!(fit((512.0, 512.0), CUSTOM_MAX_POINTS), Some((18.0, 18.0)));
        assert_eq!(fit((100.0, 25.0), CUSTOM_MAX_POINTS), Some((24.0, 6.0)));
        assert_eq!(fit((10.0, 20.0), CUSTOM_MAX_POINTS), Some((9.0, 18.0)));
        // Small images grow.
        assert_eq!(fit((8.0, 6.0), CUSTOM_MAX_POINTS), Some((24.0, 18.0)));
    }

    #[test]
    fn fit_rejects_empty_sizes() {
        assert_eq!(fit((0.0, 10.0), CUSTOM_MAX_POINTS), None);
        assert_eq!(fit((10.0, -1.0), CUSTOM_MAX_POINTS), None);
        assert_eq!(fit((f64::NAN, 10.0), CUSTOM_MAX_POINTS), None);
        assert_eq!(fit((f64::INFINITY, 10.0), CUSTOM_MAX_POINTS), None);
    }

    #[test]
    fn stored_pixels_are_3x() {
        assert_eq!(stored_pixels((18.0, 18.0)), (54, 54));
        assert_eq!(stored_pixels((24.0, 6.0)), (72, 18));
        assert_eq!(stored_pixels((0.1, 18.0)), (1, 54));
    }

    fn image(pixels: &[[u8; 4]]) -> Vec<u8> {
        pixels.iter().flatten().copied().collect()
    }

    #[test]
    fn black_glyph_on_clear_suggests_tint() {
        let mut px = vec![[0, 0, 0, 0]; 60];
        px.extend([[0, 0, 0, 255]; 40]);
        // Antialiased edge, premultiplied gray.
        px.push([60, 60, 60, 128]);
        assert!(suggests_tint(&image(&px)));
    }

    #[test]
    fn white_glyph_suggests_tint() {
        let mut px = vec![[0, 0, 0, 0]; 10];
        px.extend([[255, 255, 255, 255]; 10]);
        assert!(suggests_tint(&image(&px)));
    }

    #[test]
    fn colored_logo_keeps_its_colors() {
        let mut px = vec![[0, 0, 0, 0]; 50];
        px.extend([[220, 40, 40, 255]; 30]);
        px.extend([[0, 0, 0, 255]; 20]);
        assert!(!suggests_tint(&image(&px)));
    }

    #[test]
    fn opaque_image_keeps_its_colors() {
        // A gray photo with no clear part would tint to a solid block.
        let px = vec![[128, 128, 128, 255]; 100];
        assert!(!suggests_tint(&image(&px)));
    }

    #[test]
    fn empty_or_clear_image_does_not_suggest_tint() {
        assert!(!suggests_tint(&[]));
        assert!(!suggests_tint(&image(&[[0, 0, 0, 0]; 10])));
    }

    #[test]
    fn faint_color_in_premultiplied_edges_counts_as_gray() {
        // Spread 10 at alpha 64 is a spread of about 40 at full opacity.
        let mut px = vec![[0, 0, 0, 0]; 10];
        px.extend([[0, 0, 0, 255]; 98]);
        px.extend([[20, 15, 10, 64]; 2]);
        assert!(suggests_tint(&image(&px)));
    }

    #[test]
    fn a_little_edge_noise_is_allowed() {
        let mut px = vec![[0, 0, 0, 0]; 10];
        px.extend([[0, 0, 0, 255]; 98]);
        px.extend([[200, 0, 0, 255]; 2]);
        assert!(suggests_tint(&image(&px)));
        px.extend([[200, 0, 0, 255]; 2]);
        assert!(!suggests_tint(&image(&px)));
    }
}
