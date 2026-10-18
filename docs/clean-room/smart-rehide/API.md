# Smart and focused-app rehide: pure API

The public API of `ice_rust::rehide_policy` (`src/rehide_policy.rs`), the
pure half of the rebuild described in [SPEC.md](SPEC.md). Rule and
decision IDs (S2, R10, D5 and so on) refer to that spec. The module uses
`std` only and compiles on any target, so tests need no window server.

Everything below is public at `ice_rust::rehide_policy`. The functions are
pure: the output depends only on the arguments. Nothing reads the clock,
the settings store or the system; the macOS glue
(`src/platform/app/rehide_policy.rs`) gathers the inputs and acts on the
outputs.

```rust
use std::time::Duration;
use ice_rust::rehide_policy::*;
```

## Shared types

### `Section` (re-export of `ice_rust::sections::Section`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section { Visible, Hidden, AlwaysHidden }
```

The three sections (Terms). `Section::ALL` is
`[Section::Visible, Section::Hidden, Section::AlwaysHidden]`.

### `Rect` (re-export of `ice_rust::model::Rect`)

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect { pub x: f64, pub y: f64, pub width: f64, pub height: f64 }
impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Rect;
    pub fn contains(&self, x: f64, y: f64) -> bool;
}
```

A window's bounds (R12, R13). `contains` is half-open, like
`CGRectContainsPoint`: true when `x >= self.x`, `x < self.x + self.width`,
`y >= self.y` and `y < self.y + self.height`. A point on the minimum x or
y edge is inside; a point on the maximum x or y edge is outside.

### `SpaceId` and `Pid`

```rust
pub type SpaceId = u64;
pub type Pid = i32;
```

`SpaceId` is a window server Space ID (R7, R10). `Pid` is a process ID
(R14, D3, D5).

## Constants

| Constant | Type | Value | Meaning |
| --- | --- | --- | --- |
| `SMART_CHECK_DELAY` | `Duration` | 250 ms | R8: wait between a click and its smart check |
| `FOCUS_HIDE_DELAY` | `Duration` | 100 ms | F4: wait between a focus change and the hide |
| `DOCK_BUNDLE_ID` | `&str` | `"com.apple.dock"` | R15: the Dock's bundle identifier |
| `NORMAL_WINDOW_LAYER` | `i32` | `0` | D5: the layer of normal app windows |
| `HIDE_ORDER` | `[Section; 3]` | `[Visible, Hidden, AlwaysHidden]` | R17: the order of "hide everything" |

## Settings (S1 to S4, D1, D2, T1)

### `RehideStrategy`

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RehideStrategy { Smart, Timed, FocusedApp }
```

S1. `RehideStrategy::default()` is `Smart`.

| Item | Signature | Meaning |
| --- | --- | --- |
| `ALL` | `pub const ALL: [RehideStrategy; 3]` | S4: picker order, `[Smart, Timed, FocusedApp]` |
| `from_raw` | `pub fn from_raw(raw: i64) -> Option<RehideStrategy>` | S2: `0` is `Smart`, `1` is `Timed`, `2` is `FocusedApp`; any other value is `None` |
| `raw` | `pub fn raw(self) -> i64` | S2, D8: the inverse of `from_raw` (`Smart` 0, `Timed` 1, `FocusedApp` 2) |
| `label` | `pub fn label(self) -> &'static str` | S4: `"Smart"`, `"Timed"`, `"Focused app"` |
| `description` | `pub fn description(self) -> &'static str` | S3: the line under the picker, the exact text S3 quotes for each strategy |

### `default_strategy`

```rust
pub fn default_strategy(auto_rehide_stored: bool, interval_stored: bool) -> RehideStrategy;
```

D1: the strategy when no `RehideStrategy` value is stored. The arguments
say whether the `AutoRehide` key and the `RehideInterval` key exist in the
defaults (whatever their values). Returns `Timed` when either exists,
otherwise `Smart`.

### `stored_strategy`

```rust
pub fn stored_strategy(
    stored: Option<i64>,
    auto_rehide_stored: bool,
    interval_stored: bool,
) -> RehideStrategy;
```

