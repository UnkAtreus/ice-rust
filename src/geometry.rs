//! Coordinate conversion and frame math. CG space has its origin at the top
//! left of the primary display with y growing down; AppKit space has its
//! origin at the bottom left of the primary display with y growing up.

use crate::model::{
    ABUT_TOLERANCE, MAIN_MENU_LAYER, MENU_BAR_WINDOW_TITLE, Rect, STATUS_LAYER,
    WINDOW_SERVER_OWNER, WindowRecord,
};
use crate::sections::MenuBarState;

/// A display with its bounds in CG space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub id: u32,
    pub bounds: Rect,
}

pub fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
    if hi < lo {
        return lo;
    }
    value.max(lo).min(hi)
}

/// Converts a rectangle from CG space to AppKit space.
pub fn cg_to_appkit(rect: Rect, primary_height: f64) -> Rect {
    Rect::new(
        rect.x,
        primary_height - rect.y - rect.height,
        rect.width,
        rect.height,
    )
}

/// Converts a rectangle from AppKit space to CG space.
pub fn appkit_to_cg(rect: Rect, primary_height: f64) -> Rect {
    // The flip is its own inverse.
    cg_to_appkit(rect, primary_height)
}

/// Index of the first display whose bounds contain the point.
pub fn display_containing(displays: &[Display], x: f64, y: f64) -> Option<usize> {
    displays.iter().position(|d| d.bounds.contains(x, y))
}

/// Our icon's rectangle in CG space.
///
/// `cg_window` is the icon's CG window found by title, which needs Screen
/// Recording on macOS 26 (M4, M5). Without it, `appkit_frame` (the frame of
/// the icon button's window, read from AppKit with no TCC grant) stands in,
/// but only when it converts to a rectangle at the top edge of a display.
pub fn icon_rect(
    displays: &[Display],
    cg_window: Option<&Rect>,
    appkit_frame: Option<&Rect>,
    primary_height: f64,
) -> Option<Rect> {
    if let Some(r) = cg_window {
        return Some(*r);
    }
    let r = appkit_to_cg(*appkit_frame.filter(|f| !f.is_empty())?, primary_height);
    let at_top = displays.iter().any(|d| {
        d.bounds.contains(r.mid_x(), r.mid_y()) && (r.y - d.bounds.y).abs() <= ABUT_TOLERANCE
    });
    at_top.then_some(r)
}

/// Picks the display that shows the active menu bar.
///
/// The icon window titled with its autosave name exists only on the active
/// menu bar display (M5), so its center decides. Without it, the divider's
/// right edge decides: the visible section starts there, so it is on screen
/// even while the divider is expanded. The pointer is the last resort; it is
/// on the active menu bar right after a click on the icon.
pub fn active_display_index(
    displays: &[Display],
    icon: Option<&Rect>,
    divider: Option<&Rect>,
    pointer: Option<(f64, f64)>,
) -> Option<usize> {
    if let Some(i) = icon.and_then(|r| display_containing(displays, r.mid_x(), r.mid_y())) {
        return Some(i);
    }
    if let Some(i) = divider.and_then(|d| {
        let inset = (d.width / 2.0).min(0.5);
        display_containing(displays, d.max_x() - inset, d.mid_y())
    }) {
        return Some(i);
    }
    pointer.and_then(|(x, y)| display_containing(displays, x, y))
}

/// The active menu bar display and its menu bar height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuBarDisplay {
    pub display: Display,
    /// Height of the display's menu bar backdrop window, or of the icon or
    /// divider rectangle when the backdrop is not found.
    pub menu_bar_height: f64,
    /// True when the height came from the Window Server backdrop window.
    pub height_from_backdrop: bool,
}

