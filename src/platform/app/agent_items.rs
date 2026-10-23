//! The Ice Bar, search and Hide app menus on the macOS 27 menu bar (v1.12).
//! Hidden items are folded away there, with no place or image to read, so
//! each read of MenuBarAgent's bar taken while a section is shown goes into
//! `crate::agent_catalog` (which section each item is in) and, while the
//! Ice Bar or search wants them, item images cropped from a capture of the
//! screen with the bar cut away (`crate::glyph`). The Ice Bar and the
//! search panel list the items from there. A click shows the item's
//! section, waits for MenuBarAgent to lay the item out and clicks it where
//! it is, like the macOS 26 click-through. See docs/macos27.md.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSImage, NSRunningApplication, NSWorkspace,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSCopying, NSSize, NSString};

use super::{
    Controller, MSG_NO_ALWAYS_HIDDEN_ITEMS, MSG_NO_EVENT_ACCESS, MSG_NO_HIDDEN_ITEMS, message_item,
};
use crate::agent_bar::AgentLayout;
use crate::agent_catalog::{Catalog, CatalogItem, Sighting};
use crate::arrange::{Anchors, BarModel};
use crate::model::{Rect, WindowId};
use crate::platform::capture;
use crate::platform::events::{self, InputError};
use crate::platform::ice_bar::BarItem;
use crate::platform::item_names;
use crate::platform::layout_pane::{LayoutSnapshot, TileImage};
use crate::platform::{menu_bar_space, permissions, timer};
use crate::sections::{MenuBarState, Section};

/// An item image is taken again when the item shows after this long.
const IMAGE_MAX_AGE: Duration = Duration::from_secs(30 * 60);
/// Shortest time between two captures: each shows macOS's purple capture
/// dot for about 10 s (macOS 27.0.1).
const CAPTURE_GAP: Duration = Duration::from_secs(5);
/// How long a section shown to learn its items stays before it hides and
/// the Ice Bar opens: MenuBarAgent lays items out in about 0.5 s.
const PEEK_READ: Duration = Duration::from_millis(700);
/// A section that showed nothing when shown is not shown again for this
/// long; the Ice Bar says it is empty instead.
const PEEK_AGAIN: Duration = Duration::from_secs(60);
/// How often a click checks whether MenuBarAgent has laid the item out.
const CLICK_POLL: Duration = Duration::from_millis(100);
/// How long a click waits for the item to be laid out.
const CLICK_TIMEOUT: Duration = Duration::from_secs(2);
/// The height of the app icon that stands in for an item without an image.
const ICON_HEIGHT: f64 = 18.0;

/// An item image without the bar behind it.
struct Glyph {
    image: CFRetained<CGImage>,
    /// Pixels per point.
    scale: f64,
    taken: Instant,
}

#[derive(Default)]
pub(super) struct AgentItems {
    catalog: RefCell<Catalog>,
    glyphs: RefCell<HashMap<WindowId, Glyph>>,
    /// Whether the glyphs are light, from the last capture.
    light_glyphs: Cell<Option<bool>>,
    /// Each app item's bundle identifier, to find the app again after it
    /// relaunched.
    bundles: RefCell<HashMap<WindowId, String>>,
    /// Our icon and dividers in the last read, for `BarModel`.
    anchors: Cell<Option<Anchors>>,
    last_capture: Cell<Option<Instant>>,
    /// The search panel was opened this run, so it wants images too.
    search_used: Cell<bool>,
    /// When a section was last shown to learn its items.
    peeked: Cell<Option<(Instant, Section)>>,
    /// The apps that ran when every section was last read. An app that
    /// launched since has its items wherever MenuBarAgent kept them,
    /// unseen until a section shows. macOS posts no launch notification
    /// for apps without a Dock icon, which most apps with only status
    /// items are, so new apps are found by comparing with this.
    seen_apps: RefCell<Option<HashSet<i32>>>,
}

/// The apps that can own status items (not background-only ones).
fn running_apps() -> HashSet<i32> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() != NSApplicationActivationPolicy::Prohibited)
        .map(|app| app.processIdentifier())
        .collect()
}

