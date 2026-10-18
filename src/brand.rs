//! The app's artwork, embedded at build time, and the colors taken from it.
//! `scripts/make-icons.py` makes every file under `assets/` from
//! `assets/logo-source.png`.

use crate::image_check::prefers_dark_text;

/// Runtime application icon and About page image, 512 x 512.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-512.png");
/// Menu bar glyph, black on clear, drawn as a template image.
pub const MENU_GLYPH_PNG: &[u8] = include_bytes!("../assets/menubar-template.png");
/// Filled glyph while a section is revealed or the Ice Bar is open.
pub const MENU_GLYPH_ACTIVE_PNG: &[u8] = include_bytes!("../assets/menubar-template-active.png");
/// Point size of the menu bar glyph; the PNGs are 3x.
pub const MENU_GLYPH_POINTS: f64 = 18.0;

/// The logo's blue (#1C43AB): the Ice Bar and Menu Bar Layout fill behind
/// light glyphs.
pub const BAR_BLUE: [f64; 3] = [28.0 / 255.0, 67.0 / 255.0, 171.0 / 255.0];
/// A light tint of the logo's blue (#E4E8F5): the fill behind dark glyphs.
pub const BAR_TINT: [f64; 3] = [228.0 / 255.0, 232.0 / 255.0, 245.0 / 255.0];

/// The bar fill that contrasts with glyphs of the given tone.
pub fn bar_fill(light_glyphs: bool) -> [f64; 3] {
    if light_glyphs { BAR_BLUE } else { BAR_TINT }
}

/// The bar fill from what is known about the item glyphs (issue #3): their
/// tone read from the captured images, else the sampled menu bar color
/// (macOS draws light glyphs on a dark menu bar), else the window
/// appearance.
pub fn bar_fill_for(
    glyphs: Option<bool>,
    menu_bar: Option<[f64; 3]>,
    dark_appearance: bool,
) -> [f64; 3] {
    let light = glyphs
        .or_else(|| menu_bar.map(|color| !prefers_dark_text(color)))
        .unwrap_or(dark_appearance);
    bar_fill(light)
}

/// Width and height from a PNG's IHDR chunk, or `None` if `bytes` is not a PNG.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let read =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some((read(16), read(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_is_512_square() {
        assert_eq!(png_size(APP_ICON_PNG), Some((512, 512)));
    }

    #[test]
    fn menu_glyphs_are_3x_of_the_point_size() {
        let px = (MENU_GLYPH_POINTS * 3.0) as u32;
        assert_eq!(png_size(MENU_GLYPH_PNG), Some((px, px)));
        assert_eq!(png_size(MENU_GLYPH_ACTIVE_PNG), Some((px, px)));
    }

    #[test]
    fn png_size_rejects_other_data() {
        assert_eq!(png_size(b""), None);
        assert_eq!(png_size(b"GIF89a.................."), None);
        assert_eq!(png_size(&APP_ICON_PNG[..20]), None);
    }

    #[test]
    fn bar_fill_contrasts_with_the_glyphs() {
        use crate::image_check::prefers_dark_text;
        // Light glyphs on the logo's blue, dark glyphs on its light tint.
        assert_eq!(bar_fill(true), BAR_BLUE);
        assert_eq!(bar_fill(false), BAR_TINT);
        assert!(!prefers_dark_text(BAR_BLUE));
        assert!(prefers_dark_text(BAR_TINT));
    }

    #[test]
    fn bar_fill_prefers_the_glyphs_then_the_menu_bar_then_the_appearance() {
        let dark_bar = Some([0.07, 0.19, 0.32]);
        let light_bar = Some([0.9, 0.9, 0.9]);
        // The glyphs decide when known, whatever the rest says.
        assert_eq!(bar_fill_for(Some(true), light_bar, false), BAR_BLUE);
        assert_eq!(bar_fill_for(Some(false), dark_bar, true), BAR_TINT);
        // Else a dark menu bar means light glyphs.
        assert_eq!(bar_fill_for(None, dark_bar, false), BAR_BLUE);
        assert_eq!(bar_fill_for(None, light_bar, true), BAR_TINT);
        // Else the appearance.
        assert_eq!(bar_fill_for(None, None, true), BAR_BLUE);
        assert_eq!(bar_fill_for(None, None, false), BAR_TINT);
    }
}
