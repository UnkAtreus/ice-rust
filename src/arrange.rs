//! Menu Bar Layout model (v1.1): three bars of item tiles, the drop target
//! rule, drop-point geometry for the move engine, and the placement check.
//! Everything here is pure.

use std::time::Duration;

use crate::chain::SectionChains;
use crate::model::{
    ABUT_TOLERANCE, CONTROL_CENTER_TITLE, ControlNames, LOCKED_TITLES, MOVE_DROP_INSET,
    MOVE_DROP_NUDGE, Rect, WindowId, WindowRecord,
};
use crate::sections::Section;

/// One item in a bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: WindowId,
    pub title: String,
    /// Bounds in CG space when the model was built.
    pub bounds: Rect,
    /// Locked tiles cannot be dragged but still act as drop neighbors.
    pub locked: bool,
}

/// The control items the bars stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchors {
    pub icon: Option<WindowId>,
    pub hidden_divider: WindowId,
    pub always_hidden_divider: Option<WindowId>,
}

/// The three bars, each left to right like the menu bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BarModel {
    pub visible: Vec<Tile>,
    pub hidden: Vec<Tile>,
    pub always_hidden: Vec<Tile>,
    pub anchors: Anchors,
}

/// Where an item should go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    LeftOf(WindowId),
    RightOf(WindowId),
}

impl Destination {
    pub fn target(self) -> WindowId {
        match self {
            Destination::LeftOf(id) | Destination::RightOf(id) => id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropOutcome {
    /// The drop leaves the order as it was.
    NoChange,
    Move(Destination),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropError {
    UnknownItem,
    Locked,
    /// No neighbor and no divider to place against, for example a drop into
    /// the always-hidden bar without an always-hidden divider.
    NoAnchor,
    /// The destination would leave the target section.
    CrossesDivider,
}

impl std::fmt::Display for DropError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            DropError::UnknownItem => "the item is no longer in the menu bar",
            DropError::Locked => "this item cannot be moved",
            DropError::NoAnchor => "the always-hidden divider is missing",
            DropError::CrossesDivider => "that position is outside the section",
        };
        f.write_str(text)
    }
}

fn is_locked(window: &WindowRecord, anchors: &Anchors) -> bool {
    Some(window.id) == anchors.icon
        || window
            .title_str()
            .is_some_and(|title| LOCKED_TITLES.contains(&title))
}

fn is_system_locked(tile: &Tile) -> bool {
    LOCKED_TITLES.contains(&tile.title.as_str())
}

impl BarModel {
    /// Builds the bars from the walked sections. Dividers never become
    /// tiles; the icon, Clock and Control Center become locked tiles.
    pub fn from_sections(chains: &SectionChains, names: &ControlNames, anchors: Anchors) -> Self {
        let tiles = |chain: &[WindowRecord]| -> Vec<Tile> {
            chain
                .iter()
                .filter(|w| {
                    w.id != anchors.hidden_divider
                        && Some(w.id) != anchors.always_hidden_divider
                        && w.title_str()
                            .is_none_or(|t| t != names.hidden && t != names.always_hidden)
                })
                .map(|w| Tile {
                    id: w.id,
                    title: w.title_str().unwrap_or("Item").to_string(),
                    bounds: w.bounds,
                    locked: is_locked(w, &anchors),
                })
                .collect()
        };
        Self {
            visible: tiles(&chains.visible),
            hidden: tiles(&chains.hidden),
            always_hidden: tiles(&chains.always_hidden),
            anchors,
        }
    }

    pub fn bar(&self, section: Section) -> &[Tile] {
        match section {
            Section::Visible => &self.visible,
            Section::Hidden => &self.hidden,
            Section::AlwaysHidden => &self.always_hidden,
        }
    }

    /// The section and index of the tile with `id`.
    pub fn find(&self, id: WindowId) -> Option<(Section, usize)> {
        Section::ALL.iter().find_map(|&section| {
            self.bar(section)
                .iter()
                .position(|t| t.id == id)
                .map(|i| (section, i))
        })
    }

    pub fn tile(&self, id: WindowId) -> Option<&Tile> {
        self.find(id).map(|(section, i)| &self.bar(section)[i])
    }

    /// Left-to-right ids of a bar.
    pub fn ids(&self, section: Section) -> Vec<WindowId> {
        self.bar(section).iter().map(|t| t.id).collect()
    }

