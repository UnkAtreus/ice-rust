//! The items of each section on the macOS 27 menu bar, as last seen (v1.12).
//! Hidden items are folded into the system overflow, where MenuBarAgent
//! gives them no place, so which section an item belongs to is known only
//! from a read of the bar while that section was shown: at launch, while
//! the user shows a section, while Menu Bar Layout is open, and when the
//! Ice Bar or the search panel shows a section to click an item. The Ice
//! Bar and the search panel list the items from here. Pure.

use crate::chain::SectionChains;
use crate::model::{Rect, STATUS_LAYER, WindowId, WindowRecord};
use crate::sections::Section;

/// One item as last seen.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogItem {
    pub id: WindowId,
    /// The item's own name (title, description or identifier).
    pub name: String,
    /// Its size on the bar, in points.
    pub size: (f64, f64),
    /// The process that owned it then; `None` for Apple's items.
    pub pid: Option<i32>,
}

/// What one read of the bar saw.
#[derive(Clone, Copy, Debug)]
pub struct Sighting<'a> {
    pub chains: &'a SectionChains,
    /// The process of each other app's item.
    pub owners: &'a [(WindowId, i32)],
    /// The sections the menu bar state shows, `Section::index` order.
    pub shown: [bool; 3],
    /// Nothing was folded, so each shown section holds exactly the items
    /// read.
    pub complete: bool,
}

/// The items of each section, left to right, `Section::index` order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    sections: [Vec<CatalogItem>; 3],
}

fn chain(chains: &SectionChains, section: Section) -> &[WindowRecord] {
    match section {
        Section::Visible => &chains.visible,
        Section::Hidden => &chains.hidden,
        Section::AlwaysHidden => &chains.always_hidden,
    }
}