S2 and D1: the strategy the app uses. `stored` is the integer stored under
`RehideStrategy`, `None` when the key is absent.

- `None`: `default_strategy(auto_rehide_stored, interval_stored)`.
- `Some(raw)` with `from_raw(raw)` not `None`: that strategy.
- `Some(raw)` with any other value: `Smart`. The value is ignored and the
  default stays (S2); the D1 Timed default applies only while the key is
  absent.

### `RehideSettings`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RehideSettings {
    pub auto_rehide: bool,          // "Automatically rehide"
    pub strategy: RehideStrategy,   // "Strategy"
}
```

S1: the settings the decisions read. The interval is not part of it.

### Enabled tests

```rust
pub fn smart_enabled(settings: RehideSettings) -> bool;
pub fn focused_app_enabled(settings: RehideSettings) -> bool;
pub fn timed_checks_enabled(settings: RehideSettings) -> bool;
```

Each is true exactly when `settings.auto_rehide` is true and
`settings.strategy` is, respectively, `Smart` (R2), `FocusedApp` (F2 as
changed by D2) or `Timed` (T1).

### `RehideControls` and `rehide_controls`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RehideControls {
    pub strategy_picker: bool,  // the Strategy picker and its description line
    pub interval: bool,         // the interval slider
}
pub fn rehide_controls(settings: RehideSettings) -> RehideControls;
```

S3 and D9: which controls the Settings window shows besides the
"Automatically rehide" switch, which is always shown. `strategy_picker`
equals `settings.auto_rehide`. `interval` equals
`timed_checks_enabled(settings)`: the slider shows only while the picker
does and the strategy is `Timed`.

## Hide action (Terms, H1 to H4, R17)

### `DividerState`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DividerState { Show, Hide }
```

Terms: a section's divider state.

### `MenuBar`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuBar {
    pub visible: DividerState,       // the visible section's divider (the app's icon)
    pub hidden: DividerState,
    pub always_hidden: DividerState,
    pub ice_bar: Option<Section>,    // the section the Ice Bar panel displays; None while closed
    pub use_ice_bar: bool,           // H3: "the Ice Bar is in use"
}
```

The input and output of the hide action. `use_ice_bar` is the "Use Ice
Bar" setting as it applies on the system (H3's "while the Ice Bar is in
use"); it is independent of whether the panel is open.

| Method | Signature | Meaning |
| --- | --- | --- |
| `divider` | `pub fn divider(&self, section: Section) -> DividerState` | The field for `section` |
| `is_shown` | `pub fn is_shown(&self, section: Section) -> bool` | Terms, "Shown section": true when the section's divider is `Show`, or `ice_bar == Some(section)`, or `section` is `Visible` and `ice_bar == Some(Section::Hidden)` |
| `is_revealed` | `pub fn is_revealed(&self) -> bool` | Terms, "Something is revealed" (R5): `is_shown` is true for at least one section |

