//! The macOS 27 menu bar (v1.8). `MenuBarAgent` draws every status item in
//! one window per display, so there are no item windows to read, and it
//! drops a status item as wide as half a display instead of laying it out.
//! Items that do not fit fold, from the left, into the system overflow
//! control. Hiding uses that: the hidden divider and a spacer next to it
//! grow to the longest length every display lays out, and whatever no
//! longer fits folds (docs/macos27.md). Pure.

use std::collections::{HashMap, HashSet};

use crate::arrange::{Anchors, BarModel, Destination};
use crate::chain::SectionChains;
use crate::model::{
    ABUT_TOLERANCE, ALWAYS_HIDDEN_AUTOSAVE, ALWAYS_HIDDEN_SPACER_AUTOSAVE, CONTROL_CENTER_TITLE,
    ControlNames, DIVIDER_AUTOSAVE, ICON_AUTOSAVE, Rect, SPACER_AUTOSAVE, STATUS_LAYER, WindowId,
    WindowRecord,
};
use crate::sections::MenuBarState;

/// The first macOS major version with this menu bar.
pub const AGENT_MENU_BAR_OS: u32 = 27;
/// A status item's window is this much wider than its length.
pub const ITEM_CHROME: f64 = 16.0;
/// Kept off the exact half, so rounding cannot drop a divider.
pub const CAP_MARGIN: f64 = 2.0;
/// Wider than any collapsed divider (a chevron, about 26 to 36 pt).
pub const MAX_COLLAPSED_WIDTH: f64 = 100.0;
/// Seeded `Preferred Position` values on macOS 27: icon, hidden divider,
/// always-hidden divider. MenuBarAgent orders items by this value from the
/// trailing end and takes 0 for no value, so the macOS 26 seeds (0 and 1)
/// would put the icon at the far left, where it folds first. With 1 and 2
/// the icon is the rightmost status item and the divider sits left of it.
pub const AGENT_SEED_POSITIONS: [Option<f64>; 3] = [Some(1.0), Some(2.0), None];

/// One slot of MenuBarAgent's menu bar window, for `probe` (CG space).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSlot {
    pub frame: Rect,
    /// The process whose item fills the slot.
    pub pid: Option<i32>,
    /// The item's title, description or identifier.
    pub name: Option<String>,
    /// `AXIdentifier`, which Apple's own items carry
    /// (`com.apple.menuextra.clock`).
    pub identifier: Option<String>,
    /// The system overflow control rather than an item.
    pub overflow_control: bool,
}

/// Whether `slot` is folded into the overflow: folded slots are stacked
/// left of the overflow control (`overflow`).
pub fn is_folded(slot: &Rect, overflow: Option<&Rect>) -> bool {
    overflow.is_some_and(|o| slot.min_x() < o.min_x())
}

/// Identifier prefix of Apple's own items (Clock, Control Center, Wi-Fi),
/// which MenuBarAgent keeps at the trailing end; ice-rust does not move
/// them.
pub const APPLE_ITEM_PREFIX: &str = "com.apple.menuextra.";
/// How far a slot may be from our item's AppKit frame and still be it.
const OWN_FRAME_SLACK: f64 = 2.0;

fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Ids for `slots` (left to right) that stay the same from one read to the
/// next while the item's process and name do: there are no window ids on
/// macOS 27. Several items of one process with one name are numbered left
/// to right. Never 0, never repeated.
pub fn slot_ids(slots: &[AgentSlot]) -> Vec<WindowId> {
    let mut seen: HashMap<(Option<i32>, Option<&str>), u32> = HashMap::new();
    let mut used = HashSet::new();
    slots
        .iter()
        .map(|slot| {
            let label = slot.identifier.as_deref().or(slot.name.as_deref());
            let ordinal = seen.entry((slot.pid, label)).or_insert(0);
            let key = format!("{:?}|{}|{}", slot.pid, label.unwrap_or(""), ordinal);
            *ordinal += 1;
            let mut id = fnv1a(&key).max(1);
            while !used.insert(id) {
                id = id.wrapping_add(1).max(1);
            }
            id
        })
        .collect()
}

/// The two spacers. Each sits directly left of its divider and grows with
/// it: one divider covers at most half the display, two together almost
/// all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spacer {
    /// Left of the hidden divider; grows while everything is hidden.
    Hidden,
    /// Left of the always-hidden divider; grows while only the hidden
    /// section shows.
    AlwaysHidden,
}

impl Spacer {
    pub const ALL: [Spacer; 2] = [Spacer::Hidden, Spacer::AlwaysHidden];

    pub fn autosave(self) -> &'static str {
        match self {
            Spacer::Hidden => SPACER_AUTOSAVE,
            Spacer::AlwaysHidden => ALWAYS_HIDDEN_SPACER_AUTOSAVE,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Spacer::Hidden => 0,
            Spacer::AlwaysHidden => 1,
        }
    }
}

/// Our items' frames in CG space, from AppKit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OurFrames {
    pub icon: Option<Rect>,
    pub hidden: Option<Rect>,
    pub spacer: Option<Rect>,
    pub always_hidden: Option<Rect>,
    pub always_spacer: Option<Rect>,
}