/// What follows a peek.
type AfterPeek = Box<dyn FnOnce(&Controller)>;

fn ns_image(glyph: &Glyph) -> (Retained<NSImage>, (f64, f64)) {
    let w = CGImage::width(Some(&glyph.image)) as f64 / glyph.scale;
    let h = CGImage::height(Some(&glyph.image)) as f64 / glyph.scale;
    let image = NSImage::initWithCGImage_size(NSImage::alloc(), &glyph.image, NSSize::new(w, h));
    (image, (w, h))
}

/// The app's icon at the size of a menu bar glyph, for an item without an
/// image of its own.
fn app_icon(id: WindowId) -> Option<Retained<NSImage>> {
    let icon = item_names::icon(id)?;
    let icon = icon.copy();
    icon.setSize(NSSize::new(ICON_HEIGHT, ICON_HEIGHT));
    Some(icon)
}

impl Controller {
    /// Whether item images are worth a capture: the Ice Bar or the search
    /// panel shows them. Menu bar mode alone never captures.
    fn wants_item_images(&self) -> bool {
        self.settings.borrow().use_ice_bar || self.agent_items.search_used.get()
    }

    /// Takes in a read of MenuBarAgent's bar (`read_agent_bar`). Reads made
    /// while MenuBarAgent still moves items after a change are left out.
    pub(super) fn note_agent_read(&self, layout: &AgentLayout) {
        if !self.agent_settled() {
            return;
        }
        let state = self.status.state();
        let shown = [
            true,
            state.is_shown(Section::Hidden),
            state.is_shown(Section::AlwaysHidden),
        ];
        self.agent_items.catalog.borrow_mut().note(Sighting {
            chains: &layout.chains,
            owners: &layout.owners,
            shown,
            // A shown section folds only into the overflow control.
            complete: layout.overflow.is_none(),
        });
        self.agent_items.anchors.set(Some(layout.anchors));
        // The first read stands in until every section is read whole.
        let mut seen = self.agent_items.seen_apps.borrow_mut();
        if seen.is_none() || (shown == [true; 3] && layout.overflow.is_none()) {
            *seen = Some(running_apps());
        }
        drop(seen);
        {
            let mut bundles = self.agent_items.bundles.borrow_mut();
            for &(id, pid) in &layout.owners {
                if let Some(bundle) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                        .and_then(|app| app.bundleIdentifier())
                {
                    bundles.insert(id, bundle.to_string());
                }
            }
        }
        if self.wants_item_images() {
            self.capture_glyphs(layout, shown);
        }
    }

    /// Captures the images of the items on the bar in the shown sections
    /// when one has none or an old one.
    fn capture_glyphs(&self, layout: &AgentLayout, shown: [bool; 3]) {
        if self
            .agent_items
            .last_capture
            .get()
            .is_some_and(|at| at.elapsed() < CAPTURE_GAP)
        {
            return;
        }
        let chains = [
            &layout.chains.visible,
            &layout.chains.hidden,
            &layout.chains.always_hidden,
        ];
        let items: Vec<(WindowId, Rect)> = chains
            .into_iter()
            .zip(shown)
            .filter(|(_, on)| *on)
            .flat_map(|(chain, _)| chain.iter())
            .filter(|w| Some(w.id) != layout.anchors.icon)
            .map(|w| (w.id, w.bounds))
            .collect();
        let wanted = {
            let glyphs = self.agent_items.glyphs.borrow();
            items.iter().any(|(id, _)| {
                glyphs
                    .get(id)
                    .is_none_or(|g| g.taken.elapsed() > IMAGE_MAX_AGE)
            })
        };
        if !wanted || !permissions::check().screen_recording {
            return;
        }
        let Some((display, height)) = self.agent_display() else {
            return;
        };
        let bottom = items
            .iter()
            .map(|(_, r)| r.max_y() - display.y)
            .fold(height, f64::max);
        let region = Rect::new(display.x, display.y, display.width, bottom);
        self.agent_items.last_capture.set(Some(Instant::now()));
        let rects: Vec<Rect> = items.iter().map(|(_, r)| *r).collect();
        match capture::capture_glyphs_bounded(region, rects) {
            Ok((images, tone)) => {
                let now = Instant::now();
                let mut glyphs = self.agent_items.glyphs.borrow_mut();
                let mut stored = 0;
                for ((id, rect), image) in items.iter().zip(images) {
                    if let Some(image) = image {
                        let scale = CGImage::width(Some(&image)) as f64 / rect.width.max(1.0);
                        glyphs.insert(
                            *id,
                            Glyph {
                                image,
                                scale,
                                taken: now,
                            },
                        );
                        stored += 1;
                    }
                }
                if tone.is_some() {
                    self.agent_items.light_glyphs.set(tone);
                }
                eprintln!(
                    "ice-rust: took {stored} of {} item images from the menu bar",
                    items.len()
                );
            }
            Err(err) => eprintln!("ice-rust: item image capture {err}"),
        }
    }