    /// The name shown for a tile: plain names for the icon and Control
    /// Center instead of their window titles.
    pub fn display_name(&self, tile: &Tile) -> String {
        if Some(tile.id) == self.anchors.icon {
            return "The ice-rust icon".to_string();
        }
        match tile.title.as_str() {
            CONTROL_CENTER_TITLE => "Control Center".to_string(),
            title => title.to_string(),
        }
    }
}

/// Clamps an insertion index the way the drop rule does: a position right
/// of the trailing Clock and Control Center tiles becomes the position left
/// of them, because they cannot move. `index` counts positions in the bar
/// of `section` with `item` taken out.
pub fn clamp_insertion(model: &BarModel, item: WindowId, section: Section, index: usize) -> usize {
    let others: Vec<&Tile> = model.bar(section).iter().filter(|t| t.id != item).collect();
    let pinned_right = others
        .iter()
        .rev()
        .take_while(|t| is_system_locked(t))
        .count();
    index.min(others.len() - pinned_right)
}

/// Whether `destination` keeps an item inside `section`: next to one of
/// the section's own tiles, or against the divider edge that bounds it.
/// Visible stays right of the hidden divider, hidden stays between the two
/// dividers, always-hidden stays left of the always-hidden divider.
pub fn destination_allowed(
    model: &BarModel,
    section: Section,
    moving: WindowId,
    destination: Destination,
) -> bool {
    let target = destination.target();
    if target == moving {
        return false;
    }
    if model.bar(section).iter().any(|t| t.id == target) {
        return true;
    }
    let anchors = &model.anchors;
    match (section, destination) {
        (Section::Visible, Destination::RightOf(id)) => {
            id == anchors.hidden_divider || Some(id) == anchors.icon
        }
        (Section::Visible, Destination::LeftOf(id)) => Some(id) == anchors.icon,
        (Section::Hidden, Destination::LeftOf(id)) => id == anchors.hidden_divider,
        (Section::Hidden, Destination::RightOf(id)) => Some(id) == anchors.always_hidden_divider,
        (Section::AlwaysHidden, Destination::LeftOf(id)) => {
            Some(id) == anchors.always_hidden_divider
        }
        (Section::AlwaysHidden, Destination::RightOf(_)) => false,
    }
}

/// The destination for dropping `item` into `section` at `index`, where
/// `index` counts positions in that bar with the item taken out (0 is the
/// leftmost). With the bar as it would look after the drop, the item goes
/// left of the next tile if there is one, else right of the previous tile.
/// Into an empty bar it goes right of the icon (visible), left of the
/// hidden divider (hidden) or left of the always-hidden divider. A drop
/// right of Clock and Control Center snaps to their left, because they
/// cannot move.
pub fn drop_destination(
    model: &BarModel,
    item: WindowId,
    section: Section,
    index: usize,
) -> Result<DropOutcome, DropError> {
    let (from_section, from_index) = model.find(item).ok_or(DropError::UnknownItem)?;
    if model.bar(from_section)[from_index].locked {
        return Err(DropError::Locked);
    }
    let others: Vec<&Tile> = model.bar(section).iter().filter(|t| t.id != item).collect();
    let index = clamp_insertion(model, item, section, index);
    if section == from_section && index == from_index {
        return Ok(DropOutcome::NoChange);
    }
    let anchors = &model.anchors;
    let destination = if let Some(next) = others.get(index) {
        Destination::LeftOf(next.id)
    } else if let Some(previous) = index.checked_sub(1).and_then(|i| others.get(i)) {
        Destination::RightOf(previous.id)
    } else {
        match section {
            Section::Visible => {
                Destination::RightOf(anchors.icon.unwrap_or(anchors.hidden_divider))
            }
            Section::Hidden => Destination::LeftOf(anchors.hidden_divider),
            Section::AlwaysHidden => {
                Destination::LeftOf(anchors.always_hidden_divider.ok_or(DropError::NoAnchor)?)
            }
        }
    };
    if !destination_allowed(model, section, item, destination) {
        return Err(DropError::CrossesDivider);
    }
    Ok(DropOutcome::Move(destination))
}

/// The bars as they look once `item` sits where the user dropped it, at
/// `index` in `section` with the drop rule's clamp applied. The Menu Bar
/// Layout page shows this at once while the real item moves (v1.7), the
/// way Ice's layout editor does. `None` when the item or the move is not
/// allowed (`drop_destination`).
pub fn apply_drop(
    model: &BarModel,
    item: WindowId,
    section: Section,
    index: usize,
) -> Option<BarModel> {
    let (from_section, from_index) = model.find(item)?;
    if !matches!(
        drop_destination(model, item, section, index),
        Ok(DropOutcome::Move(_))
    ) {
        return None;
    }
    let index = clamp_insertion(model, item, section, index);
    let mut next = model.clone();
    let tile = bar_mut(&mut next, from_section).remove(from_index);
    let bar = bar_mut(&mut next, section);
    bar.insert(index.min(bar.len()), tile);
    Some(next)
}

fn bar_mut(model: &mut BarModel, section: Section) -> &mut Vec<Tile> {
    match section {
        Section::Visible => &mut model.visible,
        Section::Hidden => &mut model.hidden,
        Section::AlwaysHidden => &mut model.always_hidden,
    }
}

/// Whether two models put the same items in the same order in every
/// section. Window bounds and titles are ignored.
pub fn same_order(a: &BarModel, b: &BarModel) -> bool {
    Section::ALL
        .iter()
        .all(|&section| a.ids(section) == b.ids(section))
}

/// Whether `read` has every item of `model`, in any section. A read made
/// while the sections show or hide can miss the items that are still on
/// their way, even when two reads in a row agree (v1.7).
pub fn contains_all(read: &BarModel, model: &BarModel) -> bool {
    Section::ALL
        .iter()
        .flat_map(|&section| model.ids(section))
        .all(|id| read.find(id).is_some())
}

/// Whether the real menu bar agrees with the order the user dropped: every
/// item in both models sits in the same section and in the same relative
/// order. Items that appeared or went away meanwhile are ignored.
pub fn orders_agree(actual: &BarModel, expected: &BarModel) -> bool {
    Section::ALL.iter().all(|&section| {
        let common = |model: &BarModel, other: &BarModel| -> Vec<WindowId> {
            model
                .ids(section)
                .into_iter()
                .filter(|id| other.find(*id).is_some())
                .collect()
        };
        common(actual, expected) == common(expected, actual)
    })
}

/// Whether a model rebuilt from the real menu bar shows `item` where
/// `destination` asked for it: next to a tile on the requested side, or at
/// the bar end that a divider or the icon stands for.
pub fn model_matches(model: &BarModel, item: WindowId, destination: Destination) -> bool {
    let Some((section, index)) = model.find(item) else {
        return false;
    };
    let bar = model.bar(section);
    let target = destination.target();
    if let Some((target_section, target_index)) = model.find(target) {
        return target_section == section
            && match destination {
                Destination::LeftOf(_) => index + 1 == target_index,
                Destination::RightOf(_) => target_index + 1 == index,
            };
    }
    let anchors = &model.anchors;
    let first = index == 0;
    let last = index + 1 == bar.len();
    match destination {
        Destination::LeftOf(id) if id == anchors.hidden_divider => {
            section == Section::Hidden && last
        }
        Destination::LeftOf(id) if Some(id) == anchors.always_hidden_divider => {
            section == Section::AlwaysHidden && last
        }
        Destination::RightOf(id) if id == anchors.hidden_divider => {
            section == Section::Visible && first
        }
        Destination::RightOf(id) if Some(id) == anchors.always_hidden_divider => {
            section == Section::Hidden && first
        }
        Destination::RightOf(id) if Some(id) == anchors.icon => section == Section::Visible,
        _ => false,
    }
}

/// Direction the item travels toward its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Travel {
    Left,
    Right,
}