/// One read of the macOS 27 menu bar, split into sections for Menu Bar
/// Layout.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentLayout {
    /// The items, our dividers and spacer left out.
    pub chains: SectionChains,
    pub anchors: Anchors,
    /// The spacers' ids, `Spacer::index` order, when on the bar.
    pub spacers: [Option<WindowId>; 2],
    /// Every slot on the bar by id, our items included, for moves.
    pub frames: Vec<(WindowId, Rect)>,
    /// Apple's items, which stay where MenuBarAgent puts them.
    pub apple: Vec<WindowId>,
    /// Items folded into the system overflow, which have no place to read.
    pub folded: usize,
}

impl AgentLayout {
    pub fn frame(&self, id: WindowId) -> Option<Rect> {
        self.frames.iter().find(|(i, _)| *i == id).map(|(_, f)| *f)
    }

    /// The bars, with Apple's items locked.
    pub fn model(&self, names: &ControlNames) -> BarModel {
        let mut model = BarModel::from_sections(&self.chains, names, self.anchors);
        for tile in model
            .visible
            .iter_mut()
            .chain(&mut model.hidden)
            .chain(&mut model.always_hidden)
        {
            if self.apple.contains(&tile.id) {
                tile.locked = true;
            }
        }
        model
    }

    fn divider(&self, spacer: Spacer) -> Option<WindowId> {
        match spacer {
            Spacer::Hidden => Some(self.anchors.hidden_divider),
            Spacer::AlwaysHidden => self.anchors.always_hidden_divider,
        }
    }

    /// Whether `spacer` sits directly left of its divider: no item between
    /// them in the bar's order. The order holds while MenuBarAgent animates
    /// a change, when frames show gaps and partial widths. `None` when
    /// either is not on the bar (folded, or not shown) or overlaps another
    /// slot (a folded item whose frame is not where it is), which says
    /// nothing.
    pub fn spacer_next_to_divider(&self, spacer: Spacer) -> Option<bool> {
        let index = |id: WindowId| self.frames.iter().position(|(i, _)| *i == id);
        let s = index(self.spacers[spacer.index()]?)?;
        let d = index(self.divider(spacer)?)?;
        if self.overlaps_another(s) || self.overlaps_another(d) {
            return None;
        }
        Some(s + 1 == d)
    }

    /// Whether slot `index` overlaps another slot; slots on the bar never
    /// do.
    fn overlaps_another(&self, index: usize) -> bool {
        let (_, frame) = self.frames[index];
        self.frames.iter().enumerate().any(|(j, (_, other))| {
            j != index
                && frame.min_x() < other.max_x() - ABUT_TOLERANCE
                && other.min_x() < frame.max_x() - ABUT_TOLERANCE
        })
    }

    /// The spacer's and its divider's frames, both on the bar.
    pub fn spacer_frames(&self, spacer: Spacer) -> Option<(Rect, Rect)> {
        Some((
            self.frame(self.spacers[spacer.index()]?)?,
            self.frame(self.divider(spacer)?)?,
        ))
    }

    /// Where the drag goes for `destination`: the end of a section is left
    /// of that divider's spacer, which must stay next to the divider.
    pub fn drag_destination(&self, destination: Destination) -> Destination {
        let Destination::LeftOf(target) = destination else {
            return destination;
        };
        Spacer::ALL
            .into_iter()
            .find(|&s| {
                self.divider(s) == Some(target) && self.spacer_next_to_divider(s) == Some(true)
            })
            .and_then(|s| self.spacers[s.index()])
            .map_or(destination, Destination::LeftOf)
    }
}

fn same_frame(slot: &Rect, ours: Option<&Rect>) -> bool {
    ours.is_some_and(|o| {
        (slot.x - o.x).abs() <= OWN_FRAME_SLACK && (slot.width - o.width).abs() <= OWN_FRAME_SLACK
    })
}

/// The slots of the one MenuBarAgent menu bar window to read on `display`
/// (CG space). `windows` holds each window's slots, in MenuBarAgent's
/// order. While a full-screen Space exists, MenuBarAgent keeps a second
/// window on the same display with the same slots, except that each of our
/// items there reports the icon's identifier (macOS 27.0.1, 2026-10-18).
/// Reading both put every item in the bars twice and matched the icon to a
/// divider, so the order check saw the icon left of the dividers. The
/// window to read is the one with our hidden divider's identifier; without
/// one, the first window with slots on `display`.
pub fn bar_slots(windows: &[Vec<AgentSlot>], own_pid: i32, display: &Rect) -> Vec<AgentSlot> {
    let on_display = |slots: &Vec<AgentSlot>| -> Vec<AgentSlot> {
        slots
            .iter()
            .filter(|s| {
                (s.frame.y - display.y).abs() <= 1.0
                    && s.frame.min_x() >= display.min_x() - 1.0
                    && s.frame.max_x() <= display.max_x() + 1.0
            })
            .cloned()
            .collect()
    };
    let mut candidates: Vec<Vec<AgentSlot>> = windows
        .iter()
        .map(on_display)
        .filter(|slots| !slots.is_empty())
        .collect();
    let live = candidates
        .iter()
        .position(|slots| holds_our_divider(slots, own_pid));
    match live {
        Some(index) => candidates.swap_remove(index),
        None => candidates.into_iter().next().unwrap_or_default(),
    }
}

/// Whether `slots`, one MenuBarAgent window, are the live ones: there our
/// hidden divider's slot carries its identifier (see `bar_slots`).
fn holds_our_divider(slots: &[AgentSlot], own_pid: i32) -> bool {
    slots
        .iter()
        .any(|s| s.pid == Some(own_pid) && s.identifier.as_deref() == Some(DIVIDER_AUTOSAVE))
}

