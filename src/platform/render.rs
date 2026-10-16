//! `render-settings`: builds the Settings window offscreen and writes one
//! page as PNG. It creates no status items, never orders a window on
//! screen, posts no events, writes no defaults, and moves nothing. The
//! layout page reads the live window list and captures item images
//! (bounded, G5), like `probe`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use super::layout_pane::{LayoutSnapshot, load_order, load_snapshot};
use super::menu_icon::CustomIcon;
use super::mover::{MoveCallback, MoveError, MoveRequest};
use super::settings_window::{DEFAULT_SIZE, Page, SettingsHost, SettingsWindow};
use crate::arrange::BarModel;
use crate::model::{CONTROL_ITEM_PREFIX, ControlFrames, ControlNames, ICE_CONTROL_ITEM_PREFIX};
use crate::sections::Section;
use crate::settings::{self, Settings};

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    pub page: Page,
    pub out: PathBuf,
    pub size: (f64, f64),
    /// Control item title prefix; `None` means ours, falling back to Ice's
    /// when ours are absent.
    pub prefix: Option<String>,
    /// Draw the General page from `Settings::default()` instead of the
    /// stored settings.
    pub default_settings: bool,
}

/// What a render found, for the command's output.
#[derive(Clone, Debug, Default)]
pub struct RenderReport {
    pub pixels: (isize, isize),
    /// Prefix of the dividers the layout page used.
    pub prefix: Option<String>,
    pub notice: Option<String>,
    /// Per section: tiles, tiles with an image, capture timed out.
    pub sections: Vec<(Section, usize, usize, bool)>,
    pub bar_color: Option<[f64; 3]>,
    /// Per bar: scroll offset and the offset that shows its right end.
    pub scroll: Vec<(Section, f64, f64)>,
}

struct RenderHost {
    settings: Settings,
    prefix: Option<String>,
    used: RefCell<Option<String>>,
}

impl SettingsHost for RenderHost {
    fn settings(&self) -> Settings {
        self.settings.clone()
    }

    fn apply_settings(&self, _settings: &Settings) {}

    fn set_custom_icon(&self, _icon: &CustomIcon) {}

    fn load_layout_order(&self) -> Option<BarModel> {
        let prefix = self.prefix.as_deref().unwrap_or(CONTROL_ITEM_PREFIX);
        load_order(
            &ControlNames::with_prefix(prefix),
            &ControlFrames::default(),
        )
    }

    fn load_layout(&self) -> LayoutSnapshot {
        let prefixes: Vec<&str> = match &self.prefix {
            Some(prefix) => vec![prefix],
            None => vec![CONTROL_ITEM_PREFIX, ICE_CONTROL_ITEM_PREFIX],
        };
        let mut last = LayoutSnapshot::empty("Divider not found");
        for prefix in prefixes {
            let snapshot = load_snapshot(
                &ControlNames::with_prefix(prefix),
                &ControlFrames::default(),
            );
            if snapshot.model.is_some() {
                *self.used.borrow_mut() = Some(prefix.to_string());
                return snapshot;
            }
            last = snapshot;
        }
        last
    }

    fn request_move(&self, _request: MoveRequest, done: MoveCallback) {
        done(Err(MoveError::Input(
            "render-settings does not move items".into(),
        )));
    }

    fn refused_hotkeys(&self) -> Vec<crate::hotkeys::HotkeyAction> {
        Vec::new()
    }

    fn closed(&self) {}

    fn check_for_updates(&self) {}

    fn update_status_line(&self) -> String {
        "Not checked for updates yet".into()
    }
}

/// Renders `options.page` to `options.out`.
pub fn render(options: &RenderOptions) -> Result<RenderReport, String> {
    let mtm = MainThreadMarker::new().ok_or("render-settings must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, no menu bar, no activation.
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);

    let host = Rc::new(RenderHost {
        settings: if options.default_settings {
            Settings::default()
        } else {
            settings::load()
        },
        prefix: options.prefix.clone(),
        used: RefCell::new(None),
    });
    let dyn_host: Rc<dyn SettingsHost> = host.clone();
    // Built at the default size with the page loaded, then resized like a
    // live window, so a larger size shows how the window stretches.
    let window = SettingsWindow::new(mtm, dyn_host, false);
    window.select(options.page);
    if options.size != DEFAULT_SIZE {
        window.set_content_size(options.size);
    }
    let pixels = window.render_png(&options.out)?;

    let mut report = RenderReport {
        pixels,
        ..RenderReport::default()
    };
    if options.page == Page::Layout {
        report.prefix = host.used.borrow().clone();
        window.canvas().with_snapshot(|s| {
            report.notice = s.notice.clone();
            report.bar_color = s.bar_color;
            report.sections = s
                .captures
                .iter()
                .map(|&(section, with_image, placeholders, timed_out)| {
                    (section, with_image + placeholders, with_image, timed_out)
                })
                .collect();
            report
                .sections
                .sort_by_key(|(section, ..)| Section::ALL.iter().position(|s| s == section));
        });
        report.scroll = window.canvas().scroll_positions();
    }
    Ok(report)
}
