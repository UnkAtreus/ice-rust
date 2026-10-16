//! Section membership from window geometry (M6). Status item windows abut
//! exactly, so the hidden section is the run of windows that ends at the
//! divider's left edge, and the visible section is the run that starts at its
//! right edge.
//!
//! Other displays' copies of status items can share the divider's y and
//! height and overlap its x range, so geometry alone can pick one. Left of an
//! expanded divider every item is off screen, so the walk skips on-screen
//! windows there.
//!
//! With an always-hidden divider (v1.1) the hidden section is the run left
//! of the hidden divider up to the always-hidden divider, and the
//! always-hidden section is the run left of the always-hidden divider.

use crate::model::{
    ABUT_TOLERANCE, DIVIDER_MIN_WIDTH, MAX_CHAIN_STEPS, Rect, STATUS_LAYER, WindowId, WindowRecord,
};

/// Result of looking up a divider window by title.
#[derive(Clone, Debug, PartialEq)]
pub enum DividerLookup {
    Found(WindowRecord),
    /// No status window carries the title.
    NotFound,
    /// No status window has a readable title, which means the process lacks
    /// Screen Recording.
    TitlesUnreadable,
    /// Fewer than `MIN_ITEM_WINDOWS` status windows exist: the menu bar
    /// draws its items without windows (macOS 27), so none can be read.
    NoItemWindows,
}

/// The clock and Control Center alone give every menu bar before macOS 27
/// at least this many layer-25 windows. On macOS 27 there is at most one,
/// an empty window of ice-rust's own.
pub const MIN_ITEM_WINDOWS: usize = 2;

/// Shown instead of the items when the lookup finds `NoItemWindows`.
pub const MSG_NO_ITEM_WINDOWS: &str =
    "Not available on macOS 27 yet; Command-drag items in the menu bar instead";

/// All three sections, each in left-to-right order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionChains {
    pub visible: Vec<WindowRecord>,
    pub hidden: Vec<WindowRecord>,
    pub always_hidden: Vec<WindowRecord>,
}

/// Finds the layer-25 window titled `divider_title`. When several match, the
/// one whose y and height match the window titled `icon_title` wins.
pub fn find_divider(
    windows: &[WindowRecord],
    divider_title: &str,
    icon_title: &str,
) -> DividerLookup {
    let matches: Vec<&WindowRecord> = windows
        .iter()
        .filter(|w| w.layer == STATUS_LAYER && w.title_str() == Some(divider_title))
        .collect();
    match matches.as_slice() {
        [] => {
            let status = windows.iter().filter(|w| w.layer == STATUS_LAYER).count();
            let readable = windows
                .iter()
                .any(|w| w.layer == STATUS_LAYER && w.title_str().is_some());
            if status < MIN_ITEM_WINDOWS {
                DividerLookup::NoItemWindows
            } else if readable {
                DividerLookup::NotFound
            } else {
                DividerLookup::TitlesUnreadable
            }
        }
        [only] => DividerLookup::Found((*only).clone()),
        [first, ..] => {
            let on_icon_bar = find_titled(windows, icon_title)
                .and_then(|icon| matches.iter().find(|d| same_bar(&d.bounds, &icon.bounds)));
            DividerLookup::Found((*on_icon_bar.unwrap_or(first)).clone())
        }
    }
}

/// `find_divider`, then, when no window carries the title although titles
/// are readable, the layer-25 window at `frame`: the divider's own window
/// frame from AppKit in CG space, which needs no title (issue #1).
pub fn find_divider_or_frame(
    windows: &[WindowRecord],
    divider_title: &str,
    icon_title: &str,
    frame: Option<&Rect>,
) -> DividerLookup {
    match find_divider(windows, divider_title, icon_title) {
        DividerLookup::NotFound => match frame.and_then(|f| find_by_frame(windows, f)) {
            Some(window) => DividerLookup::Found(window.clone()),
            None => DividerLookup::NotFound,
        },
        other => other,
    }
}

/// Finds the layer-25 window whose bounds match `frame` within
/// `ABUT_TOLERANCE` on every side.
pub fn find_by_frame<'a>(windows: &'a [WindowRecord], frame: &Rect) -> Option<&'a WindowRecord> {
    let close = |a: f64, b: f64| (a - b).abs() <= ABUT_TOLERANCE;
    windows.iter().find(|w| {
        w.layer == STATUS_LAYER
            && close(w.bounds.x, frame.x)
            && close(w.bounds.y, frame.y)
            && close(w.bounds.width, frame.width)
            && close(w.bounds.height, frame.height)
    })
}