/// Our icon's rectangle in CG space from MenuBarAgent: its slot in the live
/// window (see `bar_slots`). On macOS 27 it stands in when AppKit's frame
/// of the icon button puts the icon on no display; with an external display
/// on macOS 27.2 the menu bar display was not found that way (issue #4).
/// `None` without a live window or without our icon in it.
pub fn agent_icon_rect(windows: &[Vec<AgentSlot>], own_pid: i32) -> Option<Rect> {
    windows
        .iter()
        .find(|slots| holds_our_divider(slots, own_pid))?
        .iter()
        .find(|s| s.pid == Some(own_pid) && s.identifier.as_deref() == Some(ICON_AUTOSAVE))
        .map(|s| s.frame)
}

/// Splits MenuBarAgent's slots (one display) into the three sections by
/// our dividers, found among the slots of `own_pid` by their accessibility
/// identifier (the autosave name), else by their AppKit frames, which lag
/// behind a move. `None` when the hidden divider is not on the bar.
pub fn agent_layout(slots: &[AgentSlot], own_pid: i32, ours: &OurFrames) -> Option<AgentLayout> {
    let overflow = slots.iter().find(|s| s.overflow_control).map(|s| s.frame);
    let mut on_bar: Vec<AgentSlot> = slots
        .iter()
        .filter(|s| !s.overflow_control && !is_folded(&s.frame, overflow.as_ref()))
        .cloned()
        .collect();
    let folded = slots.len() - on_bar.len() - usize::from(overflow.is_some());
    on_bar.sort_by(|a, b| a.frame.x.total_cmp(&b.frame.x));
    let ids = slot_ids(&on_bar);
    let ours_at = |name: &str, frame: Option<&Rect>| {
        let own = || {
            on_bar
                .iter()
                .zip(&ids)
                .filter(|(s, _)| s.pid == Some(own_pid))
        };
        own()
            .find(|(s, _)| s.identifier.as_deref() == Some(name))
            .or_else(|| own().find(|(s, _)| same_frame(&s.frame, frame)))
            .map(|(s, id)| (*id, s.frame))
    };
    let (hidden_id, hidden) = ours_at(DIVIDER_AUTOSAVE, ours.hidden.as_ref())?;
    let icon = ours_at(ICON_AUTOSAVE, ours.icon.as_ref());
    let spacer = ours_at(SPACER_AUTOSAVE, ours.spacer.as_ref());
    let always = ours_at(ALWAYS_HIDDEN_AUTOSAVE, ours.always_hidden.as_ref());
    let always_spacer = ours_at(ALWAYS_HIDDEN_SPACER_AUTOSAVE, ours.always_spacer.as_ref());
    let skip = [
        Some(hidden_id),
        spacer.map(|s| s.0),
        always.map(|a| a.0),
        always_spacer.map(|s| s.0),
    ];

    let mut chains = SectionChains::default();
    let mut apple = Vec::new();
    for (slot, &id) in on_bar.iter().zip(&ids) {
        if skip.contains(&Some(id)) {
            continue;
        }
        let identifier = slot.identifier.as_deref().unwrap_or("");
        let is_apple = identifier.starts_with(APPLE_ITEM_PREFIX);
        if is_apple {
            apple.push(id);
        }
        // The names the macOS 26 rules know (`LOCKED_TITLES`).
        let title = match identifier {
            "com.apple.menuextra.clock" => "Clock".to_string(),
            "com.apple.menuextra.controlcenter" => CONTROL_CENTER_TITLE.to_string(),
            _ => slot.name.clone().unwrap_or_else(|| "Item".to_string()),
        };
        let record = WindowRecord {
            id,
            layer: STATUS_LAYER,
            bounds: slot.frame,
            title: Some(title),
            owner_name: None,
            on_screen: true,
        };
        let f = slot.frame;
        if f.min_x() >= hidden.max_x() - ABUT_TOLERANCE {
            chains.visible.push(record);
        } else if always.is_some_and(|(_, a)| f.max_x() <= a.min_x() + ABUT_TOLERANCE) {
            chains.always_hidden.push(record);
        } else {
            chains.hidden.push(record);
        }
    }
    Some(AgentLayout {
        chains,
        anchors: Anchors {
            icon: icon.map(|i| i.0),
            hidden_divider: hidden_id,
            always_hidden_divider: always.map(|a| a.0),
        },
        spacers: [spacer.map(|s| s.0), always_spacer.map(|s| s.0)],
        frames: on_bar
            .iter()
            .zip(&ids)
            .map(|(s, id)| (*id, s.frame))
            .collect(),
        apple,
        folded,
    })
}

/// Whether macOS `os_major` draws the menu bar through MenuBarAgent.
pub fn uses_agent_menu_bar(os_major: u32) -> bool {
    os_major >= AGENT_MENU_BAR_OS
}

/// The longest divider length every display lays out: the item's window
/// must stay within half the narrowest display (944 is laid out on a
/// 1920 pt display and 950 dropped; 848 on 1728). `None` without displays.
pub fn max_length(display_widths: &[f64]) -> Option<f64> {
    let narrowest = display_widths
        .iter()
        .copied()
        .filter(|w| w.is_finite() && *w > 0.0)
        .reduce(f64::min)?;
    Some(((narrowest / 2.0).floor() - ITEM_CHROME - CAP_MARGIN).max(0.0))
}

