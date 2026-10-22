//! ice-rust: hides menu bar items behind a divider and shows them in a panel.
//! Pure modules carry the logic and tests; `platform` holds the macOS glue.

pub mod agent_bar;
pub mod app_menus;
pub mod arrange;
pub mod bar_layout;
pub mod bar_scroll;
pub mod base64;
pub mod brand;
pub mod chain;
pub mod geometry;
pub mod hotkeys;
pub mod image_check;
pub mod input_tag;
pub mod item_names;
pub mod login_item;
pub mod menu_icon;
pub mod model;
pub mod rehide;
pub mod rehide_policy;
pub mod sections;
pub mod settings;
pub mod triggers;
pub mod update;
pub mod worker;

#[cfg(target_os = "macos")]
pub mod platform;