/// Finds the layer-25 window titled `title`.
pub fn find_titled<'a>(windows: &'a [WindowRecord], title: &str) -> Option<&'a WindowRecord> {
    windows
        .iter()
        .find(|w| w.layer == STATUS_LAYER && w.title_str() == Some(title))
}

/// Same menu bar: matching y and height.
fn same_bar(a: &Rect, b: &Rect) -> bool {
    (a.y - b.y).abs() <= ABUT_TOLERANCE && (a.height - b.height).abs() <= ABUT_TOLERANCE
}

type Edge = fn(&Rect) -> f64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
}

/// Walks the abutting run from the divider's edge. The walk stops at a gap,
/// at the window `stop` whatever its width, and at any window at least
/// `DIVIDER_MIN_WIDTH` wide.
///
/// Another display's copy can touch the cursor at the same edge as the
/// real neighbor. So the stop checks look at every touching window before
/// one is picked: a run that ends early drops items, while a run that
/// continues through copies adds items that belong to no section. Where
/// the section is shown, on-screen windows win over off-screen ones; the
/// lowest window id breaks the last tie.
fn walk(
    windows: &[WindowRecord],
    divider: &WindowRecord,
    direction: Direction,
    stop: Option<WindowId>,
) -> Vec<WindowRecord> {
    let d = &divider.bounds;
    let expanded = d.width >= DIVIDER_MIN_WIDTH;
    // Left of an expanded divider an on-screen window is another display's
    // copy.
    let skip_on_screen = direction == Direction::Left && expanded;
    // Left of a collapsed divider the section is shown, and right of the
    // hidden divider is the visible section, so the real items are on
    // screen there.
    let prefer_on_screen = direction == Direction::Right || !expanded;
    let candidates: Vec<&WindowRecord> = windows
        .iter()
        .filter(|w| {
            w.layer == STATUS_LAYER
                && w.id != divider.id
                && w.bounds.width > 0.0
                && same_bar(&w.bounds, d)
                && !(skip_on_screen && w.on_screen)
        })
        .collect();
    let mut used = vec![false; candidates.len()];

    // The edge of a candidate that must touch the cursor, and the edge that
    // becomes the next cursor.
    let (touching, far): (Edge, Edge) = match direction {
        Direction::Left => (Rect::max_x, Rect::min_x),
        Direction::Right => (Rect::min_x, Rect::max_x),
    };
    let mut cursor = match direction {
        Direction::Left => d.min_x(),
        Direction::Right => d.max_x(),
    };

    let mut chain = Vec::new();
    for _ in 0..MAX_CHAIN_STEPS {
        let mut abutting: Vec<(usize, &WindowRecord)> = candidates
            .iter()
            .enumerate()
            .filter(|(i, w)| !used[*i] && (touching(&w.bounds) - cursor).abs() <= ABUT_TOLERANCE)
            .map(|(i, w)| (i, *w))
            .collect();
        if abutting.iter().any(|(_, w)| Some(w.id) == stop) {
            break;
        }
        if prefer_on_screen && abutting.iter().any(|(_, w)| w.on_screen) {
            abutting.retain(|(_, w)| w.on_screen);
        }
        if abutting
            .iter()
            .any(|(_, w)| w.bounds.width >= DIVIDER_MIN_WIDTH)
        {
            break;
        }
        let best = abutting.into_iter().min_by(|(_, a), (_, b)| {
            let da = (touching(&a.bounds) - cursor).abs();
            let db = (touching(&b.bounds) - cursor).abs();
            da.total_cmp(&db).then(a.id.cmp(&b.id))
        });
        let Some((index, window)) = best else { break };
        used[index] = true;
        cursor = far(&window.bounds);
        chain.push(window.clone());
    }
    if direction == Direction::Left {
        chain.reverse();
    }
    chain
}

/// Items left of the divider, left to right.
pub fn hidden_chain(windows: &[WindowRecord], divider: &WindowRecord) -> Vec<WindowRecord> {
    walk(windows, divider, Direction::Left, None)
}

/// Items left of the divider up to, not including, the window `stop`.
pub fn hidden_chain_until(
    windows: &[WindowRecord],
    divider: &WindowRecord,
    stop: Option<WindowId>,
) -> Vec<WindowRecord> {
    walk(windows, divider, Direction::Left, stop)
}

/// Items right of the divider, left to right. Meant for the hidden
/// divider, whose right side is the visible section: on ties the walk
/// prefers on-screen windows.
pub fn visible_chain(windows: &[WindowRecord], divider: &WindowRecord) -> Vec<WindowRecord> {
    walk(windows, divider, Direction::Right, None)
}

