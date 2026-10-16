//! Plain data types and constants shared by every module.

use std::time::Duration;

/// A window server window number (`CGWindowID`).
pub type WindowId = u32;

/// Autosave name of the always-visible icon item.
pub const ICON_AUTOSAVE: &str = "IceRust.ControlItem.Visible";
/// Autosave name of the hidden-section divider item.
pub const DIVIDER_AUTOSAVE: &str = "IceRust.ControlItem.Hidden";
/// Autosave name of the always-hidden-section divider item.
pub const ALWAYS_HIDDEN_AUTOSAVE: &str = "IceRust.ControlItem.AlwaysHidden";
/// Owner of the macOS 27 menu bar window, which draws every status item.
pub const MENU_BAR_AGENT_OWNER: &str = "MenuBarAgent";
/// Autosave name of the spacer left of the hidden divider (macOS 27, v1.8).
pub const SPACER_AUTOSAVE: &str = "IceRust.ControlItem.Spacer";
/// Autosave name of the spacer left of the always-hidden divider (macOS 27,
/// v1.8).
pub const ALWAYS_HIDDEN_SPACER_AUTOSAVE: &str = "IceRust.ControlItem.AlwaysHiddenSpacer";
/// Common prefix of our three autosave names.
pub const CONTROL_ITEM_PREFIX: &str = "IceRust.ControlItem.";
/// Prefix of Ice's control item titles (ledger section 2).
pub const ICE_CONTROL_ITEM_PREFIX: &str = "Ice.ControlItem.";

/// Frames of our three control item windows in CG space, read from AppKit.
/// They find the windows when the titles do not (issue #1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlFrames {
    pub icon: Option<Rect>,
    pub hidden: Option<Rect>,
    pub always_hidden: Option<Rect>,
}

/// The three control item titles that share a prefix: `<prefix>Visible`,
/// `<prefix>Hidden` and `<prefix>AlwaysHidden`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlNames {
    pub icon: String,
    pub hidden: String,
    pub always_hidden: String,
}

impl ControlNames {
    pub fn with_prefix(prefix: &str) -> Self {
        Self {
            icon: format!("{prefix}Visible"),
            hidden: format!("{prefix}Hidden"),
            always_hidden: format!("{prefix}AlwaysHidden"),
        }
    }

    /// Our own names.
    pub fn ours() -> Self {
        Self::with_prefix(CONTROL_ITEM_PREFIX)
    }

    /// Whether `title` is one of the three.
    pub fn contains(&self, title: &str) -> bool {
        title == self.icon || title == self.hidden || title == self.always_hidden
    }
}

/// Window title of the Control Center item on macOS 26 (ledger section 3).
pub const CONTROL_CENTER_TITLE: &str = "BentoBox-0";
/// Titles of system items that cannot be moved (ledger section 3: Clock and
/// Control Center).
pub const LOCKED_TITLES: [&str; 2] = ["Clock", CONTROL_CENTER_TITLE];

/// Divider length that pushes items left of it off every display (M1).
pub const HIDE_LENGTH: f64 = 10000.0;
/// `NSVariableStatusItemLength` (M1).
pub const VARIABLE_LENGTH: f64 = -1.0;

/// `kCGStatusWindowLevel`, the layer of every status item window (M4).
pub const STATUS_LAYER: i32 = 25;
/// `kCGMainMenuWindowLevel`, the layer of the menu bar backdrop windows.
pub const MAIN_MENU_LAYER: i32 = 24;
/// `kCGPopUpMenuWindowLevel`.
pub const POPUP_MENU_LAYER: i32 = 101;
/// Layer of the menus Control Center opens for the status items it hosts on
/// macOS 26 (observed on 26.3 for the AirDrop item).
pub const CONTROL_CENTER_MENU_LAYER: i32 = 100;

/// Whether an on-screen window at `layer` is an open menu.
pub fn is_menu_layer(layer: i32) -> bool {
    layer == POPUP_MENU_LAYER || layer == CONTROL_CENTER_MENU_LAYER
}

/// Adjacent items abut exactly; this absorbs rounding (M6).
pub const ABUT_TOLERANCE: f64 = 1.0;
/// Any status window at least this wide is treated as an expanded divider.
pub const DIVIDER_MIN_WIDTH: f64 = 1000.0;
/// Upper bound on chain walk steps.
pub const MAX_CHAIN_STEPS: usize = 256;

