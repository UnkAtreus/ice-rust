//! The three menu bar sections and which dividers are expanded (v1.1).
//!
//! Sections run right to left: visible, hidden, always-hidden. Each divider
//! is either expanded (10000 pt, pushing everything left of it off screen)
//! or collapsed (variable length with a chevron). Showing the always-hidden
//! section shows everything; hiding the hidden section hides everything;
//! hiding only the always-hidden section re-expands only its divider.

/// A menu bar section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Visible,
    Hidden,
    AlwaysHidden,
}

impl Section {
    /// Top to bottom in the settings pane.
    pub const ALL: [Section; 3] = [Section::Visible, Section::Hidden, Section::AlwaysHidden];

    /// The position in `ALL`.
    pub fn index(self) -> usize {
        match self {
            Section::Visible => 0,
            Section::Hidden => 1,
            Section::AlwaysHidden => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Visible => "Visible Section",
            Section::Hidden => "Hidden Section",
            Section::AlwaysHidden => "Always-Hidden Section",
        }
    }
}

/// What the menu bar currently shows. Ordered from least to most shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MenuBarState {
    /// Both dividers expanded.
    AllHidden,
    /// Hidden divider collapsed, always-hidden divider expanded.
    HiddenShown,
    /// Both dividers collapsed.
    AllShown,
}

/// Divider lengths for a state: `true` means expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DividerLengths {
    pub hidden_expanded: bool,
    pub always_hidden_expanded: bool,
}

impl MenuBarState {
    pub fn lengths(self) -> DividerLengths {
        let (hidden_expanded, always_hidden_expanded) = match self {
            MenuBarState::AllHidden => (true, true),
            MenuBarState::HiddenShown => (false, true),
            MenuBarState::AllShown => (false, false),
        };
        DividerLengths {
            hidden_expanded,
            always_hidden_expanded,
        }
    }

    pub fn is_shown(self, section: Section) -> bool {
        match section {
            Section::Visible => true,
            Section::Hidden => self >= MenuBarState::HiddenShown,
            Section::AlwaysHidden => self == MenuBarState::AllShown,
        }
    }

    /// The smallest state that shows `section`.
    fn minimum_for(section: Section) -> MenuBarState {
        match section {
            Section::Visible => MenuBarState::AllHidden,
            Section::Hidden => MenuBarState::HiddenShown,
            Section::AlwaysHidden => MenuBarState::AllShown,
        }
    }

    /// Shows `section` and keeps whatever else is shown.
    pub fn show(self, section: Section) -> MenuBarState {
        self.max(Self::minimum_for(section))
    }

    /// Hiding the visible or hidden section hides everything; hiding the
    /// always-hidden section re-expands only its divider.
    pub fn hide(self, section: Section) -> MenuBarState {
        match section {
            Section::Visible | Section::Hidden => MenuBarState::AllHidden,
            Section::AlwaysHidden => self.min(MenuBarState::HiddenShown),
        }
    }

    pub fn toggle(self, section: Section) -> MenuBarState {
        if section != Section::Visible && self.is_shown(section) {
            self.hide(section)
        } else {
            self.show(section)
        }
    }

    /// The state that shows every section in `sections`, at least `self`.
    /// The move engine uses it to put both the item and its target on
    /// screen.
    pub fn covering(self, sections: &[Section]) -> MenuBarState {
        sections
            .iter()
            .fold(self, |state, &section| state.show(section))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MenuBarState::*;

    #[test]
    fn lengths_per_state() {
        assert_eq!(
            AllHidden.lengths(),
            DividerLengths {
                hidden_expanded: true,
                always_hidden_expanded: true
            }
        );
        assert_eq!(
            HiddenShown.lengths(),
            DividerLengths {
                hidden_expanded: false,
                always_hidden_expanded: true
            }
        );
        assert_eq!(
            AllShown.lengths(),
            DividerLengths {
                hidden_expanded: false,
                always_hidden_expanded: false
            }
        );
    }

    #[test]
    fn show_transitions() {
        assert_eq!(AllHidden.show(Section::Hidden), HiddenShown);
        assert_eq!(HiddenShown.show(Section::Hidden), HiddenShown);
        assert_eq!(AllShown.show(Section::Hidden), AllShown);
        for state in [AllHidden, HiddenShown, AllShown] {
            assert_eq!(state.show(Section::AlwaysHidden), AllShown);
            assert_eq!(state.show(Section::Visible), state);
        }
    }

    #[test]
    fn hide_transitions() {
        for state in [AllHidden, HiddenShown, AllShown] {
            assert_eq!(state.hide(Section::Hidden), AllHidden);
            assert_eq!(state.hide(Section::Visible), AllHidden);
        }
        assert_eq!(AllShown.hide(Section::AlwaysHidden), HiddenShown);
        assert_eq!(HiddenShown.hide(Section::AlwaysHidden), HiddenShown);
        assert_eq!(AllHidden.hide(Section::AlwaysHidden), AllHidden);
    }

    #[test]
    fn click_and_option_click_sequences() {
        // Left click toggles the hidden section.
        let s = AllHidden.toggle(Section::Hidden);
        assert_eq!(s, HiddenShown);
        // Option-click shows everything, then hides only always-hidden.
        let s = s.toggle(Section::AlwaysHidden);
        assert_eq!(s, AllShown);
        let s = s.toggle(Section::AlwaysHidden);
        assert_eq!(s, HiddenShown);
        // Option-click from all hidden shows everything at once.
        assert_eq!(AllHidden.toggle(Section::AlwaysHidden), AllShown);
        // Left click while everything is shown hides everything.
        assert_eq!(AllShown.toggle(Section::Hidden), AllHidden);
        // The visible section is always shown and cannot be toggled off.
        assert_eq!(AllShown.toggle(Section::Visible), AllShown);
    }

    #[test]
    fn shown_queries() {
        assert!(AllHidden.is_shown(Section::Visible));
        assert!(!AllHidden.is_shown(Section::Hidden));
        assert!(!AllHidden.is_shown(Section::AlwaysHidden));
        assert!(HiddenShown.is_shown(Section::Hidden));
        assert!(!HiddenShown.is_shown(Section::AlwaysHidden));
        assert!(AllShown.is_shown(Section::Hidden));
        assert!(AllShown.is_shown(Section::AlwaysHidden));
    }

    #[test]
    fn covering_shows_both_ends_of_a_move() {
        assert_eq!(
            AllHidden.covering(&[Section::Visible, Section::Visible]),
            AllHidden
        );
        assert_eq!(
            AllHidden.covering(&[Section::Hidden, Section::Visible]),
            HiddenShown
        );
        assert_eq!(
            AllHidden.covering(&[Section::Visible, Section::AlwaysHidden]),
            AllShown
        );
        // Never less than what is already shown.
        assert_eq!(AllShown.covering(&[Section::Visible]), AllShown);
    }

    #[test]
    fn labels() {
        let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            vec!["Visible Section", "Hidden Section", "Always-Hidden Section"]
        );
    }
}