/// Resolves the active menu bar display (see `active_display_index`) and
/// reads its menu bar height from the backdrop window (M11). All rectangles
/// and the pointer are in CG space.
pub fn menu_bar_display(
    windows: &[WindowRecord],
    displays: &[Display],
    icon: Option<&Rect>,
    divider: Option<&Rect>,
    pointer: Option<(f64, f64)>,
) -> Option<MenuBarDisplay> {
    let display = displays[active_display_index(displays, icon, divider, pointer)?];
    let backdrop = menu_bar_window(windows, &display.bounds);
    let menu_bar_height = backdrop
        .map(|w| w.bounds.height)
        .or_else(|| icon.or(divider).map(|r| r.height))?;
    Some(MenuBarDisplay {
        display,
        menu_bar_height,
        height_from_backdrop: backdrop.is_some(),
    })
}

/// Whether the divider sits left of the icon, as v1 requires. Both
/// rectangles are the active-display copies in CG space.
pub fn divider_left_of_icon(icon: &Rect, divider: &Rect) -> bool {
    divider.max_x() <= icon.min_x() + ABUT_TOLERANCE
}

/// Live order of the three control items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlOrder {
    /// The hidden divider sits left of the icon, so expanding it cannot
    /// push the icon off screen.
    pub hidden_left_of_icon: bool,
    /// The same for the always-hidden divider (true when there is none).
    pub always_hidden_left_of_icon: bool,
    /// The always-hidden divider sits left of the hidden divider (true
    /// when there is no always-hidden divider).
    pub dividers_in_order: bool,
}

impl ControlOrder {
    /// Whether `state` expands only dividers that sit left of the icon.
    pub fn allows(&self, state: MenuBarState) -> bool {
        let lengths = state.lengths();
        (!lengths.hidden_expanded || self.hidden_left_of_icon)
            && (!lengths.always_hidden_expanded || self.always_hidden_left_of_icon)
    }
}

/// Checks the live order of the control items. All rectangles are the
/// active-display copies in CG space.
pub fn control_order(icon: &Rect, hidden: &Rect, always_hidden: Option<&Rect>) -> ControlOrder {
    ControlOrder {
        hidden_left_of_icon: divider_left_of_icon(icon, hidden),
        always_hidden_left_of_icon: always_hidden.is_none_or(|a| divider_left_of_icon(icon, a)),
        dividers_in_order: always_hidden
            .is_none_or(|a| a.max_x() <= hidden.min_x() + ABUT_TOLERANCE),
    }
}

/// Whether the always-hidden divider sits left of every other status item
/// on its menu bar (v1.2, L1). `display` is the bounds of the display that
/// shows the divider (CG space); copies of items on other displays that
/// share the menu bar's y are outside it and do not count.
pub fn always_hidden_leftmost(
    windows: &[WindowRecord],
    divider: &WindowRecord,
    display: &Rect,
) -> bool {
    let d = &divider.bounds;
    !windows.iter().any(|w| {
        let b = &w.bounds;
        w.layer == STATUS_LAYER
            && w.id != divider.id
            && b.width > 0.0
            && (b.y - d.y).abs() <= ABUT_TOLERANCE
            && (b.height - d.height).abs() <= ABUT_TOLERANCE
            && display.contains(b.mid_x(), b.mid_y())
            && b.mid_x() < d.mid_x()
    })
}

/// The Window Server menu bar backdrop (layer 24) that covers the top of the
/// given display.
pub fn menu_bar_window<'a>(
    windows: &'a [WindowRecord],
    display: &Rect,
) -> Option<&'a WindowRecord> {
    const EDGE: f64 = 1.0;
    windows.iter().find(|w| {
        let named = match w.title_str() {
            Some(title) => title == MENU_BAR_WINDOW_TITLE,
            None => w.owner_name.as_deref() == Some(WINDOW_SERVER_OWNER),
        };
        w.layer == MAIN_MENU_LAYER
            && named
            && (w.bounds.x - display.x).abs() <= EDGE
            && (w.bounds.y - display.y).abs() <= EDGE
            && (w.bounds.width - display.width).abs() <= EDGE
    })
}