    /// Forgets the items of apps that quit, and follows apps that
    /// relaunched to their new process.
    fn forget_gone_items(&self) {
        let bundles = self.agent_items.bundles.borrow();
        let mut gone = Vec::new();
        self.agent_items
            .catalog
            .borrow_mut()
            .retain(|item: &mut CatalogItem| {
                let Some(pid) = item.pid else {
                    return true;
                };
                let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                    .is_some_and(|app| !app.isTerminated());
                if running {
                    return true;
                }
                let relaunched = bundles.get(&item.id).and_then(|bundle| {
                    NSRunningApplication::runningApplicationsWithBundleIdentifier(
                        &NSString::from_str(bundle),
                    )
                    .iter()
                    .next()
                    .map(|app| app.processIdentifier())
                });
                match relaunched {
                    Some(pid) => {
                        item.pid = Some(pid);
                        true
                    }
                    None => {
                        gone.push(item.id);
                        false
                    }
                }
            });
        let mut glyphs = self.agent_items.glyphs.borrow_mut();
        for id in gone {
            glyphs.remove(&id);
        }
    }

    /// Whether an app has more status items than the catalog knows: one
    /// that launched since every section was read, or one whose items were
    /// seen. Each app's extras menu bar lists its folded items too.
    fn catalog_stale(&self) -> bool {
        let own = std::process::id() as i32;
        let mut pids: Vec<i32> = match &*self.agent_items.seen_apps.borrow() {
            Some(seen) => running_apps().difference(seen).copied().collect(),
            None => Vec::new(),
        };
        let catalog = self.agent_items.catalog.borrow();
        let known = |pid: i32| {
            Section::ALL
                .iter()
                .flat_map(|&s| catalog.section(s))
                .filter(|item| item.pid == Some(pid))
                .count()
        };
        for section in Section::ALL {
            pids.extend(catalog.section(section).iter().filter_map(|item| item.pid));
        }
        pids.sort_unstable();
        pids.dedup();
        pids.into_iter()
            .filter(|&pid| pid != own)
            .any(|pid| menu_bar_space::extras_item_count(pid).is_some_and(|n| n > known(pid)))
    }

    /// `open_bar` on macOS 27: the section's items as last seen. When none
    /// are known, or an app has items the catalog does not know, every
    /// section is shown once to learn them first.
    pub(super) fn agent_open_bar(&self, section: Section) {
        self.search.close();
        self.forget_gone_items();
        let empty = self
            .agent_items
            .catalog
            .borrow()
            .section(section)
            .is_empty();
        let peek = if empty {
            Some(section)
        } else if self.catalog_stale() {
            Some(Section::AlwaysHidden)
        } else {
            None
        };
        if let Some(peek) = peek
            && self.agent_peek(peek, Box::new(move |c| c.agent_show_bar(section)))
        {
            return;
        }
        self.agent_show_bar(section);
    }

