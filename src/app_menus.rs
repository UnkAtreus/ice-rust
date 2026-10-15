//! Hide application menus (v1.6), like Ice's HideApplicationMenus. When a
//! section shown in the menu bar reaches the frontmost app's menus, macOS
//! drops the items that do not fit. ice-rust then becomes the active app
//! with its own menus hidden, so the menu bar shows only the Apple menu and
//! the items get the room; it steps back once the sections hide. Pure;
//! `platform::app::app_menus` acts on the decision.

/// Whether the shown items reach the app menus: Ice's test, the leftmost
/// on-screen status item's left edge at or left of the menus' right edge.
/// No items reach nothing.
pub fn items_reach_menus(item_left_edges: impl IntoIterator<Item = f64>, menus_end: f64) -> bool {
    item_left_edges
        .into_iter()
        .filter(|x| x.is_finite())
        .fold(None, |min: Option<f64>, x| {
            Some(min.map_or(x, |m| m.min(x)))
        })
        .is_some_and(|leftmost| leftmost <= menus_end)
}

/// What the decision looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuSituation {
    /// The Hide app menus setting.
    pub enabled: bool,
    /// Ice Bar mode shows items in a panel, which never meets the menus.
    pub use_ice_bar: bool,
    /// Some section is shown in the menu bar.
    pub section_shown: bool,
    /// The Settings window is open; ice-rust is a regular app for it.
    pub settings_open: bool,
    /// ice-rust is hiding the app menus now.
    pub hiding: bool,
    /// The shown items reach the frontmost app's menus; `None` when that
    /// was not or could not be measured.
    pub overlap: Option<bool>,
}

impl MenuSituation {
    /// Whether hiding could be wanted, so the overlap is worth measuring.
    pub fn may_hide(&self) -> bool {
        self.enabled && !self.use_ice_bar && self.section_shown && !self.settings_open
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Become the active app with no menus.
    Hide,
    /// Show the app menus again and give focus back.
    Restore,
    Keep,
}

/// Hides once the shown items reach the menus, and keeps hiding until the
/// sections hide, the setting or Ice Bar mode changes, or Settings opens.
/// Like Ice, it does not show the menus again while sections stay shown,
/// since the items would collide again.
pub fn menu_action(s: MenuSituation) -> MenuAction {
    match (s.hiding, s.may_hide()) {
        (true, true) => MenuAction::Keep,
        (true, false) => MenuAction::Restore,
        (false, true) if s.overlap == Some(true) => MenuAction::Hide,
        (false, _) => MenuAction::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftmost_item_decides() {
        assert!(items_reach_menus([900.0, 640.0, 700.0], 650.0));
        assert!(items_reach_menus([650.0], 650.0));
        assert!(!items_reach_menus([900.0, 651.0], 650.0));
        assert!(!items_reach_menus([], 650.0));
        assert!(!items_reach_menus([f64::NAN], 650.0));
        assert!(items_reach_menus([f64::NAN, 10.0], 650.0));
    }

    fn shown_and_overlapping() -> MenuSituation {
        MenuSituation {
            enabled: true,
            use_ice_bar: false,
            section_shown: true,
            settings_open: false,
            hiding: false,
            overlap: Some(true),
        }
    }

    #[test]
    fn hides_when_shown_items_reach_the_menus() {
        assert_eq!(menu_action(shown_and_overlapping()), MenuAction::Hide);
    }

    #[test]
    fn does_not_hide_without_overlap_or_when_off() {
        let base = shown_and_overlapping();
        for s in [
            MenuSituation {
                overlap: Some(false),
                ..base
            },
            MenuSituation {
                overlap: None,
                ..base
            },
            MenuSituation {
                enabled: false,
                ..base
            },
            MenuSituation {
                use_ice_bar: true,
                ..base
            },
            MenuSituation {
                section_shown: false,
                ..base
            },
            MenuSituation {
                settings_open: true,
                ..base
            },
        ] {
            assert_eq!(menu_action(s), MenuAction::Keep, "{s:?}");
        }
    }

    #[test]
    fn keeps_hiding_while_sections_stay_shown() {
        let hiding = MenuSituation {
            hiding: true,
            ..shown_and_overlapping()
        };
        assert_eq!(menu_action(hiding), MenuAction::Keep);
        // No overlap any more, because the menus are gone: still hidden.
        assert_eq!(
            menu_action(MenuSituation {
                overlap: Some(false),
                ..hiding
            }),
            MenuAction::Keep
        );
    }

    #[test]
    fn restores_once_hiding_is_no_longer_wanted() {
        let hiding = MenuSituation {
            hiding: true,
            ..shown_and_overlapping()
        };
        for s in [
            MenuSituation {
                section_shown: false,
                ..hiding
            },
            MenuSituation {
                enabled: false,
                ..hiding
            },
            MenuSituation {
                use_ice_bar: true,
                ..hiding
            },
            MenuSituation {
                settings_open: true,
                ..hiding
            },
        ] {
            assert_eq!(menu_action(s), MenuAction::Restore, "{s:?}");
        }
    }

    #[test]
    fn overlap_is_measured_only_when_hiding_may_follow() {
        let base = shown_and_overlapping();
        assert!(base.may_hide());
        assert!(
            !MenuSituation {
                section_shown: false,
                ..base
            }
            .may_hide()
        );
    }
}