### `HideOutcome`

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HideOutcome {
    pub menu_bar: MenuBar,          // the state after the action
    pub close_ice_bar: bool,        // H2
    pub stop_timed: Vec<Section>,   // H4
}
impl HideOutcome {
    pub fn changed(&self) -> bool;  // !stop_timed.is_empty()
}
```

- `close_ice_bar`: H2, close the Ice Bar panel and allow show-on-hover
  again. True when at least one shown section was hidden.
- `stop_timed`: H4, the sections whose Timed checks stop, in the order they
  were hidden. Empty when nothing was shown (H1).
- `changed()`: false exactly for H1's no-op.

### `hide_section`

```rust
pub fn hide_section(menu_bar: &MenuBar, section: Section) -> HideOutcome;
```

The hide action on one section.

- H1: when `menu_bar.is_shown(section)` is false, the outcome is
  `menu_bar` unchanged, `close_ice_bar` false and `stop_timed` empty.
- Otherwise, with `next` starting as a copy of `menu_bar`:
  - H2: `next.ice_bar` becomes `None`, and `close_ice_bar` is true.
  - H3: when `section` is `AlwaysHidden` and `menu_bar.use_ice_bar` is
    false, only `next.always_hidden` becomes `Hide`; the other dividers
    keep their states. In every other case (`Visible`, `Hidden`, or any
    section with `use_ice_bar` true) all three dividers become `Hide`.
  - H4: `stop_timed` is `vec![section]`.
  - `use_ice_bar` is copied unchanged.

### `hide_sections`

```rust
pub fn hide_sections(menu_bar: &MenuBar, sections: &[Section]) -> HideOutcome;
```

Runs `hide_section` on each element of `sections` in order, each step on
the `menu_bar` the previous step returned. The result's `menu_bar` is the
last step's, `close_ice_bar` is true when any step's was, and `stop_timed`
concatenates the steps' lists in order. An empty slice returns `menu_bar`
unchanged with `close_ice_bar` false and `stop_timed` empty.

### `hide_everything`

```rust
pub fn hide_everything(menu_bar: &MenuBar) -> HideOutcome;
```

Terms "Hide everything" and R17: `hide_sections(menu_bar, &HIDE_ORDER)`,
that is visible, then hidden, then always-hidden.

## Smart strategy at click time (R1 to R8, D4, D6)

### `ClickTarget`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickTarget {
    Other,    // any other window, in ice-rust or another app
    AppIcon,  // R3: the window of ice-rust's own menu bar icon
    IceBar,   // R4: the Ice Bar panel
}
```

### `SmartClick`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmartClick {
    pub settings: RehideSettings,    // R2
    pub target: ClickTarget,         // R3, R4
    pub posted_by_app: bool,         // D6: ice-rust posted the click
    pub revealed: bool,              // R5: MenuBar::is_revealed
    pub pointer_in_menu_bar: bool,   // R6, D4
    pub space: SpaceId,              // R7: the active Space at the click
}
```

One left mouse-down (R1). Only left mouse-downs reach this type; the glue
never builds one for another button.

### `ClickDecision`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickDecision {
    Drop,
    Check { space: SpaceId, after: Duration },
}
```

`Drop`: the click is not a candidate. `Check`: R7 and R8, run
`smart_check` once `after` has passed, with `space` as the snapshot. The
caller keeps no handle, so checks are never cancelled and never replace
each other, and the check does not read the settings again (R9).

### `smart_click`

```rust
pub fn smart_click(click: &SmartClick) -> ClickDecision;
```

Returns `Drop` when any of these holds, and otherwise
`Check { space: click.space, after: SMART_CHECK_DELAY }`:

- `smart_enabled(click.settings)` is false (R2);
- `click.posted_by_app` is true (D6);
- `click.target` is not `ClickTarget::Other` (R3, R4);
- `click.revealed` is false (R5);
- `click.pointer_in_menu_bar` is true (R6, D4).

`Drop` carries no reason, so the order of these tests is not observable.

## Smart strategy after the wait (R9 to R17, D5)

### `Window`

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub layer: i32,    // kCGWindowLayer
    pub bounds: Rect,  // global coordinates, origin top left of the main display, y down
    pub owner: Pid,    // kCGWindowOwnerPID
}
```

An on-screen window (R12). There is no title field: D5 chose rule (b),
which does not use titles.

### `ActivationPolicy` and `RunningApp`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationPolicy { Regular, Accessory, Prohibited }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningApp {
    pub bundle_id: Option<String>,   // R15
    pub active: bool,                // R16: the app is active (frontmost)
    pub policy: ActivationPolicy,    // R16
}
```

What the check reads about the clicked window's owner (R14 to R16).
`Regular`, `Accessory` and `Prohibited` correspond to the raw
`NSApplicationActivationPolicy` values 0, 1 and 2.

### `SmartCheck`

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartCheck<'a> {
    pub snapshot_space: SpaceId,       // R7: from ClickDecision::Check
    pub current_space: SpaceId,        // R10: the active Space now
    pub pointer: Option<(f64, f64)>,   // R11: (x, y) now, same coordinates as Window::bounds; None if unreadable
    pub windows: &'a [Window],         // R12: on-screen windows, front to back
    pub cursor_level: i32,             // R12: CGWindowLevelForKey(kCGCursorWindowLevelKey)
    pub dock: Option<Pid>,             // D5: the Dock's process ID; None when it is not running
}
```

### `CheckDecision`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckDecision { HideEverything, NoChange }
```