    fn agent_show_bar(&self, section: Section) {
        let catalog = self.agent_items.catalog.borrow().clone();
        let known = catalog.section(section);
        let all = crate::platform::windows::list_windows();
        let Some((place, anchor)) = self.bar_place(&all, None) else {
            eprintln!("ice-rust: cannot find the active menu bar display");
            return;
        };
        let (items, sizes) = if known.is_empty() {
            message_item(match section {
                Section::AlwaysHidden => MSG_NO_ALWAYS_HIDDEN_ITEMS,
                _ => MSG_NO_HIDDEN_ITEMS,
            })
        } else {
            let pairs: Vec<(BarItem, (f64, f64))> =
                known.iter().map(|item| self.bar_item(item)).collect();
            item_names::note_owners(
                &known
                    .iter()
                    .filter_map(|item| item.pid.map(|pid| (item.id, pid)))
                    .collect::<Vec<_>>(),
            );
            pairs.into_iter().unzip()
        };
        let appearance = NSApplication::sharedApplication(self.mtm).effectiveAppearance();
        let color = crate::brand::bar_fill_for(
            self.agent_items.light_glyphs.get(),
            None,
            crate::platform::ui::is_dark(&appearance),
        );
        self.bar_section.set(section);
        self.present(&place, anchor, &items, &sizes, Some(color));
    }

    /// An Ice Bar tile for `item`: its image, else its app's icon, else its
    /// name.
    fn bar_item(&self, item: &CatalogItem) -> (BarItem, (f64, f64)) {
        let title = item_names::name(item.id, &item.name);
        let glyphs = self.agent_items.glyphs.borrow();
        let (image, size) = match glyphs.get(&item.id) {
            Some(glyph) => {
                let (image, size) = ns_image(glyph);
                (Some(image), size)
            }
            None => (app_icon(item.id), item.size),
        };
        (
            BarItem {
                id: item.id,
                title,
                image,
                clickable: true,
            },
            size,
        )
    }

    /// Shows `section` in the menu bar for a moment to learn its items,
    /// then runs `then`. False when it was shown a short while ago; `then`
    /// is not run.
    fn agent_peek(&self, section: Section, then: AfterPeek) -> bool {
        if self
            .agent_items
            .peeked
            .get()
            .is_some_and(|(at, s)| s == section && at.elapsed() < PEEK_AGAIN)
        {
            return false;
        }
        self.agent_items.peeked.set(Some((Instant::now(), section)));
        eprintln!("ice-rust: showing the {section:?} section once to learn its items");
        let previous = self.menu_state();
        self.apply_state(previous.show(section));
        let me = self.me.clone();
        // The timer takes an `Fn`; it fires once.
        let then = Cell::new(Some(then));
        timer::after(self.mtm, PEEK_READ, move || {
            if let Some(c) = me.upgrade() {
                c.read_agent_bar();
                if previous.show(section) == MenuBarState::AllShown {
                    *c.agent_items.seen_apps.borrow_mut() = Some(running_apps());
                }
                c.apply_state(previous);
                if let Some(then) = then.take() {
                    then(&c);
                }
            }
        });
        true
    }

    /// `toggle_search` on macOS 27: shows every section once first when an
    /// app has items the catalog does not know.
    pub(super) fn agent_open_search(&self) {
        self.agent_items.search_used.set(true);
        self.forget_gone_items();
        let open = |c: &Controller| {
            let snapshot = c.agent_search_snapshot();
            c.search.show(&snapshot);
        };
        if (self.agent_items.catalog.borrow().is_empty() || self.catalog_stale())
            && self.agent_peek(Section::AlwaysHidden, Box::new(open))
        {
            return;
        }
        open(self);
    }