/// Ice Bar panel frame in AppKit space (M8).
///
/// `screen` is the display frame in AppKit space. The panel is centered under
/// `anchor_mid_x` and clamped to the screen. A panel wider than the screen is
/// narrowed to the screen width. Without an anchor the panel is right-aligned.
pub fn ice_bar_frame(
    screen: &Rect,
    menu_bar_height: f64,
    anchor_mid_x: Option<f64>,
    width: f64,
    height: f64,
) -> Rect {
    let width = width.min(screen.width);
    let x = match anchor_mid_x {
        Some(mid) => clamp(mid - width / 2.0, screen.min_x(), screen.max_x() - width),
        None => screen.max_x() - width,
    };
    let y = (screen.max_y() - 1.0) - menu_bar_height - height;
    Rect::new(x, y, width, height)
}

/// Pixel rectangle of `item` inside a composite image of `union`, both in
/// points, at `scale` pixels per point. Edges are rounded to whole pixels.
pub fn crop_rect(item: &Rect, union: &Rect, scale: f64) -> Rect {
    let left = ((item.min_x() - union.min_x()) * scale).round();
    let right = ((item.max_x() - union.min_x()) * scale).round();
    let top = ((item.min_y() - union.min_y()) * scale).round();
    let bottom = ((item.max_y() - union.min_y()) * scale).round();
    Rect::new(left, top, right - left, bottom - top)
}