impl Catalog {
    pub fn section(&self, section: Section) -> &[CatalogItem] {
        &self.sections[section.index()]
    }

    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(Vec::is_empty)
    }

    /// The section `id` was last seen in.
    pub fn section_of(&self, id: WindowId) -> Option<Section> {
        Section::ALL
            .into_iter()
            .find(|&s| self.section(s).iter().any(|item| item.id == id))
    }

    pub fn item(&self, id: WindowId) -> Option<&CatalogItem> {
        self.sections.iter().flatten().find(|item| item.id == id)
    }

    /// Takes in one read. For each shown section the items read are its
    /// items now. After a complete read nothing else is. After a partial
    /// one the section's items that were not read stay, left of those
    /// read: items fold from the left, so the ones missing are the
    /// leftmost. An item read in one section leaves the others.
    pub fn note(&mut self, seen: Sighting<'_>) {
        for section in Section::ALL {
            if !seen.shown[section.index()] {
                continue;
            }
            let read: Vec<CatalogItem> = chain(seen.chains, section)
                .iter()
                .map(|w| CatalogItem {
                    id: w.id,
                    name: w.title_str().unwrap_or("Item").to_string(),
                    size: (w.bounds.width, w.bounds.height),
                    pid: seen
                        .owners
                        .iter()
                        .find(|(id, _)| *id == w.id)
                        .map(|(_, pid)| *pid),
                })
                .collect();
            for other in &mut self.sections {
                other.retain(|item| !read.iter().any(|r| r.id == item.id));
            }
            let list = &mut self.sections[section.index()];
            if seen.complete {
                *list = read;
            } else {
                list.extend(read);
            }
        }
    }

    /// Forgets the items `keep` turns down, such as those of apps that
    /// quit; `keep` may update an item, such as the process of an app that
    /// relaunched.
    pub fn retain(&mut self, mut keep: impl FnMut(&mut CatalogItem) -> bool) {
        for list in &mut self.sections {
            list.retain_mut(|item| keep(item));
        }
    }

    /// The sections as window records laid out left to right from x 0, for
    /// a `BarModel`.
    pub fn chains(&self) -> SectionChains {
        let mut x = 0.0;
        let mut records = |section: Section| -> Vec<WindowRecord> {
            self.section(section)
                .iter()
                .map(|item| {
                    let record = WindowRecord {
                        id: item.id,
                        layer: STATUS_LAYER,
                        bounds: Rect::new(x, 0.0, item.size.0, item.size.1),
                        title: Some(item.name.clone()),
                        owner_name: None,
                        on_screen: false,
                    };
                    x += item.size.0;
                    record
                })
                .collect()
        };
        let always_hidden = records(Section::AlwaysHidden);
        let hidden = records(Section::Hidden);
        let visible = records(Section::Visible);
        SectionChains {
            visible,
            hidden,
            always_hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: WindowId, name: &str) -> WindowRecord {
        WindowRecord {
            id,
            layer: STATUS_LAYER,
            bounds: Rect::new(0.0, 0.0, 30.0, 30.0),
            title: Some(name.to_string()),
            owner_name: None,
            on_screen: true,
        }
    }

    fn chains(visible: &[u32], hidden: &[u32], always: &[u32]) -> SectionChains {
        let list = |ids: &[u32]| {
            ids.iter()
                .map(|&id| record(id, &format!("D{id}")))
                .collect()
        };
        SectionChains {
            visible: list(visible),
            hidden: list(hidden),
            always_hidden: list(always),
        }
    }

    fn ids(catalog: &Catalog, section: Section) -> Vec<WindowId> {
        catalog
            .section(section)
            .iter()
            .map(|item| item.id)
            .collect()
    }

    const ALL: [bool; 3] = [true, true, true];
    const HIDDEN_SHOWN: [bool; 3] = [true, true, false];
    const ALL_HIDDEN: [bool; 3] = [true, false, false];

    #[test]
    fn a_complete_read_sets_every_section() {
        let mut catalog = Catalog::default();
        let seen = chains(&[1, 2], &[3, 4, 5], &[6]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[(3, 700)],
            shown: ALL,
            complete: true,
        });
        assert_eq!(ids(&catalog, Section::Visible), [1, 2]);
        assert_eq!(ids(&catalog, Section::Hidden), [3, 4, 5]);
        assert_eq!(ids(&catalog, Section::AlwaysHidden), [6]);
        assert_eq!(catalog.item(3).unwrap().pid, Some(700));
        assert_eq!(catalog.item(4).unwrap().pid, None);
        assert_eq!(catalog.item(4).unwrap().name, "D4");
        assert_eq!(catalog.section_of(6), Some(Section::AlwaysHidden));
        // A later complete read replaces what was there.
        let seen = chains(&[1], &[2, 3], &[]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: ALL,
            complete: true,
        });
        assert_eq!(ids(&catalog, Section::Visible), [1]);
        assert_eq!(ids(&catalog, Section::Hidden), [2, 3]);
        assert!(ids(&catalog, Section::AlwaysHidden).is_empty());
    }

    #[test]
    fn sections_not_shown_keep_what_was_seen() {
        let mut catalog = Catalog::default();
        let seen = chains(&[1], &[3, 4], &[6]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: ALL,
            complete: true,
        });
        // Everything hidden: only the visible section is read.
        let seen = chains(&[1, 9], &[], &[]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: ALL_HIDDEN,
            complete: true,
        });
        assert_eq!(ids(&catalog, Section::Visible), [1, 9]);
        assert_eq!(ids(&catalog, Section::Hidden), [3, 4]);
        assert_eq!(ids(&catalog, Section::AlwaysHidden), [6]);
    }

    #[test]
    fn a_partial_read_keeps_the_folded_items_on_the_left() {
        let mut catalog = Catalog::default();
        let seen = chains(&[1], &[3, 4, 5], &[6]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: ALL,
            complete: true,
        });
        // Hidden section shown on a crowded bar: 3 folded, 7 is new.
        let seen = chains(&[1], &[4, 5, 7], &[]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: HIDDEN_SHOWN,
            complete: false,
        });
        assert_eq!(ids(&catalog, Section::Hidden), [3, 4, 5, 7]);
        assert_eq!(ids(&catalog, Section::AlwaysHidden), [6]);
    }

    #[test]
    fn an_item_seen_elsewhere_moves() {
        let mut catalog = Catalog::default();
        let seen = chains(&[1], &[3, 4], &[6]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: ALL,
            complete: true,
        });
        // 6 was dragged into the hidden section.
        let seen = chains(&[1], &[6, 3, 4], &[]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[],
            shown: HIDDEN_SHOWN,
            complete: false,
        });
        assert_eq!(ids(&catalog, Section::Hidden), [6, 3, 4]);
        assert!(ids(&catalog, Section::AlwaysHidden).is_empty());
    }

    #[test]
    fn retain_and_chains() {
        let mut catalog = Catalog::default();
        let seen = chains(&[1], &[3, 4], &[6]);
        catalog.note(Sighting {
            chains: &seen,
            owners: &[(3, 700), (4, 800)],
            shown: ALL,
            complete: true,
        });
        catalog.retain(|item| item.pid != Some(800));
        assert_eq!(ids(&catalog, Section::Hidden), [3]);
        let laid_out = catalog.chains();
        assert_eq!(laid_out.always_hidden[0].id, 6);
        assert_eq!(laid_out.always_hidden[0].bounds.x, 0.0);
        assert_eq!(laid_out.hidden[0].bounds.x, 30.0);
        assert_eq!(laid_out.visible[0].bounds.x, 60.0);
        assert_eq!(laid_out.hidden[0].title_str(), Some("D3"));
        assert!(!catalog.is_empty());
        assert!(Catalog::default().is_empty());
    }
}