`HideEverything`: run `hide_everything` (R10, R15, R16). `NoChange`: leave
the menu bar as it is.

### `clicked_window`

```rust
pub fn clicked_window(
    windows: &[Window],
    pointer: (f64, f64),
    cursor_level: i32,
    dock: Option<Pid>,
) -> Option<&Window>;
```

D5, rule (b): the window a click is judged by, or `None` for no change.
It walks `windows` in slice order (front to back) and skips every window
with `layer >= cursor_level` (R12) or whose `bounds.contains(pointer.0,
pointer.1)` is false. For the first remaining window:

- `layer == NORMAL_WINDOW_LAYER`, or `dock == Some(owner)`: returns
  `Some` of that window (any layer, for the Dock's windows).
- `layer > NORMAL_WINDOW_LAYER` and not the Dock's: returns `None` at once
  (a menu, a pop-up, a status item panel; the Q6 change in D5).
- `layer < NORMAL_WINDOW_LAYER` and not the Dock's: passed over; the walk
  goes on to the next window that passes the two skips.

When the walk ends without a match it returns `None`. Window titles play
no part.

### `smart_check`

```rust
pub fn smart_check(
    check: &SmartCheck<'_>,
    app: impl Fn(Pid) -> Option<RunningApp>,
) -> CheckDecision;
```

In this order:

1. R10: `current_space != snapshot_space` returns `HideEverything`, with no
   further checks; `app` is not called.
2. R11: `pointer` is `None` returns `NoChange`.
3. R12, R13 as changed by D5:
   `clicked_window(windows, pointer, cursor_level, dock)` is `None`
   returns `NoChange`.
4. R14: `app(window.owner)` is `None` returns `NoChange`.
5. R15: the app's `bundle_id` is `Some(DOCK_BUNDLE_ID)` returns
   `HideEverything`.
6. R16: `active` is true and `policy` is `ActivationPolicy::Regular`
   returns `HideEverything`; otherwise `NoChange`.

`app` is the lookup from process ID to running app. It is called at most
once, with the clicked window's owner, and only in steps 4 to 6.

## Focused app strategy (F1 to F4, D2, D3)

### `FocusChange`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusChange {
    pub settings: RehideSettings,      // F2, D2
    pub frontmost: Option<Pid>,        // F1: the new frontmost app; None when there is none
    pub own: Pid,                      // D3: ice-rust's own process ID
    pub last_other: Option<Pid>,       // D3: from the previous FocusOutcome; None before the first
    pub pointer_in_menu_bar: bool,     // F3, D4
}
```

One change of the frontmost app (F1), including the one delivery of the
current frontmost app when the observation starts.

### `FocusDecision` and `FocusOutcome`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDecision {
    Drop,
    Hide { section: Section, after: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusOutcome {
    pub decision: FocusDecision,
    pub last_other: Option<Pid>,   // D3: pass back as FocusChange::last_other next time
}
```

`Hide`: F4, once `after` has passed, run `hide_section` on `section`.

### `focus_change`

```rust
pub fn focus_change(change: &FocusChange) -> FocusOutcome;
```

`decision` is `Drop` when any of these holds, and otherwise
`Hide { section: Section::Hidden, after: FOCUS_HIDE_DELAY }`:

- `focused_app_enabled(change.settings)` is false (F2, D2);
- `change.frontmost == Some(change.own)`: ice-rust became frontmost (D3);
- `change.frontmost` is `Some(pid)` and `change.last_other == Some(pid)`:
  focus returns to the last app other than ice-rust (D3);
- `change.pointer_in_menu_bar` is true (F3, D4).

A `None` frontmost is neither ice-rust nor a return. `Drop` carries no
reason.

`last_other` is computed whatever the decision: `Some(pid)` when
`change.frontmost` is `Some(pid)` with `pid != change.own`, otherwise
`change.last_other` unchanged.

F4: the decision has no Space or window input; the hide itself is the
`hide_section` call that follows.