/// `kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution` (M7).
pub const CAPTURE_OPTION: u32 = 9;

/// `NSWindowStyleMaskNonactivatingPanel | NSWindowStyleMaskFullSizeContentView` (M8).
pub const PANEL_STYLE_MASK: usize = 0x8080;
/// Panel window level (M8).
pub const PANEL_LEVEL: isize = 25;
/// `fullScreenAuxiliary | ignoresCycle | moveToActiveSpace` (M8).
pub const PANEL_COLLECTION_BEHAVIOR: usize = 322;

/// `kCGMouseEventClickState` (M9).
pub const FIELD_CLICK_STATE: u32 = 1;
/// `kCGMouseEventWindowUnderMousePointer` (M9).
pub const FIELD_WINDOW_UNDER_POINTER: u32 = 91;
/// `kCGMouseEventWindowUnderMousePointerThatCanHandleThisEvent` (M9).
pub const FIELD_WINDOW_THAT_CAN_HANDLE: u32 = 92;

/// Default idle time before a revealed section hides again.
pub const REHIDE_INTERVAL: Duration = Duration::from_secs(15);
/// How often the rehide state machine is ticked.
pub const REHIDE_TICK: Duration = Duration::from_millis(500);
/// How long click-through waits for the item to appear on screen.
pub const CLICK_SETTLE_TIMEOUT: Duration = Duration::from_secs(1);
/// `kCGEventSourceUserData`: carries the tag of each posted event (G4).
pub const FIELD_EVENT_SOURCE_USER_DATA: u32 = 42;
/// `kVK_Command`, the left Command key.
pub const KEY_CODE_COMMAND: u16 = 55;
/// The cursor is restored after this long when the event tap never sees
/// the last posted event (G4).
pub const CURSOR_RESTORE_FALLBACK: Duration = Duration::from_millis(300);
/// Delay between the tap seeing the last event and the cursor restore, so
/// the event reaches its target first.
pub const CURSOR_RESTORE_SETTLE: Duration = Duration::from_millis(0);
/// Longest wait for a window capture on the worker thread (G5).
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(1);
/// Poll period while waiting for the item to appear on screen.
pub const CLICK_SETTLE_POLL: Duration = Duration::from_millis(20);

/// Move engine: poll period while waiting for sections, idle and placement.
pub const MOVE_POLL: Duration = Duration::from_millis(20);
/// Move engine: how long the touched sections may take to appear.
pub const MOVE_SHOW_TIMEOUT: Duration = Duration::from_secs(1);
/// Move engine: the user counts as idle after this long without input.
pub const MOVE_IDLE_QUIET: Duration = Duration::from_millis(50);
/// Move engine: give up with "user busy" after waiting this long.
pub const MOVE_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Move engine: number of dragged events between down and up.
pub const MOVE_DRAG_STEPS: usize = 8;
/// Move engine: pause between posted drag events.
pub const MOVE_EVENT_INTERVAL: Duration = Duration::from_millis(10);
/// Move engine: drop point offset from the target edge, in points.
pub const MOVE_DROP_INSET: f64 = 2.0;
/// Move engine: extra drop offset per retry, in the travel direction.
pub const MOVE_DROP_NUDGE: f64 = 4.0;
/// Move engine: retries after the first attempt.
pub const MOVE_RETRIES: usize = 3;
/// Move engine: how long placement is polled after each attempt.
pub const MOVE_VERIFY_TIMEOUT: Duration = Duration::from_millis(500);

/// Settings window: Menu Bar Layout refresh period while visible and idle.
pub const LAYOUT_REFRESH: Duration = Duration::from_secs(3);
/// Settings window: Permissions refresh period while visible.
pub const PERMISSIONS_REFRESH: Duration = Duration::from_secs(1);

/// Padding between and around item images in the Ice Bar panel.
pub const BAR_PADDING: f64 = 4.0;
/// A press in the Ice Bar counts as a click only when released within this
/// many seconds...
pub const CLICK_MAX_SECONDS: f64 = 0.5;
/// ...and after the pointer moved less than this many points.
pub const CLICK_MAX_TRAVEL: f64 = 5.0;

/// Title of the Window Server menu bar backdrop window (layer 24).
pub const MENU_BAR_WINDOW_TITLE: &str = "Menubar";
/// Owner name of the menu bar backdrop window.
pub const WINDOW_SERVER_OWNER: &str = "Window Server";
/// Owner and title prefix of the desktop picture window (ledger section 5).
pub const WALLPAPER_OWNER: &str = "Dock";
pub const WALLPAPER_TITLE_PREFIX: &str = "Wallpaper";
/// `kCGWindowImageNominalResolution`, used for the menu bar color sample.
pub const COLOR_CAPTURE_OPTION: u32 = 16;