/// The spacer's seed: directly left of the hidden divider's value. Values
/// grow leftward, and the next item's value is at least the divider's
/// width larger.
pub fn spacer_seed(hidden_position: Option<f64>) -> f64 {
    hidden_position.or(AGENT_SEED_POSITIONS[1]).unwrap_or(2.0) + 1.0
}

/// Lengths for a state. `None` collapses a divider to its chevron; a
/// spacer is never collapsed, 0 leaves a blank 16 pt slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentLengths {
    pub hidden: Option<f64>,
    pub spacer: f64,
    pub always_hidden: Option<f64>,
    pub always_spacer: f64,
}

/// Divider and spacer lengths for `state`. `usable[i]` says spacer `i`
/// (`Spacer::index`) sits directly left of its divider; elsewhere it would
/// fold the items between them or, right of the divider, items meant to
/// show.
pub fn agent_lengths(state: MenuBarState, max_length: f64, usable: [bool; 2]) -> AgentLengths {
    let grown = |spacer: Spacer| {
        if usable[spacer.index()] {
            max_length
        } else {
            0.0
        }
    };
    match state {
        // The always-hidden divider stays grown too, so its items get no
        // room while the hidden divider grows (they showed for a moment
        // when it collapsed first). Everything left of the hidden divider
        // folds anyway.
        MenuBarState::AllHidden => AgentLengths {
            hidden: Some(max_length),
            spacer: grown(Spacer::Hidden),
            always_hidden: Some(max_length),
            always_spacer: grown(Spacer::AlwaysHidden),
        },
        MenuBarState::HiddenShown => AgentLengths {
            hidden: None,
            spacer: 0.0,
            always_hidden: Some(max_length),
            always_spacer: grown(Spacer::AlwaysHidden),
        },
        MenuBarState::AllShown => AgentLengths {
            hidden: None,
            spacer: 0.0,
            always_hidden: None,
            always_spacer: 0.0,
        },
    }
}

/// Whether `frame` shows a divider laid out collapsed. Right after a
/// length change AppKit still reports the old frame, and a folded item's
/// frame is not where it is, so only such a frame tells the order.
pub fn collapsed_divider(frame: &Rect) -> bool {
    frame.width > 0.0 && frame.width <= MAX_COLLAPSED_WIDTH
}

/// Whether `frame` shows the spacer at length 0.
pub fn collapsed_spacer(frame: &Rect) -> bool {
    frame.width > 0.0 && frame.width <= ITEM_CHROME + ABUT_TOLERANCE
}

/// Whether the spacer sits directly left of the hidden divider on the same
/// menu bar. Both frames must be read while both are laid out.
pub fn spacer_adjacent(spacer: &Rect, hidden: &Rect) -> bool {
    (spacer.max_x() - hidden.min_x()).abs() <= ABUT_TOLERANCE
        && (spacer.y - hidden.y).abs() <= ABUT_TOLERANCE
}

/// Our items `state` expands, by autosave name (and accessibility
/// identifier).
pub fn expanded_items(state: MenuBarState) -> &'static [&'static str] {
    match state {
        MenuBarState::AllHidden => &[DIVIDER_AUTOSAVE, SPACER_AUTOSAVE],
        MenuBarState::HiddenShown => &[ALWAYS_HIDDEN_AUTOSAVE, ALWAYS_HIDDEN_SPACER_AUTOSAVE],
        MenuBarState::AllShown => &[],
    }
}