/// The three sections. The hidden walk stops at the always-hidden divider
/// by window id, collapsed or expanded. Without an always-hidden divider the
/// always-hidden section is empty.
pub fn section_chains(
    windows: &[WindowRecord],
    hidden_divider: &WindowRecord,
    always_hidden_divider: Option<&WindowRecord>,
) -> SectionChains {
    SectionChains {
        visible: visible_chain(windows, hidden_divider),
        hidden: hidden_chain_until(windows, hidden_divider, always_hidden_divider.map(|d| d.id)),
        always_hidden: always_hidden_divider
            .map(|d| hidden_chain(windows, d))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MAIN_MENU_LAYER;

    fn w(
        id: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        title: &str,
        on_screen: bool,
    ) -> WindowRecord {
        WindowRecord {
            id,
            layer: STATUS_LAYER,
            bounds: Rect::new(x, y, width, height),
            title: Some(title.to_string()),
            owner_name: Some("Control Center".into()),
            on_screen,
        }
    }

    /// Window list modeled on the evidence host (runtime snapshot RT1).
    /// Display 5 (x 2560..5120, y 0) is the active menu bar display. Ice's
    /// hidden divider (188) is expanded to 5016 pt; its hidden items sit at
    /// negative x between the always-hidden divider (190) and 188, where they
    /// overlap the live items of display 3 (x -1920..0, also at y 0).
    fn evidence() -> Vec<WindowRecord> {
        vec![
            // Display 5, active copies.
            w(214, -5538.0, 0.0, 34.0, 30.0, "", false),
            w(
                190,
                -5514.0,
                0.0,
                5016.0,
                30.0,
                "Ice.ControlItem.AlwaysHidden",
                false,
            ),
            w(312, -498.0, 0.0, 32.0, 30.0, "Item-0", false),
            w(52, -466.0, 0.0, 34.0, 30.0, "AirDrop", false),
            w(68, -432.0, 0.0, 38.0, 30.0, "Item-0", false),
            w(
                188,
                -394.0,
                0.0,
                5016.0,
                30.0,
                "Ice.ControlItem.Hidden",
                false,
            ),
            w(7090, 4622.0, 0.0, 48.0, 30.0, "AudioVideoModule", true),
            w(
                186,
                4670.0,
                0.0,
                33.0,
                30.0,
                "Ice.ControlItem.Visible",
                true,
            ),
            w(446, 4703.0, 0.0, 38.0, 30.0, "Item-0", true),
            w(8288, 4741.0, 0.0, 33.0, 30.0, "NowPlaying", true),
            w(170, 4774.0, 0.0, 44.0, 30.0, "Item-0", true),
            w(55, 4818.0, 0.0, 38.0, 30.0, "Sound", true),
            w(53, 4856.0, 0.0, 42.0, 30.0, "Battery", true),
            w(56, 4898.0, 0.0, 38.0, 30.0, "WiFi", true),
            w(57, 4936.0, 0.0, 42.0, 30.0, "BentoBox-0", true),
            w(54, 4978.0, 0.0, 142.0, 30.0, "Clock", true),
            // Display 3 replicas: same y, overlapping x.
            w(
                289,
                -5466.0,
                0.0,
                5016.0,
                30.0,
                "com.jordanbaird.Ice",
                false,
            ),
            w(288, -450.0, 0.0, 33.0, 30.0, "com.jordanbaird.Ice", true),
            w(448, -417.0, 0.0, 38.0, 30.0, "com.example.AppA", true),
            w(8290, -379.0, 0.0, 33.0, 30.0, "", true),
            w(286, -346.0, 0.0, 44.0, 30.0, "com.example.AppB", true),
            w(231, -142.0, 0.0, 142.0, 30.0, "", true),
            // Display 4 replicas: same y, other x.
            w(
                6961,
                -2906.0,
                0.0,
                5016.0,
                30.0,
                "com.jordanbaird.Ice",
                false,
            ),
            w(6960, 2110.0, 0.0, 33.0, 30.0, "com.jordanbaird.Ice", true),
            w(6966, 2143.0, 0.0, 38.0, 30.0, "com.example.AppA", true),
            // Display 1 replicas: other y and height, x abutting 188's edges.
            w(
                6991,
                -5464.0,
                1080.0,
                5016.0,
                33.0,
                "com.jordanbaird.Ice",
                false,
            ),
            w(6993, -428.0, 1080.0, 34.0, 33.0, "com.example.AppC", false),
            w(6999, 4622.0, 1080.0, 40.0, 33.0, "", false),
        ]
    }

    fn ids(chain: &[WindowRecord]) -> Vec<u32> {
        chain.iter().map(|w| w.id).collect()
    }

    fn divider(windows: &[WindowRecord]) -> WindowRecord {
        windows.iter().find(|w| w.id == 188).unwrap().clone()
    }

    #[test]
    fn hidden_chain_matches_evidence() {
        let windows = evidence();
        let d = divider(&windows);
        // Left to right; the walk stops at the always-hidden divider (190).
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![312, 52, 68]);
    }

    #[test]
    fn visible_chain_matches_evidence() {
        let windows = evidence();
        let d = divider(&windows);
        assert_eq!(
            ids(&visible_chain(&windows, &d)),
            vec![7090, 186, 446, 8288, 170, 55, 53, 56, 57, 54]
        );
    }

    #[test]
    fn replicas_never_join() {
        let windows = evidence();
        let d = divider(&windows);
        let hidden = ids(&hidden_chain(&windows, &d));
        let visible = ids(&visible_chain(&windows, &d));
        for replica in [
            289, 288, 448, 8290, 286, 231, 6961, 6960, 6966, 6991, 6993, 6999,
        ] {
            assert!(!hidden.contains(&replica), "{replica} joined hidden");
            assert!(!visible.contains(&replica), "{replica} joined visible");
        }
    }

    /// Read-only listing on the evidence host with display 5 active: its
    /// off-screen hidden copies interleave with display 3's on-screen
    /// replicas at y 0, and their edges fall within 1 pt of each other.
    fn interleaved() -> Vec<WindowRecord> {
        vec![
            w(
                6900,
                -308.0,
                0.0,
                5016.0,
                30.0,
                "IceRust.ControlItem.Hidden",
                false,
            ),
            w(6964, -412.0, 0.0, 32.0, 30.0, "Item-0", false),
            w(6936, -380.0, 0.0, 34.0, 30.0, "AirDrop", false),
            w(6948, -346.0, 0.0, 38.0, 30.0, "Item-0", false),
            // Display 3 replicas, on screen.
            w(290, -450.0, 0.0, 38.0, 30.0, "", true),
            w(288, -412.0, 0.0, 33.0, 30.0, "com.jordanbaird.Ice", true),
            w(448, -379.0, 0.0, 38.0, 30.0, "com.example.AppA", true),
            w(286, -341.0, 0.0, 44.0, 30.0, "com.example.AppB", true),
        ]
    }

    #[test]
    fn on_screen_replicas_do_not_extend_hidden_chain() {
        let windows = interleaved();
        let d = windows[0].clone();
        // Without the on-screen rule, replica 290 (max x -412) would join at
        // the end of the chain.
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![6964, 6936, 6948]);
    }

    #[test]
    fn on_screen_replica_loses_exact_tie() {
        let mut windows = interleaved();
        // A lower-id replica whose right edge equals a hidden item's.
        windows.push(w(250, -414.0, 0.0, 34.0, 30.0, "", true));
        let d = windows[0].clone();
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![6964, 6936, 6948]);
    }

    #[test]
    fn collapsed_divider_keeps_on_screen_hidden_items() {
        // Revealed: the hidden items are on screen left of the chevron.
        let windows = vec![
            w(1, 4600.0, 0.0, 30.0, 30.0, "a", true),
            w(2, 4630.0, 0.0, 24.0, 30.0, "b", true),
            w(
                3,
                4654.0,
                0.0,
                16.0,
                30.0,
                "IceRust.ControlItem.Hidden",
                true,
            ),
        ];
        let d = windows[2].clone();
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![1, 2]);
    }

    #[test]
    fn visible_chain_keeps_off_screen_items() {
        // Right of Ice's expanded always-hidden divider (190) sit its hidden
        // items, off screen, up to the hidden divider (188).
        let windows = evidence();
        let d = windows.iter().find(|w| w.id == 190).unwrap().clone();
        assert_eq!(ids(&visible_chain(&windows, &d)), vec![312, 52, 68]);
    }

    #[test]
    fn foreign_expanded_divider_stops_walk() {
        let mut windows = evidence();
        // Remove the always-hidden divider: 214 no longer abuts anything, so
        // the walk ends at 312 either way. Put a foreign 5016 pt divider
        // directly left of 312 instead.
        windows.retain(|w| w.id != 190);
        windows.push(w(9000, -5514.0, 0.0, 5016.0, 30.0, "Other.Divider", false));
        windows.push(w(9001, -5548.0, 0.0, 34.0, 30.0, "Item-0", false));
        let d = divider(&windows);
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![312, 52, 68]);
    }

    #[test]
    fn divider_width_threshold_is_inclusive() {
        let windows = vec![
            w(1, -1000.0, 0.0, 1000.0, 30.0, "wide", false),
            w(2, -1.0, 0.0, 999.0, 30.0, "narrow", false),
            w(3, 998.0, 0.0, 5016.0, 30.0, "div", false),
        ];
        let d = windows[2].clone();
        // 999 pt joins; the 1000 pt window stops the walk.
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![2]);
    }

    #[test]
    fn gap_stops_walk() {
        let mut windows = evidence();
        windows.retain(|w| w.id != 52);
        let d = divider(&windows);
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![68]);

        let mut windows = evidence();
        windows.retain(|w| w.id != 8288);
        let d = divider(&windows);
        assert_eq!(ids(&visible_chain(&windows, &d)), vec![7090, 186, 446]);
    }

    #[test]
    fn tolerance_accepts_small_offsets() {
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let windows = vec![
            d.clone(),
            // max_x -0.5 is within 1 pt of the divider's min_x 0.
            w(1, -30.5, 0.0, 30.0, 30.0, "a", false),
            // y and height off by 0.5 still count as the same menu bar.
            w(2, -60.5, 0.5, 30.0, 29.5, "b", false),
        ];
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![2, 1]);
    }

    #[test]
    fn tolerance_rejects_larger_offsets() {
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let windows = vec![
            d.clone(),
            w(1, -31.5, 0.0, 30.0, 30.0, "gap 1.5", false),
            w(2, 5016.0, 1.5, 30.0, 30.0, "y off 1.5", true),
            w(3, 5016.0, 0.0, 30.0, 31.5, "h off 1.5", true),
        ];
        assert!(hidden_chain(&windows, &d).is_empty());
        assert!(visible_chain(&windows, &d).is_empty());
    }

    #[test]
    fn closest_candidate_wins_a_tie() {
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let windows = vec![
            d.clone(),
            w(1, -30.8, 0.0, 30.0, 30.0, "off 0.8", false),
            w(2, -30.0, 0.0, 30.0, 30.0, "exact", false),
            w(3, -30.3, 0.0, 30.0, 30.0, "off 0.3", false),
        ];
        let hidden = hidden_chain(&windows, &d);
        assert_eq!(hidden.last().map(|w| w.id), Some(2));

        let windows = vec![
            d.clone(),
            w(1, 5016.9, 0.0, 30.0, 30.0, "off 0.9", true),
            w(2, 5016.2, 0.0, 30.0, 30.0, "off 0.2", true),
        ];
        assert_eq!(visible_chain(&windows, &d).first().map(|w| w.id), Some(2));
    }

    #[test]
    fn exact_tie_is_deterministic() {
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let a = vec![
            d.clone(),
            w(7, -30.0, 0.0, 30.0, 30.0, "a", false),
            w(3, -30.0, 0.0, 30.0, 30.0, "b", false),
        ];
        let b = vec![
            d.clone(),
            w(3, -30.0, 0.0, 30.0, 30.0, "b", false),
            w(7, -30.0, 0.0, 30.0, 30.0, "a", false),
        ];
        let first_a = hidden_chain(&a, &d).last().map(|w| w.id);
        let first_b = hidden_chain(&b, &d).last().map(|w| w.id);
        assert_eq!(first_a, Some(3));
        assert_eq!(first_a, first_b);
    }

    #[test]
    fn ignores_other_layers_and_zero_width() {
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let mut menubar = w(1, -30.0, 0.0, 30.0, 30.0, "Menubar", true);
        menubar.layer = MAIN_MENU_LAYER;
        let windows = vec![d.clone(), menubar, w(2, 0.0, 0.0, 0.0, 30.0, "zero", false)];
        assert!(hidden_chain(&windows, &d).is_empty());
    }

    #[test]
    fn walk_is_capped() {
        let d = w(100_000, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let mut windows = vec![d.clone()];
        for i in 0..300u32 {
            windows.push(w(
                i,
                -(f64::from(i) + 1.0) * 10.0,
                0.0,
                10.0,
                30.0,
                "x",
                false,
            ));
        }
        assert_eq!(hidden_chain(&windows, &d).len(), MAX_CHAIN_STEPS);
    }

    /// Everything shown: both dividers collapsed to chevrons, all items on
    /// screen at y 0 on one display.
    fn collapsed_sections() -> Vec<WindowRecord> {
        vec![
            w(40, 4500.0, 0.0, 40.0, 30.0, "ExampleMenu", true),
            w(41, 4540.0, 0.0, 40.0, 30.0, "Item-1", true),
            w(
                3,
                4580.0,
                0.0,
                20.0,
                30.0,
                "IceRust.ControlItem.AlwaysHidden",
                true,
            ),
            w(10, 4600.0, 0.0, 30.0, 30.0, "ExampleStatus", true),
            w(11, 4630.0, 0.0, 24.0, 30.0, "AirDrop", true),
            w(
                2,
                4654.0,
                0.0,
                16.0,
                30.0,
                "IceRust.ControlItem.Hidden",
                true,
            ),
            w(
                1,
                4670.0,
                0.0,
                33.0,
                30.0,
                "IceRust.ControlItem.Visible",
                true,
            ),
            w(54, 4703.0, 0.0, 142.0, 30.0, "Clock", true),
        ]
    }

    fn by_id(windows: &[WindowRecord], id: u32) -> WindowRecord {
        windows.iter().find(|w| w.id == id).unwrap().clone()
    }

    #[test]
    fn three_sections_with_collapsed_dividers() {
        let windows = collapsed_sections();
        let hidden = by_id(&windows, 2);
        let always = by_id(&windows, 3);
        let s = section_chains(&windows, &hidden, Some(&always));
        assert_eq!(ids(&s.visible), vec![1, 54]);
        // The narrow always-hidden divider stops the hidden walk by id.
        assert_eq!(ids(&s.hidden), vec![10, 11]);
        assert_eq!(ids(&s.always_hidden), vec![40, 41]);
    }

    #[test]
    fn without_always_hidden_divider_the_walk_runs_through() {
        let windows = collapsed_sections();
        let hidden = by_id(&windows, 2);
        let s = section_chains(&windows, &hidden, None);
        // The chevron is an ordinary narrow window to this walk.
        assert_eq!(ids(&s.hidden), vec![40, 41, 3, 10, 11]);
        assert!(s.always_hidden.is_empty());
    }

    #[test]
    fn three_sections_with_expanded_dividers() {
        // Ice's dividers from the evidence host: 190 (always-hidden) and 188
        // (hidden), both 5016 pt. Always-hidden items sit left of 190.
        let mut windows = evidence();
        windows.push(w(9100, -5560.0, 0.0, 46.0, 30.0, "ExampleMenu", false));
        let hidden = divider(&windows);
        let always = by_id(&windows, 190);
        let s = section_chains(&windows, &hidden, Some(&always));
        assert_eq!(ids(&s.hidden), vec![312, 52, 68]);
        assert_eq!(ids(&s.always_hidden), vec![9100]);
        assert_eq!(
            ids(&s.visible),
            vec![7090, 186, 446, 8288, 170, 55, 53, 56, 57, 54]
        );
    }

    #[test]
    fn hidden_shown_always_hidden_expanded() {
        // Hidden divider collapsed, always-hidden divider expanded with its
        // right edge on the leftmost hidden item.
        let mut windows = collapsed_sections();
        windows.retain(|w| w.id != 3 && w.id != 40 && w.id != 41);
        windows.push(w(
            3,
            -416.0,
            0.0,
            5016.0,
            30.0,
            "IceRust.ControlItem.AlwaysHidden",
            false,
        ));
        windows.push(w(40, -462.0, 0.0, 46.0, 30.0, "ExampleMenu", false));
        let hidden = by_id(&windows, 2);
        let always = by_id(&windows, 3);
        let s = section_chains(&windows, &hidden, Some(&always));
        assert_eq!(ids(&s.hidden), vec![10, 11]);
        assert_eq!(ids(&s.always_hidden), vec![40]);
    }

    #[test]
    fn stop_id_ends_walk_before_that_window() {
        let windows = collapsed_sections();
        let hidden = by_id(&windows, 2);
        assert_eq!(
            ids(&hidden_chain_until(&windows, &hidden, Some(11))),
            Vec::<u32>::new()
        );
        assert_eq!(
            ids(&hidden_chain_until(&windows, &hidden, Some(41))),
            vec![3, 10, 11]
        );
    }

    /// Read-only probe on the development host (macOS 26.3) with display 3
    /// (x -1920..0, y 0) active and Ice's dividers expanded. Off-screen
    /// copies titled with bundle ids touch the chain at two edges: 6957
    /// ends at -5532 like the always-hidden divider 9125, and 6964 starts
    /// at -412 like Ice's icon 9117.
    fn display_3_active() -> Vec<WindowRecord> {
        let off = |id, x, width, title| w(id, x, 0.0, width, 30.0, title, false);
        let on = |id, x, width, title| w(id, x, 0.0, width, 30.0, title, true);
        vec![
            off(9125, -10548.0, 5016.0, "Ice.ControlItem.AlwaysHidden"),
            off(8092, -10620.0, 40.0, "Item-0"),
            off(164, -10580.0, 32.0, "Item-0"),
            off(9121, -5428.0, 5016.0, "Ice.ControlItem.Hidden"),
            off(312, -5532.0, 32.0, "Item-0"),
            off(52, -5500.0, 34.0, "AirDrop"),
            off(68, -5466.0, 38.0, "Item-0"),
            // Copies that mirror the always-hidden items.
            off(6954, -5646.0, 38.0, "com.example.AppD"),
            off(6951, -5608.0, 32.0, "com.example.AppE"),
            off(6957, -5576.0, 44.0, "com.apple.TextInputMenuAgent"),
            // Copies at the visible section's edge.
            off(6964, -412.0, 32.0, "com.example.AppF"),
            off(6936, -380.0, 34.0, ""),
            off(6948, -346.0, 38.0, "com.example.AppG"),
            on(9117, -412.0, 33.0, "Ice.ControlItem.Visible"),
            on(446, -379.0, 38.0, "Item-0"),
            on(170, -341.0, 44.0, "Item-0"),
            on(55, -297.0, 38.0, "Sound"),
            on(53, -259.0, 42.0, "Battery"),
            on(56, -217.0, 38.0, "WiFi"),
            on(57, -179.0, 42.0, "BentoBox-0"),
            on(54, -137.0, 137.0, "Clock"),
        ]
    }

    #[test]
    fn copies_tied_with_dividers_and_icon_stay_out() {
        let windows = display_3_active();
        let hidden = by_id(&windows, 9121);
        let always = by_id(&windows, 9125);
        let s = section_chains(&windows, &hidden, Some(&always));
        // 6957 ties with the always-hidden divider; the stop id wins.
        assert_eq!(ids(&s.hidden), vec![312, 52, 68]);
        // 6964 ties with the icon; the on-screen icon wins.
        assert_eq!(ids(&s.visible), vec![9117, 446, 170, 55, 53, 56, 57, 54]);
        assert_eq!(ids(&s.always_hidden), vec![8092, 164]);
        // Without the always-hidden divider's id the walk still stops,
        // because a divider-wide window touches the edge.
        assert_eq!(ids(&hidden_chain(&windows, &hidden)), vec![312, 52, 68]);
    }

    #[test]
    fn wide_window_tied_with_an_item_ends_the_walk() {
        // 2 and 3 both end at -30. Stopping there drops 3; continuing
        // could run on through another display's copies.
        let d = w(100, 0.0, 0.0, 5016.0, 30.0, "div", false);
        let windows = vec![
            d.clone(),
            w(1, -30.0, 0.0, 30.0, 30.0, "a", false),
            w(
                2,
                -5046.0,
                0.0,
                5016.0,
                30.0,
                "other display's divider",
                false,
            ),
            w(3, -60.0, 0.0, 30.0, 30.0, "b", false),
        ];
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![1]);
    }

    #[test]
    fn collapsed_divider_prefers_on_screen_items() {
        // Revealed hidden section: an off-screen copy with a lower id
        // touches the chevron at the same edge as the real item.
        let windows = vec![
            w(5, 4600.0, 0.0, 30.0, 30.0, "copy", false),
            w(10, 4600.0, 0.0, 30.0, 30.0, "a", true),
            w(
                3,
                4630.0,
                0.0,
                16.0,
                30.0,
                "IceRust.ControlItem.Hidden",
                true,
            ),
        ];
        let d = by_id(&windows, 3);
        assert_eq!(ids(&hidden_chain(&windows, &d)), vec![10]);
    }

    #[test]
    fn find_divider_by_title() {
        let windows = evidence();
        match find_divider(
            &windows,
            "Ice.ControlItem.Hidden",
            "Ice.ControlItem.Visible",
        ) {
            DividerLookup::Found(d) => assert_eq!(d.id, 188),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            find_divider(&windows, "Nope", "Ice.ControlItem.Visible"),
            DividerLookup::NotFound
        );
    }

    #[test]
    fn find_divider_prefers_icon_menu_bar() {
        let windows = vec![
            w(
                1,
                -5000.0,
                1080.0,
                5016.0,
                33.0,
                "IceRust.ControlItem.Hidden",
                false,
            ),
            w(
                2,
                -394.0,
                0.0,
                5016.0,
                30.0,
                "IceRust.ControlItem.Hidden",
                false,
            ),
            w(
                3,
                4670.0,
                0.0,
                33.0,
                30.0,
                "IceRust.ControlItem.Visible",
                true,
            ),
        ];
        match find_divider(
            &windows,
            "IceRust.ControlItem.Hidden",
            "IceRust.ControlItem.Visible",
        ) {
            DividerLookup::Found(d) => assert_eq!(d.id, 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn find_divider_reports_unreadable_titles() {
        let mut windows = evidence();
        for win in &mut windows {
            win.title = None;
        }
        assert_eq!(
            find_divider(
                &windows,
                "Ice.ControlItem.Hidden",
                "Ice.ControlItem.Visible"
            ),
            DividerLookup::TitlesUnreadable
        );
    }

    #[test]
    fn find_divider_reports_a_menu_bar_without_item_windows() {
        // Issue #2, macOS 27.0.1: with every permission granted, the only
        // layer-25 window is an untitled one of ice-rust's own.
        let mut windows = vec![w(9, 1636.0, 0.0, 0.0, 0.0, "", true)];
        windows[0].title = None;
        let lookup = find_divider(
            &windows,
            "IceRust.ControlItem.Hidden",
            "IceRust.ControlItem.Visible",
        );
        assert_eq!(lookup, DividerLookup::NoItemWindows);
        assert_eq!(
            find_divider(
                &[],
                "IceRust.ControlItem.Hidden",
                "IceRust.ControlItem.Visible"
            ),
            DividerLookup::NoItemWindows
        );
        let frame = Rect::new(1636.0, 0.0, 0.0, 0.0);
        assert_eq!(
            find_divider_or_frame(
                &windows,
                "IceRust.ControlItem.Hidden",
                "IceRust.ControlItem.Visible",
                Some(&frame),
            ),
            DividerLookup::NoItemWindows
        );
    }

    #[test]
    fn divider_falls_back_to_its_frame_when_the_title_differs() {
        // Issue #1: titles are readable, but no window carries the divider's
        // autosave name. The divider's own frame still finds it.
        let windows = vec![
            w(1, -3034.0, 0.0, 5016.0, 30.0, "Item-1", false),
            w(2, 1982.0, 0.0, 34.0, 30.0, "Item-0", true),
            w(3, 2016.0, 0.0, 40.0, 30.0, "Clock", true),
        ];
        let frame = Rect::new(-3034.0, 0.0, 5016.0, 30.0);
        match find_divider_or_frame(
            &windows,
            "IceRust.ControlItem.Hidden",
            "IceRust.ControlItem.Visible",
            Some(&frame),
        ) {
            DividerLookup::Found(d) => assert_eq!(d.id, 1),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            find_divider_or_frame(
                &windows,
                "IceRust.ControlItem.Hidden",
                "IceRust.ControlItem.Visible",
                None,
            ),
            DividerLookup::NotFound
        );
        let elsewhere = Rect::new(-2000.0, 0.0, 5016.0, 30.0);
        assert_eq!(
            find_divider_or_frame(
                &windows,
                "IceRust.ControlItem.Hidden",
                "IceRust.ControlItem.Visible",
                Some(&elsewhere),
            ),
            DividerLookup::NotFound
        );
    }

    #[test]
    fn title_wins_over_frame_and_unreadable_titles_stay_unreadable() {
        let windows = evidence();
        let other = by_id(&windows, 54).bounds;
        match find_divider_or_frame(
            &windows,
            "Ice.ControlItem.Hidden",
            "Ice.ControlItem.Visible",
            Some(&other),
        ) {
            DividerLookup::Found(d) => assert_eq!(d.id, 188),
            other => panic!("unexpected {other:?}"),
        }
        let mut untitled = evidence();
        for win in &mut untitled {
            win.title = None;
        }
        let divider = by_id(&windows, 188).bounds;
        assert_eq!(
            find_divider_or_frame(
                &untitled,
                "Ice.ControlItem.Hidden",
                "Ice.ControlItem.Visible",
                Some(&divider),
            ),
            DividerLookup::TitlesUnreadable
        );
    }

    #[test]
    fn find_by_frame_allows_rounding_and_ignores_other_layers() {
        let mut windows = vec![w(7, 100.0, 0.0, 34.0, 30.0, "x", true)];
        assert_eq!(
            find_by_frame(&windows, &Rect::new(100.5, 0.4, 34.0, 30.0)).map(|w| w.id),
            Some(7)
        );
        assert!(find_by_frame(&windows, &Rect::new(103.0, 0.0, 34.0, 30.0)).is_none());
        windows[0].layer = MAIN_MENU_LAYER;
        assert!(find_by_frame(&windows, &Rect::new(100.0, 0.0, 34.0, 30.0)).is_none());
    }

    #[test]
    fn find_titled_ignores_other_layers() {
        let mut windows = evidence();
        windows[5].layer = MAIN_MENU_LAYER;
        assert!(find_titled(&windows, "Ice.ControlItem.Hidden").is_none());
        assert_eq!(find_titled(&windows, "Clock").map(|w| w.id), Some(54));
    }
}
