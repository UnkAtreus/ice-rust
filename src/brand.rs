//! The app's artwork, embedded at build time. `scripts/make-icons.py` makes
//! every file under `assets/` from `assets/logo-source.png`.

/// Runtime application icon and About page image, 512 x 512.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-512.png");
/// Menu bar glyph, black on clear, drawn as a template image.
pub const MENU_GLYPH_PNG: &[u8] = include_bytes!("../assets/menubar-template.png");
/// Filled glyph while a section is revealed or the Ice Bar is open.
pub const MENU_GLYPH_ACTIVE_PNG: &[u8] = include_bytes!("../assets/menubar-template-active.png");
/// Point size of the menu bar glyph; the PNGs are 3x.
pub const MENU_GLYPH_POINTS: f64 = 18.0;

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
}