impl Travel {
    fn sign(self) -> f64 {
        match self {
            Travel::Left => -1.0,
            Travel::Right => 1.0,
        }
    }
}

pub fn travel(item: &Rect, target: &Rect) -> Travel {
    if target.mid_x() < item.mid_x() {
        Travel::Left
    } else {
        Travel::Right
    }
}

/// Where the Command-drag releases, in CG space, `MOVE_DROP_INSET` from a
/// target edge. For `LeftOf` the point is inside the target's left edge
/// when moving left and outside it when moving right; for `RightOf` inside
/// the right edge when moving right and outside it when moving left. Each
/// retry (`attempt` 1, 2, ...) moves the point `MOVE_DROP_NUDGE` further in
/// the travel direction.
pub fn drop_point(
    destination: Destination,
    target: &Rect,
    travel: Travel,
    attempt: usize,
) -> (f64, f64) {
    let base = match (destination, travel) {
        (Destination::LeftOf(_), Travel::Left) => target.min_x() + MOVE_DROP_INSET,
        (Destination::LeftOf(_), Travel::Right) => target.min_x() - MOVE_DROP_INSET,
        (Destination::RightOf(_), Travel::Right) => target.max_x() - MOVE_DROP_INSET,
        (Destination::RightOf(_), Travel::Left) => target.max_x() + MOVE_DROP_INSET,
    };
    let x = base + travel.sign() * MOVE_DROP_NUDGE * attempt as f64;
    (x, target.mid_y())
}

/// Whether the item abuts the target on the requested side, on the same
/// menu bar.
pub fn is_placed(destination: Destination, item: &Rect, target: &Rect) -> bool {
    let same_bar = (item.y - target.y).abs() <= ABUT_TOLERANCE
        && (item.height - target.height).abs() <= ABUT_TOLERANCE;
    let touching = match destination {
        Destination::LeftOf(_) => (item.max_x() - target.min_x()).abs() <= ABUT_TOLERANCE,
        Destination::RightOf(_) => (item.min_x() - target.max_x()).abs() <= ABUT_TOLERANCE,
    };
    same_bar && touching
}

/// `steps` evenly spaced points from `from` toward `to`, ending exactly at
/// `to`. Zero steps give `to` alone.
pub fn drag_path(from: (f64, f64), to: (f64, f64), steps: usize) -> Vec<(f64, f64)> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|i| {
            if i == steps {
                return to;
            }
            let t = i as f64 / steps as f64;
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
        })
        .collect()
}

/// The move engine waits for this: no mouse button down and no input for
/// at least `quiet`.
pub fn user_idle(button_down: bool, seconds_since_input: f64, quiet: Duration) -> bool {
    !button_down && seconds_since_input >= quiet.as_secs_f64()
}

/// Insertion index for a pointer at `x`. `slots` pairs each drawn tile's
/// index (counted with the dragged tile taken out) with its frame, left to
/// right. The index is that of the first tile whose center is right of
/// `x`, or one past the last tile.
pub fn insertion_index(slots: &[(usize, Rect)], x: f64) -> usize {
    slots
        .iter()
        .find(|(_, frame)| frame.mid_x() > x)
        .map(|(index, _)| *index)
        .unwrap_or_else(|| slots.last().map_or(0, |(index, _)| index + 1))
}