/// An axis-aligned rectangle. The coordinate space is implied by context.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    pub fn min_y(&self) -> f64 {
        self.y
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Half-open containment: the min edges are inside, the max edges are not.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.min_x() && x < self.max_x() && y >= self.min_y() && y < self.max_y()
    }

    /// Smallest rectangle containing both. An empty rectangle is ignored.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let min_x = self.min_x().min(other.min_x());
        let min_y = self.min_y().min(other.min_y());
        let max_x = self.max_x().max(other.max_x());
        let max_y = self.max_y().max(other.max_y());
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

/// One entry of the window server's window list.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowRecord {
    pub id: WindowId,
    pub layer: i32,
    /// Bounds in CG space (top-left origin).
    pub bounds: Rect,
    /// `kCGWindowName`. `None` when absent or unreadable without Screen Recording.
    pub title: Option<String>,
    pub owner_name: Option<String>,
    pub on_screen: bool,
}

impl WindowRecord {
    /// The title, treating an empty string the same as a missing one.
    pub fn title_str(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_layers_cover_app_and_control_center_menus() {
        assert!(is_menu_layer(POPUP_MENU_LAYER));
        assert!(is_menu_layer(CONTROL_CENTER_MENU_LAYER));
        assert!(!is_menu_layer(STATUS_LAYER));
        assert!(!is_menu_layer(MAIN_MENU_LAYER));
        assert!(!is_menu_layer(0));
        assert!(!is_menu_layer(20));
    }

    #[test]
    fn rect_edges() {
        let r = Rect::new(-394.0, 0.0, 5016.0, 30.0);
        assert_eq!(r.min_x(), -394.0);
        assert_eq!(r.max_x(), 4622.0);
        assert_eq!(r.max_y(), 30.0);
        assert_eq!(r.mid_x(), 2114.0);
        assert_eq!(r.mid_y(), 15.0);
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(9.99, 9.99));
        assert!(!r.contains(10.0, 5.0));
        assert!(!r.contains(5.0, 10.0));
        assert!(!r.contains(-0.01, 5.0));
    }

    #[test]
    fn rect_union_skips_empty() {
        let a = Rect::new(-498.0, 0.0, 32.0, 30.0);
        let b = Rect::new(-432.0, 0.0, 38.0, 30.0);
        assert_eq!(a.union(&b), Rect::new(-498.0, 0.0, 104.0, 30.0));
        assert_eq!(Rect::default().union(&b), b);
        assert_eq!(a.union(&Rect::default()), a);
    }

    #[test]
    fn empty_title_counts_as_missing() {
        let mut w = WindowRecord {
            id: 1,
            layer: STATUS_LAYER,
            bounds: Rect::new(0.0, 0.0, 10.0, 30.0),
            title: Some(String::new()),
            owner_name: None,
            on_screen: true,
        };
        assert_eq!(w.title_str(), None);
        w.title = Some("Clock".into());
        assert_eq!(w.title_str(), Some("Clock"));
    }

    #[test]
    fn control_names_from_prefix() {
        let ours = ControlNames::ours();
        assert_eq!(ours.icon, ICON_AUTOSAVE);
        assert_eq!(ours.hidden, DIVIDER_AUTOSAVE);
        assert_eq!(ours.always_hidden, ALWAYS_HIDDEN_AUTOSAVE);
        let ice = ControlNames::with_prefix(ICE_CONTROL_ITEM_PREFIX);
        assert_eq!(ice.always_hidden, "Ice.ControlItem.AlwaysHidden");
        assert!(ice.contains("Ice.ControlItem.Hidden"));
        assert!(!ice.contains("IceRust.ControlItem.Hidden"));
        assert!(!ice.contains("Clock"));
    }

    #[test]
    fn capture_option_matches_flag_bits() {
        // boundsIgnoreFraming (1 << 0) | bestResolution (1 << 3)
        assert_eq!(CAPTURE_OPTION, 1 | 8);
        // nonactivatingPanel (1 << 7) | fullSizeContentView (1 << 15)
        assert_eq!(PANEL_STYLE_MASK, (1 << 7) | (1 << 15));
    }
}