/// Scale of a composite capture, or `None` when the image size does not
/// match the union of the captured windows.
pub fn composite_scale(image_width: usize, image_height: usize, union: &Rect) -> Option<f64> {
    if union.is_empty() || image_width == 0 || image_height == 0 {
        return None;
    }
    let scale = image_width as f64 / union.width;
    let expected_height = (union.height * scale).round();
    ((expected_height - image_height as f64).abs() <= 1.0).then_some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY_HEIGHT: f64 = 1440.0;

    // Display layout from the evidence host (CG space).
    fn displays() -> Vec<Display> {
        vec![
            Display {
                id: 4,
                bounds: Rect::new(0.0, 0.0, 2560.0, 1440.0),
            },
            Display {
                id: 1,
                bounds: Rect::new(-1512.0, 1080.0, 1512.0, 982.0),
            },
            Display {
                id: 3,
                bounds: Rect::new(-1920.0, 0.0, 1920.0, 1080.0),
            },
            Display {
                id: 5,
                bounds: Rect::new(2560.0, 0.0, 2560.0, 1440.0),
            },
        ]
    }

    fn win(id: u32, layer: i32, bounds: Rect, title: &str, owner: &str) -> WindowRecord {
        WindowRecord {
            id,
            layer,
            bounds,
            title: Some(title.to_string()),
            owner_name: Some(owner.to_string()),
            on_screen: true,
        }
    }

    fn item(id: u32, x: f64, width: f64, height: f64) -> WindowRecord {
        win(
            id,
            STATUS_LAYER,
            Rect::new(x, 0.0, width, height),
            "Item-0",
            "Control Center",
        )
    }

    #[test]
    fn always_hidden_divider_leftmost_on_its_display() {
        let display = Rect::new(0.0, 0.0, 2560.0, 1440.0);
        let divider = item(190, 2000.0, 20.0, 30.0);
        let mut all = vec![
            divider.clone(),
            item(188, 2020.0, 18.0, 30.0),
            item(52, 2038.0, 34.0, 30.0),
            item(186, 2072.0, 33.0, 30.0),
            // Off screen left of an expanded divider of another manager.
            item(312, -5532.0, 32.0, 30.0),
            // A copy on display 3 (x -1920), which shares the top y.
            item(7001, -400.0, 34.0, 30.0),
            // A copy on the notched display, another menu bar height.
            item(7002, 1990.0, 30.0, 33.0),
            // The menu bar backdrop is not a status item.
            win(
                9,
                MAIN_MENU_LAYER,
                Rect::new(0.0, 0.0, 2560.0, 30.0),
                "Menubar",
                "Window Server",
            ),
        ];
        assert!(always_hidden_leftmost(&all, &divider, &display));
        // An item left of the divider on the same menu bar.
        all.push(item(68, 1962.0, 38.0, 30.0));
        assert!(!always_hidden_leftmost(&all, &divider, &display));
        // A seed at 2 put it directly left of the hidden divider (v1.1).
        let v11 = vec![
            item(214, 1900.0, 34.0, 30.0),
            item(215, 1934.0, 32.0, 30.0),
            item(190, 1966.0, 20.0, 30.0),
            item(188, 1986.0, 18.0, 30.0),
        ];
        assert!(!always_hidden_leftmost(&v11, &v11[2], &display));
        // Alone on the menu bar.
        assert!(always_hidden_leftmost(
            std::slice::from_ref(&divider),
            &divider,
            &display
        ));
    }

    #[test]
    fn clamp_bounds() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(11.0, 0.0, 10.0), 10.0);
        // An inverted range resolves to the lower bound.
        assert_eq!(clamp(5.0, 10.0, 0.0), 10.0);
    }

    #[test]
    fn cg_to_appkit_matches_nsscreen_frames() {
        // Built-in display: NSScreen frame (-1512, -622, 1512, 982).
        let cg = Rect::new(-1512.0, 1080.0, 1512.0, 982.0);
        assert_eq!(
            cg_to_appkit(cg, PRIMARY_HEIGHT),
            Rect::new(-1512.0, -622.0, 1512.0, 982.0)
        );
        // Left display: NSScreen frame (-1920, 360, 1920, 1080).
        let cg = Rect::new(-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            cg_to_appkit(cg, PRIMARY_HEIGHT),
            Rect::new(-1920.0, 360.0, 1920.0, 1080.0)
        );
        // A menu bar item at the top of the primary display.
        let cg = Rect::new(100.0, 0.0, 33.0, 30.0);
        assert_eq!(
            cg_to_appkit(cg, PRIMARY_HEIGHT),
            Rect::new(100.0, 1410.0, 33.0, 30.0)
        );
    }

    #[test]
    fn conversion_round_trips() {
        let r = Rect::new(-466.0, 1080.0, 34.0, 33.0);
        assert_eq!(
            appkit_to_cg(cg_to_appkit(r, PRIMARY_HEIGHT), PRIMARY_HEIGHT),
            r
        );
    }

    #[test]
    fn display_lookup() {
        let d = displays();
        assert_eq!(display_containing(&d, 4621.5, 15.0), Some(3));
        assert_eq!(display_containing(&d, -10.0, 15.0), Some(2));
        assert_eq!(display_containing(&d, -10.0, 1100.0), Some(1));
        assert_eq!(display_containing(&d, -5000.0, 15.0), None);
    }

    #[test]
    fn active_display_prefers_icon() {
        let d = displays();
        let icon = Rect::new(4670.0, 0.0, 33.0, 30.0);
        let divider = Rect::new(-394.0, 0.0, 5016.0, 30.0);
        let pointer = Some((-10.0, 1100.0));
        assert_eq!(
            active_display_index(&d, Some(&icon), Some(&divider), pointer),
            Some(3)
        );
        // Icon unknown: the expanded divider's right edge is on display 5.
        assert_eq!(
            active_display_index(&d, None, Some(&divider), pointer),
            Some(3)
        );
        // Collapsed divider on the primary display.
        let collapsed = Rect::new(2000.0, 0.0, 16.0, 30.0);
        assert_eq!(
            active_display_index(&d, None, Some(&collapsed), None),
            Some(0)
        );
        // Neither known: the pointer decides.
        assert_eq!(active_display_index(&d, None, None, pointer), Some(1));
        assert_eq!(active_display_index(&d, None, None, None), None);
    }

    fn backdrops() -> Vec<WindowRecord> {
        vec![
            win(
                7094,
                MAIN_MENU_LAYER,
                Rect::new(-1512.0, 1080.0, 1512.0, 33.0),
                "Menubar",
                "Window Server",
            ),
            win(
                339,
                MAIN_MENU_LAYER,
                Rect::new(2560.0, 0.0, 2560.0, 30.0),
                "Menubar",
                "Window Server",
            ),
        ]
    }

    #[test]
    fn icon_rect_falls_back_to_appkit_frame() {
        let d = displays();
        let cg = Rect::new(4670.0, 0.0, 33.0, 30.0);
        // The same icon as an AppKit window frame on display 5.
        let appkit = Rect::new(4670.0, 1410.0, 33.0, 30.0);
        assert_eq!(
            icon_rect(&d, Some(&cg), Some(&appkit), PRIMARY_HEIGHT),
            Some(cg)
        );
        assert_eq!(icon_rect(&d, None, Some(&appkit), PRIMARY_HEIGHT), Some(cg));
        // Notched built-in display: AppKit frame (-466, 327, 34, 33).
        let built_in = Rect::new(-466.0, 327.0, 34.0, 33.0);
        assert_eq!(
            icon_rect(&d, None, Some(&built_in), PRIMARY_HEIGHT),
            Some(Rect::new(-466.0, 1080.0, 34.0, 33.0))
        );
        // A frame that is not at the top of any display, or empty, is not used.
        let bogus = Rect::new(0.0, 0.0, 33.0, 30.0);
        assert_eq!(icon_rect(&d, None, Some(&bogus), PRIMARY_HEIGHT), None);
        assert_eq!(
            icon_rect(&d, None, Some(&Rect::default()), PRIMARY_HEIGHT),
            None
        );
        assert_eq!(icon_rect(&d, None, None, PRIMARY_HEIGHT), None);
    }

    #[test]
    fn menu_bar_display_without_titles_uses_appkit_icon() {
        // No readable titles: no CG icon window and no divider. The backdrop
        // windows still match by owner name.
        let mut windows = backdrops();
        for w in &mut windows {
            w.title = None;
        }
        let d = displays();
        let appkit = Rect::new(4670.0, 1410.0, 33.0, 30.0);
        let icon = icon_rect(&d, None, Some(&appkit), PRIMARY_HEIGHT);
        let place = menu_bar_display(&windows, &d, icon.as_ref(), None, None).unwrap();
        assert_eq!(place.display.id, 5);
        assert_eq!(place.menu_bar_height, 30.0);
        assert!(place.height_from_backdrop);
        // No backdrop either: the icon rectangle gives the height.
        let place = menu_bar_display(&[], &d, icon.as_ref(), None, None).unwrap();
        assert_eq!(place.display.id, 5);
        assert_eq!(place.menu_bar_height, 30.0);
        assert!(!place.height_from_backdrop);
    }

    #[test]
    fn menu_bar_display_falls_back_to_pointer() {
        let d = displays();
        let place = menu_bar_display(&backdrops(), &d, None, None, Some((-100.0, 1090.0))).unwrap();
        assert_eq!(place.display.id, 1);
        assert_eq!(place.menu_bar_height, 33.0);
        // The pointer alone gives no height when the display has no backdrop.
        assert_eq!(
            menu_bar_display(&backdrops(), &d, None, None, Some((100.0, 10.0))),
            None
        );
        assert_eq!(menu_bar_display(&backdrops(), &d, None, None, None), None);
    }

    #[test]
    fn divider_order() {
        let icon = Rect::new(4670.0, 0.0, 33.0, 30.0);
        // Expanded or collapsed directly left of the icon.
        assert!(divider_left_of_icon(
            &icon,
            &Rect::new(-346.0, 0.0, 5016.0, 30.0)
        ));
        assert!(divider_left_of_icon(
            &icon,
            &Rect::new(4654.0, 0.0, 16.0, 30.0)
        ));
        // Other items between the divider and the icon are fine.
        assert!(divider_left_of_icon(
            &icon,
            &Rect::new(4600.0, 0.0, 16.0, 30.0)
        ));
        // The icon was dragged left of the divider.
        assert!(!divider_left_of_icon(
            &icon,
            &Rect::new(4703.0, 0.0, 16.0, 30.0)
        ));
        let pushed = Rect::new(-379.0, 0.0, 33.0, 30.0);
        assert!(!divider_left_of_icon(
            &pushed,
            &Rect::new(-346.0, 0.0, 5016.0, 30.0)
        ));
    }

    #[test]
    fn three_control_items_order() {
        let icon = Rect::new(4670.0, 0.0, 33.0, 30.0);
        let hidden = Rect::new(4654.0, 0.0, 16.0, 30.0);
        let always = Rect::new(4580.0, 0.0, 20.0, 30.0);
        let ok = ControlOrder {
            hidden_left_of_icon: true,
            always_hidden_left_of_icon: true,
            dividers_in_order: true,
        };
        assert_eq!(control_order(&icon, &hidden, Some(&always)), ok);
        assert_eq!(control_order(&icon, &hidden, None), ok);
        // Expanded dividers (Ice on the evidence host).
        assert_eq!(
            control_order(
                &icon,
                &Rect::new(-394.0, 0.0, 5016.0, 30.0),
                Some(&Rect::new(-5514.0, 0.0, 5016.0, 30.0))
            ),
            ok
        );
        // Always-hidden divider between the hidden divider and the icon.
        let swapped = Rect::new(4600.0, 0.0, 20.0, 30.0);
        let hidden_left = Rect::new(4500.0, 0.0, 16.0, 30.0);
        assert_eq!(
            control_order(&icon, &hidden_left, Some(&swapped)),
            ControlOrder {
                hidden_left_of_icon: true,
                always_hidden_left_of_icon: true,
                dividers_in_order: false,
            }
        );
        // Always-hidden divider right of the icon.
        let right_of_icon = Rect::new(4703.0, 0.0, 20.0, 30.0);
        assert_eq!(
            control_order(&icon, &hidden, Some(&right_of_icon)),
            ControlOrder {
                hidden_left_of_icon: true,
                always_hidden_left_of_icon: false,
                dividers_in_order: false,
            }
        );
    }

    #[test]
    fn order_decides_which_states_are_safe() {
        let ok = ControlOrder {
            hidden_left_of_icon: true,
            always_hidden_left_of_icon: true,
            dividers_in_order: true,
        };
        for state in [
            MenuBarState::AllHidden,
            MenuBarState::HiddenShown,
            MenuBarState::AllShown,
        ] {
            assert!(ok.allows(state), "{state:?}");
        }
        // Always-hidden divider right of the icon: showing only the hidden
        // section would expand it, so only everything shown is safe.
        let always_right = ControlOrder {
            always_hidden_left_of_icon: false,
            ..ok
        };
        assert!(!always_right.allows(MenuBarState::AllHidden));
        assert!(!always_right.allows(MenuBarState::HiddenShown));
        assert!(always_right.allows(MenuBarState::AllShown));
        // Hidden divider right of the icon: only states that keep it
        // collapsed are safe.
        let hidden_right = ControlOrder {
            hidden_left_of_icon: false,
            ..ok
        };
        assert!(!hidden_right.allows(MenuBarState::AllHidden));
        assert!(hidden_right.allows(MenuBarState::HiddenShown));
        assert!(hidden_right.allows(MenuBarState::AllShown));
    }

    #[test]
    fn menu_bar_window_per_display() {
        let windows = vec![
            win(
                7094,
                MAIN_MENU_LAYER,
                Rect::new(-1512.0, 1080.0, 1512.0, 33.0),
                "Menubar",
                "Window Server",
            ),
            win(
                6885,
                MAIN_MENU_LAYER,
                Rect::new(-1920.0, 0.0, 1920.0, 30.0),
                "Menubar",
                "Window Server",
            ),
            win(
                6876,
                MAIN_MENU_LAYER,
                Rect::new(0.0, 0.0, 2560.0, 30.0),
                "Menubar",
                "Window Server",
            ),
            win(
                339,
                MAIN_MENU_LAYER,
                Rect::new(2560.0, 0.0, 2560.0, 30.0),
                "Menubar",
                "Window Server",
            ),
            win(
                54,
                STATUS_LAYER,
                Rect::new(4978.0, 0.0, 142.0, 30.0),
                "Clock",
                "Control Center",
            ),
        ];
        let d = displays();
        assert_eq!(
            menu_bar_window(&windows, &d[3].bounds).map(|w| w.id),
            Some(339)
        );
        assert_eq!(
            menu_bar_window(&windows, &d[1].bounds).map(|w| w.bounds.height),
            Some(33.0)
        );
        assert_eq!(
            menu_bar_window(&windows, &d[0].bounds).map(|w| w.id),
            Some(6876)
        );
    }

    #[test]
    fn menu_bar_window_without_titles_uses_owner() {
        let mut w = win(
            339,
            MAIN_MENU_LAYER,
            Rect::new(2560.0, 0.0, 2560.0, 30.0),
            "",
            "Window Server",
        );
        w.title = None;
        let windows = vec![w];
        assert_eq!(
            menu_bar_window(&windows, &displays()[3].bounds).map(|w| w.id),
            Some(339)
        );
        // A layer-24 window from another owner with no title does not count.
        let mut other = windows[0].clone();
        other.owner_name = Some("Dock".into());
        assert_eq!(menu_bar_window(&[other], &displays()[3].bounds), None);
    }

    #[test]
    fn ice_bar_frame_centers_under_icon() {
        // Display 5 in AppKit space, icon at CG x 4670 w 33.
        let screen = Rect::new(2560.0, 0.0, 2560.0, 1440.0);
        let f = ice_bar_frame(&screen, 30.0, Some(4686.5), 200.0, 30.0);
        assert_eq!(f, Rect::new(4586.5, 1379.0, 200.0, 30.0));
    }

    #[test]
    fn ice_bar_frame_clamps_to_screen() {
        let screen = Rect::new(2560.0, 0.0, 2560.0, 1440.0);
        let right = ice_bar_frame(&screen, 30.0, Some(5100.0), 200.0, 30.0);
        assert_eq!(right.max_x(), 5120.0);
        let left = ice_bar_frame(&screen, 30.0, Some(2570.0), 200.0, 30.0);
        assert_eq!(left.min_x(), 2560.0);
        let wide = ice_bar_frame(&screen, 30.0, Some(3000.0), 4000.0, 30.0);
        assert_eq!(wide, Rect::new(2560.0, 1379.0, 2560.0, 30.0));
        let no_anchor = ice_bar_frame(&screen, 30.0, None, 200.0, 30.0);
        assert_eq!(no_anchor.max_x(), 5120.0);
    }

    #[test]
    fn ice_bar_frame_on_notched_secondary_display() {
        // Built-in display in AppKit space, 33 pt menu bar.
        let screen = Rect::new(-1512.0, -622.0, 1512.0, 982.0);
        let f = ice_bar_frame(&screen, 33.0, Some(-400.0), 100.0, 33.0);
        assert_eq!(f.y, (360.0 - 1.0) - 33.0 - 33.0);
        assert_eq!(f.x, -450.0);
    }

    #[test]
    fn crop_rect_scales_relative_to_union() {
        let union = Rect::new(-498.0, 0.0, 104.0, 30.0);
        let item = Rect::new(-466.0, 0.0, 34.0, 30.0);
        assert_eq!(
            crop_rect(&item, &union, 2.0),
            Rect::new(64.0, 0.0, 68.0, 60.0)
        );
        let first = Rect::new(-498.0, 0.0, 32.0, 30.0);
        assert_eq!(
            crop_rect(&first, &union, 1.0),
            Rect::new(0.0, 0.0, 32.0, 30.0)
        );
    }

    #[test]
    fn crop_rect_rounds_fractional_points() {
        let union = Rect::new(0.5, 0.0, 10.0, 30.0);
        let item = Rect::new(3.25, 0.0, 4.5, 30.0);
        // (3.25 - 0.5) * 2 = 5.5 -> 6; right edge (7.75 - 0.5) * 2 = 14.5 -> 15.
        assert_eq!(
            crop_rect(&item, &union, 2.0),
            Rect::new(6.0, 0.0, 9.0, 60.0)
        );
    }

    #[test]
    fn composite_scale_checks_both_axes() {
        let union = Rect::new(-498.0, 0.0, 104.0, 30.0);
        assert_eq!(composite_scale(208, 60, &union), Some(2.0));
        assert_eq!(composite_scale(104, 30, &union), Some(1.0));
        assert_eq!(composite_scale(208, 30, &union), None);
        assert_eq!(composite_scale(0, 0, &union), None);
        assert_eq!(composite_scale(10, 10, &Rect::default()), None);
    }
}