/// x of the insertion marker for `index` (see `insertion_index`): midway in
/// the gap before that tile, after the last tile, or near the bar's right
/// end when the bar is empty.
pub fn marker_x(slots: &[(usize, Rect)], index: usize, bar: &Rect, inset: f64) -> f64 {
    let at = slots.iter().position(|(i, _)| *i >= index);
    match at {
        Some(0) => slots[0].1.min_x() - inset / 2.0,
        Some(pos) => (slots[pos - 1].1.max_x() + slots[pos].1.min_x()) / 2.0,
        None => slots.last().map_or(bar.max_x() - inset, |(_, frame)| {
            frame.max_x() + inset / 2.0
        }),
    }
}

/// The section whose bar contains `y`, allowing `slack` above and below.
pub fn bar_at(bars: &[(Section, Rect)], y: f64, slack: f64) -> Option<Section> {
    bars.iter()
        .find(|(_, r)| y >= r.min_y() - slack && y < r.max_y() + slack)
        .map(|(section, _)| *section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::STATUS_LAYER;

    fn w(id: u32, x: f64, width: f64, title: &str) -> WindowRecord {
        WindowRecord {
            id,
            layer: STATUS_LAYER,
            bounds: Rect::new(x, 0.0, width, 30.0),
            title: Some(title.into()),
            owner_name: Some("Control Center".into()),
            on_screen: true,
        }
    }

    const ICON: u32 = 186;
    const HIDDEN: u32 = 188;
    const ALWAYS: u32 = 190;

    fn anchors() -> Anchors {
        Anchors {
            icon: Some(ICON),
            hidden_divider: HIDDEN,
            always_hidden_divider: Some(ALWAYS),
        }
    }

    /// Sections modeled on the evidence host with Ice's names. The chains
    /// include a divider each, as a broken walk might.
    fn chains() -> SectionChains {
        SectionChains {
            visible: vec![
                w(7090, 4622.0, 48.0, "AudioVideoModule"),
                w(ICON, 4670.0, 33.0, "Ice.ControlItem.Visible"),
                w(446, 4703.0, 38.0, "Item-0"),
                w(8288, 4741.0, 33.0, "NowPlaying"),
                w(57, 4774.0, 42.0, "BentoBox-0"),
                w(54, 4816.0, 142.0, "Clock"),
            ],
            hidden: vec![
                w(312, -498.0, 32.0, "Item-0"),
                w(52, -466.0, 34.0, "AirDrop"),
                w(68, -432.0, 38.0, "Item-0"),
                w(HIDDEN, -394.0, 5016.0, "Ice.ControlItem.Hidden"),
            ],
            always_hidden: vec![
                w(214, -5580.0, 34.0, "ExampleMenu"),
                w(215, -5546.0, 32.0, "Item-2"),
            ],
        }
    }

    fn model() -> BarModel {
        BarModel::from_sections(
            &chains(),
            &ControlNames::with_prefix("Ice.ControlItem."),
            anchors(),
        )
    }

    fn empty_model() -> BarModel {
        BarModel {
            visible: vec![],
            hidden: vec![],
            always_hidden: vec![],
            anchors: anchors(),
        }
    }

    fn tile(id: u32, title: &str) -> Tile {
        Tile {
            id,
            title: title.into(),
            bounds: Rect::default(),
            locked: false,
        }
    }

    fn mv(dest: Destination) -> Result<DropOutcome, DropError> {
        Ok(DropOutcome::Move(dest))
    }

    use Destination::{LeftOf, RightOf};

    #[test]
    fn apply_drop_moves_the_tile_where_it_was_dropped() {
        let m = model();
        // Within a bar: 312 to the end of the hidden bar.
        let next = apply_drop(&m, 312, Section::Hidden, 2).unwrap();
        assert_eq!(next.ids(Section::Hidden), vec![52, 68, 312]);
        // Across bars: AirDrop into the always-hidden bar, first place.
        let next = apply_drop(&m, 52, Section::AlwaysHidden, 0).unwrap();
        assert_eq!(next.ids(Section::Hidden), vec![312, 68]);
        assert_eq!(next.ids(Section::AlwaysHidden), vec![52, 214, 215]);
        // The other bars are untouched.
        assert_eq!(next.ids(Section::Visible), m.ids(Section::Visible));
    }

    #[test]
    fn apply_drop_clamps_like_the_drop_rule() {
        // Right of Clock and Control Center snaps to their left.
        let m = model();
        let next = apply_drop(&m, 446, Section::Visible, 99).unwrap();
        assert_eq!(
            next.ids(Section::Visible),
            vec![7090, ICON, 8288, 446, 57, 54]
        );
    }

    #[test]
    fn apply_drop_agrees_with_the_destination_it_asks_for() {
        let m = model();
        for (item, section, index) in [
            (312, Section::Hidden, 2),
            (52, Section::AlwaysHidden, 0),
            (215, Section::Visible, 0),
            (68, Section::Hidden, 0),
            (446, Section::Visible, 99),
            (7090, Section::Hidden, 3),
        ] {
            let Ok(DropOutcome::Move(destination)) = drop_destination(&m, item, section, index)
            else {
                panic!("{item} to {section:?} {index} should move");
            };
            let next = apply_drop(&m, item, section, index).unwrap();
            assert!(
                model_matches(&next, item, destination),
                "{item} to {section:?} {index}: {destination:?}"
            );
        }
    }

    #[test]
    fn apply_drop_refuses_what_the_drop_rule_refuses() {
        let m = model();
        // Locked, unknown, and a drop that changes nothing.
        assert_eq!(apply_drop(&m, ICON, Section::Hidden, 0), None);
        assert_eq!(apply_drop(&m, 999, Section::Hidden, 0), None);
        assert_eq!(apply_drop(&m, 312, Section::Hidden, 0), None);
    }

    #[test]
    fn same_order_compares_ids_per_section() {
        let m = model();
        assert!(same_order(&m, &m.clone()));
        let mut moved_bounds = m.clone();
        moved_bounds.hidden[0].bounds = Rect::new(1.0, 2.0, 3.0, 4.0);
        assert!(same_order(&m, &moved_bounds));
        let next = apply_drop(&m, 312, Section::Hidden, 2).unwrap();
        assert!(!same_order(&m, &next));
        // An empty bar that had items is a different order, which is what
        // a read caught mid-transition looks like.
        let mut emptied = m.clone();
        emptied.always_hidden.clear();
        assert!(!same_order(&m, &emptied));
    }

    #[test]
    fn contains_all_spots_items_still_on_their_way() {
        let m = model();
        assert!(contains_all(&m, &m));
        // Measured after a move: most always-hidden items were missing
        // from two reads in a row while the menu bar settled.
        let mut partial = m.clone();
        partial.always_hidden.truncate(1);
        assert!(!contains_all(&partial, &m));
        assert!(contains_all(&m, &partial));
        // A moved item counts wherever it is.
        let next = apply_drop(&m, 312, Section::AlwaysHidden, 0).unwrap();
        assert!(contains_all(&next, &m));
    }

    #[test]
    fn orders_agree_ignores_items_that_came_or_went() {
        let m = model();
        let expected = apply_drop(&m, 312, Section::Hidden, 2).unwrap();
        assert!(orders_agree(&expected, &expected));
        // A new item appeared in the real menu bar meanwhile.
        let mut actual = expected.clone();
        actual.hidden.insert(1, tile(999, "New"));
        assert!(orders_agree(&actual, &expected));
        // An item went away.
        let mut actual = expected.clone();
        actual.visible.remove(0);
        assert!(orders_agree(&actual, &expected));
        // The moved item did not land: the old order.
        assert!(!orders_agree(&m, &expected));
        // An item in another section.
        let mut actual = expected.clone();
        let moved = actual.hidden.remove(0);
        actual.always_hidden.push(moved);
        assert!(!orders_agree(&actual, &expected));
    }

    #[test]
    fn model_skips_dividers_and_locks_fixed_items() {
        let m = model();
        assert_eq!(m.ids(Section::Visible), vec![7090, ICON, 446, 8288, 57, 54]);
        assert_eq!(m.ids(Section::Hidden), vec![312, 52, 68]);
        assert_eq!(m.ids(Section::AlwaysHidden), vec![214, 215]);
        let locked: Vec<u32> = m
            .visible
            .iter()
            .filter(|t| t.locked)
            .map(|t| t.id)
            .collect();
        assert_eq!(locked, vec![ICON, 57, 54]);
        assert!(m.hidden.iter().all(|t| !t.locked));
        assert_eq!(m.find(52), Some((Section::Hidden, 1)));
        assert_eq!(m.find(999), None);
        assert_eq!(m.tile(54).map(|t| t.title.as_str()), Some("Clock"));
    }

    #[test]
    fn divider_titles_are_dropped_even_with_other_ids() {
        let mut c = chains();
        c.always_hidden
            .push(w(9999, -5514.0, 20.0, "Ice.ControlItem.AlwaysHidden"));
        let m = BarModel::from_sections(
            &c,
            &ControlNames::with_prefix("Ice.ControlItem."),
            anchors(),
        );
        assert_eq!(m.ids(Section::AlwaysHidden), vec![214, 215]);
    }

    #[test]
    fn drop_left_of_next_tile() {
        let m = model();
        // Hidden item to the front of the visible bar.
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 0),
            mv(LeftOf(7090))
        );
        // Between the icon and 446.
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 2),
            mv(LeftOf(446))
        );
        // Visible item into the middle of the hidden bar.
        assert_eq!(
            drop_destination(&m, 446, Section::Hidden, 1),
            mv(LeftOf(52))
        );
    }

    #[test]
    fn drop_right_of_previous_at_bar_end() {
        let m = model();
        // End of the hidden bar: right of 68, left of the hidden divider.
        assert_eq!(
            drop_destination(&m, 446, Section::Hidden, 3),
            mv(RightOf(68))
        );
        // End of the always-hidden bar.
        assert_eq!(
            drop_destination(&m, 312, Section::AlwaysHidden, 2),
            mv(RightOf(215))
        );
        // An index past the end is clamped.
        assert_eq!(
            drop_destination(&m, 312, Section::AlwaysHidden, 99),
            mv(RightOf(215))
        );
    }

    #[test]
    fn drop_right_of_clock_snaps_left_of_control_center() {
        let m = model();
        // Visible bar without 52 has 6 tiles; 57 and 54 are pinned right.
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 6),
            mv(LeftOf(57))
        );
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 5),
            mv(LeftOf(57))
        );
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 4),
            mv(LeftOf(57))
        );
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 3),
            mv(LeftOf(8288))
        );
    }

    #[test]
    fn same_bar_moves() {
        let m = model();
        // 312 one step right: after the drop the bar is 52, 312, 68.
        assert_eq!(
            drop_destination(&m, 312, Section::Hidden, 1),
            mv(LeftOf(68))
        );
        // 68 to the front.
        assert_eq!(
            drop_destination(&m, 68, Section::Hidden, 0),
            mv(LeftOf(312))
        );
        // 312 to the end.
        assert_eq!(
            drop_destination(&m, 312, Section::Hidden, 2),
            mv(RightOf(68))
        );
    }

    #[test]
    fn drop_at_original_position_does_nothing() {
        let m = model();
        assert_eq!(
            drop_destination(&m, 52, Section::Hidden, 1),
            Ok(DropOutcome::NoChange)
        );
        assert_eq!(
            drop_destination(&m, 312, Section::Hidden, 0),
            Ok(DropOutcome::NoChange)
        );
        assert_eq!(
            drop_destination(&m, 215, Section::AlwaysHidden, 1),
            Ok(DropOutcome::NoChange)
        );
        assert_eq!(
            drop_destination(&m, 7090, Section::Visible, 0),
            Ok(DropOutcome::NoChange)
        );
    }

    #[test]
    fn empty_bars_use_the_dividers() {
        let mut m = empty_model();
        m.visible = vec![tile(446, "Item-0")];
        assert_eq!(
            drop_destination(&m, 446, Section::Hidden, 0),
            mv(LeftOf(HIDDEN))
        );
        assert_eq!(
            drop_destination(&m, 446, Section::AlwaysHidden, 0),
            mv(LeftOf(ALWAYS))
        );
        let mut m = empty_model();
        m.hidden = vec![tile(52, "AirDrop")];
        // The icon is not a tile here, so the visible bar is empty.
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 0),
            mv(RightOf(ICON))
        );
        m.anchors.icon = None;
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 0),
            mv(RightOf(HIDDEN))
        );
    }

    #[test]
    fn last_item_leaving_a_bar() {
        let mut m = empty_model();
        m.hidden = vec![tile(52, "AirDrop")];
        m.always_hidden = vec![tile(214, "ExampleMenu")];
        assert_eq!(
            drop_destination(&m, 52, Section::AlwaysHidden, 0),
            mv(LeftOf(214))
        );
        assert_eq!(
            drop_destination(&m, 214, Section::Hidden, 1),
            mv(RightOf(52))
        );
        // A lone item dropped back into its own bar stays.
        assert_eq!(
            drop_destination(&m, 52, Section::Hidden, 0),
            Ok(DropOutcome::NoChange)
        );
    }

    #[test]
    fn always_hidden_without_divider() {
        let mut m = empty_model();
        m.anchors.always_hidden_divider = None;
        m.hidden = vec![tile(52, "AirDrop")];
        assert_eq!(
            drop_destination(&m, 52, Section::AlwaysHidden, 0),
            Err(DropError::NoAnchor)
        );
    }

    #[test]
    fn locked_and_unknown_items_cannot_move() {
        let m = model();
        assert_eq!(
            drop_destination(&m, 54, Section::Hidden, 0),
            Err(DropError::Locked)
        );
        assert_eq!(
            drop_destination(&m, 57, Section::Visible, 0),
            Err(DropError::Locked)
        );
        assert_eq!(
            drop_destination(&m, ICON, Section::Hidden, 0),
            Err(DropError::Locked)
        );
        assert_eq!(
            drop_destination(&m, 4242, Section::Hidden, 0),
            Err(DropError::UnknownItem)
        );
    }

    #[test]
    fn locked_tiles_are_drop_neighbors() {
        let m = model();
        // Left of the icon, right of AudioVideoModule.
        assert_eq!(
            drop_destination(&m, 52, Section::Visible, 1),
            mv(LeftOf(ICON))
        );
    }

    #[test]
    fn divider_crossing_guard() {
        let m = model();
        // Visible stays right of the hidden divider.
        assert!(destination_allowed(
            &m,
            Section::Visible,
            52,
            RightOf(HIDDEN)
        ));
        assert!(!destination_allowed(
            &m,
            Section::Visible,
            52,
            LeftOf(HIDDEN)
        ));
        assert!(!destination_allowed(&m, Section::Visible, 52, LeftOf(312)));
        assert!(destination_allowed(&m, Section::Visible, 52, LeftOf(ICON)));
        // Hidden stays between the two dividers.
        assert!(destination_allowed(
            &m,
            Section::Hidden,
            446,
            LeftOf(HIDDEN)
        ));
        assert!(destination_allowed(
            &m,
            Section::Hidden,
            446,
            RightOf(ALWAYS)
        ));
        assert!(!destination_allowed(
            &m,
            Section::Hidden,
            446,
            RightOf(HIDDEN)
        ));
        assert!(!destination_allowed(
            &m,
            Section::Hidden,
            446,
            LeftOf(ALWAYS)
        ));
        assert!(!destination_allowed(&m, Section::Hidden, 446, LeftOf(7090)));
        assert!(!destination_allowed(&m, Section::Hidden, 446, RightOf(214)));
        // Always-hidden stays left of the always-hidden divider.
        assert!(destination_allowed(
            &m,
            Section::AlwaysHidden,
            52,
            LeftOf(ALWAYS)
        ));
        assert!(!destination_allowed(
            &m,
            Section::AlwaysHidden,
            52,
            RightOf(ALWAYS)
        ));
        assert!(!destination_allowed(
            &m,
            Section::AlwaysHidden,
            52,
            LeftOf(312)
        ));
        // Never relative to itself.
        assert!(!destination_allowed(&m, Section::Hidden, 52, LeftOf(52)));
    }

    #[test]
    fn rebuilt_model_matches_destination() {
        let m = model();
        assert!(model_matches(&m, 52, LeftOf(68)));
        assert!(model_matches(&m, 68, RightOf(52)));
        assert!(!model_matches(&m, 52, LeftOf(312)));
        assert!(!model_matches(&m, 52, RightOf(68)));
        // Neighbors in different bars do not match.
        assert!(!model_matches(&m, 68, LeftOf(7090)));
        // Divider anchors stand for bar ends.
        assert!(model_matches(&m, 68, LeftOf(HIDDEN)));
        assert!(!model_matches(&m, 52, LeftOf(HIDDEN)));
        assert!(model_matches(&m, 7090, RightOf(HIDDEN)));
        assert!(model_matches(&m, 215, LeftOf(ALWAYS)));
        assert!(model_matches(&m, 312, RightOf(ALWAYS)));
        assert!(!model_matches(&m, 214, LeftOf(ALWAYS)));
        // Missing items never match.
        assert!(!model_matches(&m, 4242, LeftOf(68)));
    }

    #[test]
    fn drop_points() {
        let target = Rect::new(100.0, 0.0, 30.0, 30.0);
        let l = LeftOf(1);
        let r = RightOf(1);
        assert_eq!(drop_point(l, &target, Travel::Left, 0), (102.0, 15.0));
        assert_eq!(drop_point(l, &target, Travel::Right, 0), (98.0, 15.0));
        assert_eq!(drop_point(r, &target, Travel::Right, 0), (128.0, 15.0));
        assert_eq!(drop_point(r, &target, Travel::Left, 0), (132.0, 15.0));
        // Retries nudge 4 pt per attempt in the travel direction.
        assert_eq!(drop_point(l, &target, Travel::Left, 1), (98.0, 15.0));
        assert_eq!(drop_point(l, &target, Travel::Left, 3), (90.0, 15.0));
        assert_eq!(drop_point(l, &target, Travel::Right, 2), (106.0, 15.0));
        assert_eq!(drop_point(r, &target, Travel::Left, 1), (128.0, 15.0));
    }

    #[test]
    fn travel_direction() {
        let item = Rect::new(100.0, 0.0, 30.0, 30.0);
        assert_eq!(
            travel(&item, &Rect::new(10.0, 0.0, 30.0, 30.0)),
            Travel::Left
        );
        assert_eq!(
            travel(&item, &Rect::new(200.0, 0.0, 30.0, 30.0)),
            Travel::Right
        );
        // Off-screen hidden item moving into the visible section.
        assert_eq!(
            travel(
                &Rect::new(-466.0, 0.0, 34.0, 30.0),
                &Rect::new(4703.0, 0.0, 38.0, 30.0)
            ),
            Travel::Right
        );
    }

    #[test]
    fn placement_check() {
        let target = Rect::new(100.0, 0.0, 30.0, 30.0);
        assert!(is_placed(
            LeftOf(1),
            &Rect::new(66.0, 0.0, 34.0, 30.0),
            &target
        ));
        assert!(is_placed(
            LeftOf(1),
            &Rect::new(66.5, 0.0, 34.0, 30.0),
            &target
        ));
        assert!(!is_placed(
            LeftOf(1),
            &Rect::new(64.0, 0.0, 34.0, 30.0),
            &target
        ));
        assert!(!is_placed(
            LeftOf(1),
            &Rect::new(130.0, 0.0, 34.0, 30.0),
            &target
        ));
        assert!(is_placed(
            RightOf(1),
            &Rect::new(130.0, 0.0, 34.0, 30.0),
            &target
        ));
        assert!(!is_placed(
            RightOf(1),
            &Rect::new(66.0, 0.0, 34.0, 30.0),
            &target
        ));
        // A copy on another menu bar does not count.
        assert!(!is_placed(
            LeftOf(1),
            &Rect::new(66.0, 1080.0, 34.0, 33.0),
            &target
        ));
    }

    #[test]
    fn drag_path_steps() {
        assert_eq!(
            drag_path((0.0, 15.0), (80.0, 15.0), 8),
            vec![
                (10.0, 15.0),
                (20.0, 15.0),
                (30.0, 15.0),
                (40.0, 15.0),
                (50.0, 15.0),
                (60.0, 15.0),
                (70.0, 15.0),
                (80.0, 15.0)
            ]
        );
        assert_eq!(drag_path((0.0, 0.0), (5.0, 5.0), 0), vec![(5.0, 5.0)]);
        // The last point is exact even when the division is not.
        let p = drag_path((0.0, 0.0), (1.0, 0.0), 3);
        assert_eq!(p.last(), Some(&(1.0, 0.0)));
    }

    #[test]
    fn idle_predicate() {
        let quiet = Duration::from_millis(50);
        assert!(user_idle(false, 0.05, quiet));
        assert!(user_idle(false, 10.0, quiet));
        assert!(!user_idle(false, 0.049, quiet));
        assert!(!user_idle(true, 10.0, quiet));
    }

    #[test]
    fn insertion_clamps_like_the_drop_rule() {
        let m = model();
        // Visible bar without 52: 7090, ICON, 446, 8288, 57, 54. The
        // trailing 57 and 54 cannot move, so indices past 4 become 4.
        assert_eq!(clamp_insertion(&m, 52, Section::Visible, 6), 4);
        assert_eq!(clamp_insertion(&m, 52, Section::Visible, 5), 4);
        assert_eq!(clamp_insertion(&m, 52, Section::Visible, 4), 4);
        assert_eq!(clamp_insertion(&m, 52, Section::Visible, 2), 2);
        // A visible item moving within its own bar.
        assert_eq!(clamp_insertion(&m, 446, Section::Visible, 5), 3);
        // Bars without pinned tiles only clamp to their length.
        assert_eq!(clamp_insertion(&m, 446, Section::Hidden, 9), 3);
        assert_eq!(clamp_insertion(&m, 446, Section::AlwaysHidden, 1), 1);
        // The marker index and the drop agree.
        for index in 0..8 {
            let clamped = clamp_insertion(&m, 52, Section::Visible, index);
            assert_eq!(
                drop_destination(&m, 52, Section::Visible, index),
                drop_destination(&m, 52, Section::Visible, clamped)
            );
        }
    }

    #[test]
    fn display_names_for_locked_tiles() {
        let m = model();
        let name = |id| m.tile(id).map(|t| m.display_name(t));
        assert_eq!(name(57).as_deref(), Some("Control Center"));
        assert_eq!(name(54).as_deref(), Some("Clock"));
        assert_eq!(name(ICON).as_deref(), Some("The ice-rust icon"));
        assert_eq!(name(52).as_deref(), Some("AirDrop"));
    }

    #[test]
    fn insertion_by_tile_centers() {
        let slots = vec![
            (0, Rect::new(10.0, 0.0, 20.0, 30.0)),
            (1, Rect::new(34.0, 0.0, 20.0, 30.0)),
            (2, Rect::new(58.0, 0.0, 20.0, 30.0)),
        ];
        assert_eq!(insertion_index(&slots, 0.0), 0);
        assert_eq!(insertion_index(&slots, 19.0), 0);
        assert_eq!(insertion_index(&slots, 21.0), 1);
        assert_eq!(insertion_index(&slots, 50.0), 2);
        assert_eq!(insertion_index(&slots, 70.0), 3);
        assert_eq!(insertion_index(&[], 70.0), 0);
        // A slot list that starts past the bar's first tile.
        let drawn = vec![(3, Rect::new(10.0, 0.0, 20.0, 30.0))];
        assert_eq!(insertion_index(&drawn, 0.0), 3);
        assert_eq!(insertion_index(&drawn, 25.0), 4);
    }

    #[test]
    fn marker_positions() {
        let bar = Rect::new(0.0, 0.0, 200.0, 40.0);
        let slots = vec![
            (0, Rect::new(100.0, 5.0, 20.0, 30.0)),
            (1, Rect::new(124.0, 5.0, 20.0, 30.0)),
        ];
        assert_eq!(marker_x(&slots, 0, &bar, 4.0), 98.0);
        assert_eq!(marker_x(&slots, 1, &bar, 4.0), 122.0);
        assert_eq!(marker_x(&slots, 2, &bar, 4.0), 146.0);
        assert_eq!(marker_x(&[], 0, &bar, 4.0), 196.0);
    }

    #[test]
    fn bar_hit_test() {
        let bars = vec![
            (Section::Visible, Rect::new(0.0, 100.0, 200.0, 40.0)),
            (Section::Hidden, Rect::new(0.0, 170.0, 200.0, 40.0)),
        ];
        assert_eq!(bar_at(&bars, 120.0, 6.0), Some(Section::Visible));
        assert_eq!(bar_at(&bars, 96.0, 6.0), Some(Section::Visible));
        assert_eq!(bar_at(&bars, 160.0, 6.0), None);
        assert_eq!(bar_at(&bars, 166.0, 6.0), Some(Section::Hidden));
        assert_eq!(bar_at(&bars, 300.0, 6.0), None);
    }
}