    /// `click_through` on macOS 27: shows the item's section, waits until
    /// MenuBarAgent has laid the item out and it stays put for one poll,
    /// then clicks it there.
    pub(super) fn agent_click_through(&self, id: WindowId, section: Section) {
        self.close_bar();
        if section != Section::Visible {
            self.reveal(section);
        }
        if !permissions::check().can_post_events() {
            permissions::request_event_access();
            self.show_message(MSG_NO_EVENT_ACCESS);
            return;
        }
        if let Some(old) = self.settle_timer.borrow_mut().take() {
            old.invalidate();
        }
        let deadline = Cell::new(Instant::now() + CLICK_TIMEOUT);
        let last = Cell::new(None::<Rect>);
        let me = self.me.clone();
        let timer = timer::schedule(self.mtm, CLICK_POLL, true, move |timer| {
            let Some(c) = me.upgrade() else {
                timer.invalidate();
                return;
            };
            let done = |c: &Controller| {
                timer.invalidate();
                c.settle_timer.borrow_mut().take();
            };
            if c.mover.is_busy() || events::is_posting() {
                deadline.set(Instant::now() + CLICK_TIMEOUT);
                return;
            }
            if Instant::now() >= deadline.get() {
                eprintln!("ice-rust: item {id} was not laid out within {CLICK_TIMEOUT:?}");
                done(&c);
                return;
            }
            if !c.agent_settled() {
                return;
            }
            let frame = c.read_agent_bar().and_then(|layout| layout.frame(id));
            let still = frame.is_some() && frame == last.replace(frame);
            let Some(frame) = frame.filter(|_| still) else {
                return;
            };
            match events::click_point(c.mtm, (frame.mid_x(), frame.mid_y())) {
                Ok(()) => done(&c),
                Err(InputError::Busy) => {}
                Err(err) => {
                    eprintln!("ice-rust: click on item {id} failed: {err}");
                    done(&c);
                }
            }
        });
        *self.settle_timer.borrow_mut() = Some(timer);
    }

    /// `load_layout` for the search panel on macOS 27: every section as
    /// last seen, with the visible section read now.
    pub(super) fn agent_search_snapshot(&self) -> LayoutSnapshot {
        self.agent_items.search_used.set(true);
        self.read_agent_bar();
        self.forget_gone_items();
        let catalog = self.agent_items.catalog.borrow();
        let Some(anchors) = self.agent_items.anchors.get() else {
            return LayoutSnapshot::empty("The menu bar could not be read");
        };
        if catalog.is_empty() {
            return LayoutSnapshot::empty("The menu bar could not be read");
        }
        let model = BarModel::from_sections(&catalog.chains(), &self.names, anchors);
        let glyphs = self.agent_items.glyphs.borrow();
        let images = glyphs
            .iter()
            .map(|(&id, glyph)| {
                let (image, size) = ns_image(glyph);
                (
                    id,
                    TileImage {
                        image: Some(image),
                        size,
                    },
                )
            })
            .collect();
        let mut snapshot = LayoutSnapshot::empty("");
        snapshot.notice = None;
        snapshot.model = Some(model);
        snapshot.images = images;
        snapshot.light_glyphs = self.agent_items.light_glyphs.get();
        if let Some((_, height)) = self.agent_display() {
            snapshot.menu_bar_height = height;
        }
        snapshot
    }

    /// `items_reach_app_menus` on macOS 27: items of a shown section that
    /// the bar folded away for lack of room. With every section shown any
    /// fold counts. Otherwise a folded item counts unless it is known to be
    /// in a hidden section (the always-hidden items fold on purpose), and
    /// so does an item known to be in a shown hidden section that is not on
    /// the bar: MenuBarAgent lists only some folded items. `None` without
    /// a read.
    pub(super) fn agent_items_reach_app_menus(&self) -> Option<bool> {
        // Mid-change the bar folds items it is about to lay out.
        if !self.agent_settled() {
            return None;
        }
        let layout = self.read_agent_bar()?;
        let state = self.status.state();
        if state == MenuBarState::AllShown && layout.overflow.is_some() {
            return Some(true);
        }
        self.forget_gone_items();
        let on_bar: HashSet<WindowId> = layout.frames.iter().map(|(id, _)| *id).collect();
        let catalog = self.agent_items.catalog.borrow();
        // The visible section is left out: it is shown in every state, so
        // an item that left it unseen would hide the menus for good.
        let missing = [Section::Hidden, Section::AlwaysHidden]
            .into_iter()
            .filter(|&s| state.is_shown(s))
            .flat_map(|s| catalog.section(s))
            .any(|item| !on_bar.contains(&item.id));
        let folded_shown = layout
            .folded_ids
            .iter()
            .any(|&id| catalog.section_of(id).is_none_or(|s| state.is_shown(s)));
        Some(missing || folded_shown)
    }
}