/// The menu bar parts that hold an item, for the empty-space test: every
/// MenuBarAgent slot (the overflow control included) except our own
/// expanded items (`expanded`, by identifier), which look like empty space,
/// as they do on macOS 26 where they sit off screen.
pub fn occupied(slots: &[AgentSlot], own_pid: i32, expanded: &[&str]) -> Vec<Rect> {
    slots
        .iter()
        .filter(|slot| {
            !(slot.pid == Some(own_pid)
                && slot
                    .identifier
                    .as_deref()
                    .is_some_and(|id| expanded.contains(&id)))
        })
        .map(|slot| slot.frame)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_menu_bar_from_macos_27() {
        assert!(!uses_agent_menu_bar(26));
        assert!(uses_agent_menu_bar(27));
        assert!(uses_agent_menu_bar(28));
    }

    #[test]
    fn max_length_stays_under_half_the_narrowest_display() {
        // VM, macOS 27.0.1: 944 laid out on 1920, 950 dropped.
        assert_eq!(max_length(&[1920.0]), Some(942.0));
        assert!(max_length(&[1920.0]).unwrap() + ITEM_CHROME <= 960.0);
        // Stash: 848 hides on a 1728 pt display, 849 does not.
        assert!(max_length(&[1728.0]).unwrap() <= 848.0);
        // Two displays: the narrower one decides.
        assert_eq!(max_length(&[2560.0, 1512.0]), max_length(&[1512.0]));
        assert_eq!(max_length(&[]), None);
        assert_eq!(max_length(&[f64::NAN, 0.0]), None);
    }

    #[test]
    fn spacer_seed_is_left_of_the_divider() {
        assert_eq!(spacer_seed(Some(2.0)), 3.0);
        // A macOS 26 position carried over keeps the spacer next to it.
        assert_eq!(spacer_seed(Some(340.0)), 341.0);
        assert_eq!(spacer_seed(None), 3.0);
        // Icon right of the divider: the icon's value is smaller.
        let [icon, hidden, always] = AGENT_SEED_POSITIONS;
        assert!(icon.unwrap() > 0.0 && icon < hidden);
        assert_eq!(always, None);
    }

    #[test]
    fn lengths_per_state() {
        let all_hidden = agent_lengths(MenuBarState::AllHidden, 942.0, [true, true]);
        assert_eq!(all_hidden.hidden, Some(942.0));
        assert_eq!(all_hidden.spacer, 942.0);
        assert_eq!(all_hidden.always_hidden, Some(942.0));
        assert_eq!(all_hidden.always_spacer, 942.0);
        // A spacer away from its divider stays at 0.
        assert_eq!(
            agent_lengths(MenuBarState::AllHidden, 942.0, [false, true]).spacer,
            0.0
        );
        let hidden_shown = agent_lengths(MenuBarState::HiddenShown, 942.0, [true, true]);
        assert_eq!(hidden_shown.hidden, None);
        assert_eq!(hidden_shown.spacer, 0.0);
        assert_eq!(hidden_shown.always_hidden, Some(942.0));
        assert_eq!(hidden_shown.always_spacer, 942.0);
        assert_eq!(
            agent_lengths(MenuBarState::HiddenShown, 942.0, [true, false]).always_spacer,
            0.0
        );
        assert_eq!(
            agent_lengths(MenuBarState::AllShown, 942.0, [true, true]),
            AgentLengths {
                hidden: None,
                spacer: 0.0,
                always_hidden: None,
                always_spacer: 0.0,
            }
        );
    }

    #[test]
    fn spacer_adjacency_from_frames() {
        // VM: sp3 at 1598+36 directly left of div at 1634.
        let div = Rect::new(1634.0, 0.0, 36.0, 30.0);
        assert!(spacer_adjacent(&Rect::new(1598.0, 0.0, 36.0, 30.0), &div));
        // A 0-length spacer is a 16 pt slot.
        assert!(spacer_adjacent(&Rect::new(1618.0, 0.0, 16.0, 30.0), &div));
        // An item between them.
        assert!(!spacer_adjacent(&Rect::new(1562.0, 0.0, 36.0, 30.0), &div));
        // Right of the divider.
        assert!(!spacer_adjacent(&Rect::new(1670.0, 0.0, 16.0, 30.0), &div));
        // Another display's bar.
        assert!(!spacer_adjacent(
            &Rect::new(1618.0, -1080.0, 16.0, 30.0),
            &div
        ));
    }

    fn slot(x: f64, width: f64, pid: i32, name: &str, identifier: Option<&str>) -> AgentSlot {
        AgentSlot {
            frame: Rect::new(x, 0.0, width, 30.0),
            pid: Some(pid),
            name: (!name.is_empty()).then(|| name.to_string()),
            identifier: identifier.map(str::to_string),
            overflow_control: false,
        }
    }

    /// The VM's bar with every section shown (fresh install, 2026-10-16):
    /// IceRust 3084, the test app 2179, MenuBarAgent 419.
    fn vm_bar() -> Vec<AgentSlot> {
        vec![
            slot(792.0, 33.0, 3084, "", None),
            slot(825.0, 44.0, 2179, "D16", None),
            slot(869.0, 44.0, 2179, "D15", None),
            slot(1434.0, 36.0, 2179, "D1", None),
            slot(1594.0, 44.0, 646, "", None),
            slot(1638.0, 31.0, 464, "Spotlight", None),
            slot(1669.0, 16.0, 3084, "IceRust", None),
            slot(1685.0, 26.0, 3084, "", None),
            slot(1711.0, 34.0, 3084, "ice-rust", None),
            slot(
                1753.0,
                26.0,
                419,
                "Control Center",
                Some("com.apple.menuextra.controlcenter"),
            ),
            slot(
                1795.0,
                105.0,
                419,
                "Clock",
                Some("com.apple.menuextra.clock"),
            ),
        ]
    }

    fn vm_ours() -> OurFrames {
        OurFrames {
            icon: Some(Rect::new(1711.0, 0.0, 34.0, 30.0)),
            hidden: Some(Rect::new(1685.0, 0.0, 26.0, 30.0)),
            spacer: Some(Rect::new(1669.0, 0.0, 16.0, 30.0)),
            always_hidden: Some(Rect::new(792.0, 0.0, 33.0, 30.0)),
            always_spacer: None,
        }
    }

    #[test]
    fn slot_ids_are_stable_and_distinct() {
        let bar = vm_bar();
        let ids = slot_ids(&bar);
        assert_eq!(ids, slot_ids(&bar));
        assert!(ids.iter().all(|&id| id != 0));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        // Moving an item does not change any id.
        let mut moved = bar.clone();
        moved.swap(1, 2);
        let moved_ids = slot_ids(&moved);
        assert_eq!(moved_ids[1], ids[2]);
        assert_eq!(moved_ids[2], ids[1]);
        // Two unnamed items of one process are numbered left to right.
        let twins = [slot(10.0, 20.0, 7, "", None), slot(30.0, 20.0, 7, "", None)];
        let twin_ids = slot_ids(&twins);
        assert_ne!(twin_ids[0], twin_ids[1]);
    }

    #[test]
    fn layout_splits_the_vm_bar_into_sections() {
        let layout = agent_layout(&vm_bar(), 3084, &vm_ours()).unwrap();
        let titles = |chain: &[WindowRecord]| -> Vec<String> {
            chain
                .iter()
                .map(|w| w.title_str().unwrap_or("").to_string())
                .collect()
        };
        assert_eq!(titles(&layout.chains.always_hidden), Vec::<String>::new());
        assert_eq!(
            titles(&layout.chains.hidden),
            ["D16", "D15", "D1", "Item", "Spotlight"]
        );
        assert_eq!(
            titles(&layout.chains.visible),
            ["ice-rust", CONTROL_CENTER_TITLE, "Clock"]
        );
        let ids = slot_ids(&vm_bar());
        assert_eq!(layout.anchors.hidden_divider, ids[7]);
        assert_eq!(layout.anchors.icon, Some(ids[8]));
        assert_eq!(layout.anchors.always_hidden_divider, Some(ids[0]));
        assert_eq!(layout.spacers[0], Some(ids[6]));
        assert_eq!(layout.apple, vec![ids[9], ids[10]]);
        assert_eq!(layout.folded, 0);

        let names = ControlNames::ours();
        let model = layout.model(&names);
        assert!(model.visible.iter().all(|t| t.locked));
        assert!(model.hidden.iter().all(|t| !t.locked));
        // The end of the hidden section is left of the spacer.
        assert_eq!(
            layout.drag_destination(Destination::LeftOf(ids[7])),
            Destination::LeftOf(ids[6])
        );
        assert_eq!(
            layout.drag_destination(Destination::RightOf(ids[7])),
            Destination::RightOf(ids[7])
        );
        assert_eq!(
            layout.frame(ids[3]),
            Some(Rect::new(1434.0, 0.0, 36.0, 30.0))
        );
    }

    #[test]
    fn layout_finds_our_items_by_identifier_when_frames_lag() {
        // Right after a move AppKit still reports the old frames.
        let mut bar = vm_bar();
        for (index, name) in [
            (0, ALWAYS_HIDDEN_AUTOSAVE),
            (6, SPACER_AUTOSAVE),
            (7, DIVIDER_AUTOSAVE),
            (8, ICON_AUTOSAVE),
        ] {
            bar[index].identifier = Some(name.to_string());
        }
        let stale = OurFrames {
            icon: Some(Rect::new(1650.0, 0.0, 34.0, 30.0)),
            hidden: Some(Rect::new(1600.0, 0.0, 26.0, 30.0)),
            spacer: None,
            always_hidden: None,
            always_spacer: None,
        };
        let layout = agent_layout(&bar, 3084, &stale).unwrap();
        let ids = slot_ids(&bar);
        assert_eq!(layout.anchors.hidden_divider, ids[7]);
        assert_eq!(layout.anchors.icon, Some(ids[8]));
        assert_eq!(layout.spacers[0], Some(ids[6]));
        assert_eq!(layout.anchors.always_hidden_divider, Some(ids[0]));
        // Another process's item with our identifier is not ours.
        bar[7].pid = Some(1);
        assert!(agent_layout(&bar, 3084, &stale).is_none());
    }

    #[test]
    fn both_spacers_and_where_drags_go() {
        let mut bar = vm_bar();
        // The always-hidden spacer directly left of its divider (792).
        bar.insert(
            0,
            slot(776.0, 16.0, 3084, "", Some(ALWAYS_HIDDEN_SPACER_AUTOSAVE)),
        );
        bar[1].identifier = Some(ALWAYS_HIDDEN_AUTOSAVE.to_string());
        let layout = agent_layout(&bar, 3084, &vm_ours()).unwrap();
        let ids = slot_ids(&bar);
        let (always_spacer, always) = (ids[0], ids[1]);
        assert_eq!(layout.spacers, [Some(ids[7]), Some(always_spacer)]);
        assert_eq!(layout.spacer_next_to_divider(Spacer::Hidden), Some(true));
        assert_eq!(
            layout.spacer_next_to_divider(Spacer::AlwaysHidden),
            Some(true)
        );
        // Neither spacer becomes a tile.
        let model = layout.model(&ControlNames::ours());
        assert!(model.always_hidden.is_empty());
        assert_eq!(model.hidden.len(), 5);
        assert_eq!(
            layout.drag_destination(Destination::LeftOf(always)),
            Destination::LeftOf(always_spacer)
        );

        // The hidden spacer right of its divider (after MenuBarAgent
        // recomputed positions): drags go to the divider itself.
        let mut moved = vm_bar();
        moved[6].frame = Rect::new(1695.0, 0.0, 16.0, 30.0);
        moved[7].frame = Rect::new(1669.0, 0.0, 26.0, 30.0);
        let ours = OurFrames {
            hidden: Some(Rect::new(1669.0, 0.0, 26.0, 30.0)),
            spacer: Some(Rect::new(1695.0, 0.0, 16.0, 30.0)),
            ..vm_ours()
        };
        let layout = agent_layout(&moved, 3084, &ours).unwrap();
        assert_eq!(layout.spacer_next_to_divider(Spacer::Hidden), Some(false));
        let hidden = layout.anchors.hidden_divider;
        assert_eq!(
            layout.drag_destination(Destination::LeftOf(hidden)),
            Destination::LeftOf(hidden)
        );
        // No always-hidden spacer on the bar says nothing.
        assert_eq!(layout.spacer_next_to_divider(Spacer::AlwaysHidden), None);

        // Mid-animation: a gap between spacer and divider, same order.
        let named = || {
            let mut bar = vm_bar();
            bar[6].identifier = Some(SPACER_AUTOSAVE.to_string());
            bar[7].identifier = Some(DIVIDER_AUTOSAVE.to_string());
            bar
        };
        let mut animating = named();
        animating[6].frame = Rect::new(1670.0, 0.0, 12.0, 30.0);
        let layout = agent_layout(&animating, 3084, &vm_ours()).unwrap();
        assert_eq!(layout.spacer_next_to_divider(Spacer::Hidden), Some(true));

        // A folded spacer stacked on another slot says nothing.
        let mut stacked = named();
        stacked[6].frame = Rect::new(1640.0, 0.0, 16.0, 30.0);
        let layout = agent_layout(&stacked, 3084, &vm_ours()).unwrap();
        assert_eq!(layout.spacer_next_to_divider(Spacer::Hidden), None);
    }

    #[test]
    fn layout_leaves_folded_items_out() {
        let mut bar = vm_bar();
        bar.push(AgentSlot {
            frame: Rect::new(766.0, 1.0, 18.0, 27.0),
            pid: Some(419),
            name: None,
            identifier: None,
            overflow_control: true,
        });
        // Folded: stacked left of the overflow control.
        bar.push(slot(740.0, 44.0, 2179, "D17", None));
        bar.push(slot(740.0, 44.0, 2179, "D18", None));
        let layout = agent_layout(&bar, 3084, &vm_ours()).unwrap();
        assert_eq!(layout.folded, 2);
        assert!(
            layout
                .chains
                .hidden
                .iter()
                .all(|w| w.title_str() != Some("D17"))
        );
        // Without the hidden divider on the bar there is no layout.
        let mut ours = vm_ours();
        ours.hidden = Some(Rect::new(10.0, 0.0, 26.0, 30.0));
        assert!(agent_layout(&vm_bar(), 3084, &ours).is_none());
    }

    #[test]
    fn only_collapsed_frames_count() {
        // VM: a chevron 26 pt, an expanded divider 958 pt, the spacer at 0
        // is 16 pt and at the cap 958 pt.
        assert!(collapsed_divider(&Rect::new(1685.0, 0.0, 26.0, 30.0)));
        assert!(!collapsed_divider(&Rect::new(753.0, 0.0, 958.0, 30.0)));
        assert!(!collapsed_divider(&Rect::default()));
        assert!(collapsed_spacer(&Rect::new(1669.0, 0.0, 16.0, 30.0)));
        assert!(!collapsed_spacer(&Rect::new(150.0, 0.0, 958.0, 30.0)));
    }

    #[test]
    fn folded_slots_sit_left_of_the_overflow_control() {
        // VM: overflow control at 428, D15 stacked at 401, D8 on the bar.
        let overflow = Rect::new(428.0, 1.0, 18.0, 27.0);
        assert!(is_folded(
            &Rect::new(401.0, 0.0, 44.0, 30.0),
            Some(&overflow)
        ));
        assert!(!is_folded(
            &Rect::new(453.0, 0.0, 38.0, 30.0),
            Some(&overflow)
        ));
        assert!(!is_folded(&Rect::new(401.0, 0.0, 44.0, 30.0), None));
    }

    #[test]
    fn our_expanded_items_count_as_empty_space() {
        let mut overflow = slot(429.0, 18.0, 419, "", None);
        overflow.overflow_control = true;
        let slots = [
            overflow,
            slot(454.0, 256.0, 7, "", Some(SPACER_AUTOSAVE)),
            slot(710.0, 960.0, 7, "", Some(DIVIDER_AUTOSAVE)),
            slot(1670.0, 44.0, 646, "", None),
        ];
        let frames = |rects: Vec<Rect>| rects;
        assert_eq!(
            occupied(&slots, 7, expanded_items(MenuBarState::AllHidden)),
            frames(vec![slots[0].frame, slots[3].frame])
        );
        // Shown, the collapsed items are items like any other.
        assert_eq!(
            occupied(&slots, 7, expanded_items(MenuBarState::AllShown)).len(),
            4
        );
        // Another process's item with our identifier is not ours.
        assert_eq!(
            occupied(&slots, 8, expanded_items(MenuBarState::AllHidden)).len(),
            4
        );
        assert_eq!(
            expanded_items(MenuBarState::HiddenShown),
            [ALWAYS_HIDDEN_AUTOSAVE, ALWAYS_HIDDEN_SPACER_AUTOSAVE]
        );
    }

    /// IceRust's process in the bars below.
    const OWN: i32 = 4548;

    fn main_display() -> Rect {
        Rect::new(0.0, 0.0, 1616.0, 1010.0)
    }

    /// The live menu bar window in the VM with every section shown
    /// (macOS 27.0.1, 2026-10-18).
    fn live_window() -> Vec<AgentSlot> {
        vec![
            slot(1234.0, 16.0, OWN, "", Some(ALWAYS_HIDDEN_SPACER_AUTOSAVE)),
            slot(1250.0, 33.0, OWN, "<<", Some(ALWAYS_HIDDEN_AUTOSAVE)),
            slot(1283.0, 44.0, 671, "", None),
            slot(1327.0, 31.0, 505, "Search", None),
            slot(1358.0, 16.0, OWN, "", Some(SPACER_AUTOSAVE)),
            slot(1374.0, 26.0, OWN, "<", Some(DIVIDER_AUTOSAVE)),
            slot(1400.0, 34.0, OWN, "ice-rust", Some(ICON_AUTOSAVE)),
            slot(1442.0, 26.0, 450, "Control Center", None),
            slot(1484.0, 112.0, 450, "Clock", None),
        ]
    }

    /// The second window MenuBarAgent keeps on the same display while a
    /// full-screen Space exists: the same slots, but each of our items
    /// reports the icon's identifier.
    fn full_screen_copy() -> Vec<AgentSlot> {
        live_window()
            .into_iter()
            .map(|mut s| {
                if s.pid == Some(OWN) {
                    s.name = Some("IceRust".to_string());
                    s.identifier = Some(ICON_AUTOSAVE.to_string());
                }
                s
            })
            .collect()
    }

    fn live_ours() -> OurFrames {
        OurFrames {
            icon: Some(Rect::new(1400.0, 0.0, 34.0, 30.0)),
            hidden: Some(Rect::new(1374.0, 0.0, 26.0, 30.0)),
            spacer: Some(Rect::new(1358.0, 0.0, 16.0, 30.0)),
            always_hidden: Some(Rect::new(1250.0, 0.0, 33.0, 30.0)),
            always_spacer: Some(Rect::new(1234.0, 0.0, 16.0, 30.0)),
        }
    }

    #[test]
    fn bar_slots_leave_the_full_screen_copy_out() {
        // MenuBarAgent lists the copy first while the full-screen Space is
        // active and second otherwise; the live window wins either way.
        for windows in [
            vec![full_screen_copy(), live_window()],
            vec![live_window(), full_screen_copy()],
        ] {
            let slots = bar_slots(&windows, OWN, &main_display());
            assert_eq!(slots, live_window());
            let layout = agent_layout(&slots, OWN, &live_ours()).unwrap();
            let icon = layout.anchors.icon.and_then(|id| layout.frame(id)).unwrap();
            assert_eq!(icon, Rect::new(1400.0, 0.0, 34.0, 30.0));
            let hidden = layout.frame(layout.anchors.hidden_divider).unwrap();
            let always = layout
                .anchors
                .always_hidden_divider
                .and_then(|id| layout.frame(id));
            let order = crate::geometry::control_order(&icon, &hidden, always.as_ref());
            assert!(order.hidden_left_of_icon && order.always_hidden_left_of_icon);
            // One tile per item: nothing from the copy.
            assert_eq!(layout.frames.len(), live_window().len());
        }
    }

    #[test]
    fn bar_slots_without_our_identifiers_take_the_first_window() {
        let plain = |window: Vec<AgentSlot>| -> Vec<AgentSlot> {
            window
                .into_iter()
                .map(|mut s| {
                    s.identifier = None;
                    s
                })
                .collect()
        };
        let first = plain(live_window());
        let mut second = plain(live_window());
        second[2].name = Some("Other".to_string());
        let windows = vec![first.clone(), second];
        assert_eq!(bar_slots(&windows, OWN, &main_display()), first);
    }

    #[test]
    fn bar_slots_keep_only_the_display() {
        let other_display = live_window()
            .into_iter()
            .map(|mut s| {
                s.frame.y = -1080.0;
                s
            })
            .collect::<Vec<_>>();
        let windows = vec![other_display, live_window()];
        assert_eq!(bar_slots(&windows, OWN, &main_display()), live_window());
        // A slot that runs past the display's right edge is not on it.
        let mut spill = live_window();
        spill.push(slot(1600.0, 40.0, 450, "Spill", None));
        assert_eq!(bar_slots(&[spill], OWN, &main_display()), live_window());
        assert!(bar_slots(&[], OWN, &main_display()).is_empty());
    }

    /// The live window and its copy shifted onto another display: the
    /// external display (x 1512) shows our live items, the built-in one the
    /// copy whose items all report the icon's identifier.
    fn two_display_windows() -> Vec<Vec<AgentSlot>> {
        let shift = |window: Vec<AgentSlot>, dx: f64| -> Vec<AgentSlot> {
            window
                .into_iter()
                .map(|mut s| {
                    s.frame.x += dx;
                    s
                })
                .collect()
        };
        vec![
            shift(full_screen_copy(), -1000.0),
            shift(live_window(), 1512.0),
        ]
    }

    #[test]
    fn agent_icon_rect_comes_from_the_live_window() {
        let rect = agent_icon_rect(&two_display_windows(), OWN).unwrap();
        assert_eq!(rect, Rect::new(1400.0 + 1512.0, 0.0, 34.0, 30.0));
        // The order MenuBarAgent lists the windows in does not matter.
        let mut reversed = two_display_windows();
        reversed.reverse();
        assert_eq!(agent_icon_rect(&reversed, OWN), Some(rect));
    }

    #[test]
    fn agent_icon_rect_needs_the_live_window_and_our_icon() {
        // Only the copy: every item claims to be the icon, none is trusted.
        assert_eq!(agent_icon_rect(&[full_screen_copy()], OWN), None);
        // Another process's items.
        assert_eq!(agent_icon_rect(&[live_window()], OWN + 1), None);
        // The live window without our icon (folded away).
        let without_icon: Vec<AgentSlot> = live_window()
            .into_iter()
            .filter(|s| s.identifier.as_deref() != Some(ICON_AUTOSAVE))
            .collect();
        assert_eq!(agent_icon_rect(&[without_icon], OWN), None);
        assert_eq!(agent_icon_rect(&[], OWN), None);
    }
}
