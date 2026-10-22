# ice-rust design

ice-rust is a Rust port of Ice. This contract started with two Ice behaviors on macOS:
hiding menu bar items behind a divider, and the Ice Bar panel that shows the
hidden items under the menu bar. Mechanism facts come from the evidence ledger
([ice-internals-evidence.md](ice-internals-evidence.md)), which cites Ice
0.11.13-dev.2 source, the installed binary, and passive runtime probes on
macOS 26.3.

Ice is GPL-3.0, and so is ice-rust, a derivative of it: its features, UI
design and text, settings keys and mechanisms come from Ice. The rule used
while writing the code: work from facts about how Ice and macOS behave (API
names, constants, window-server behavior, the UI and its text) and write the
Rust code from those facts; do not translate Ice's Swift line by line or copy
its code comments.

## Scope (v1)

In scope:

1. Two status items: the icon (always visible) and a hidden-section divider.
2. Left-click on the icon toggles the hidden section. Right-click opens a menu.
3. Ice Bar mode (default on): toggling shows a panel under the menu bar with
   images of the hidden items instead of revealing them in the menu bar.
4. Clicking an image in the Ice Bar reveals the section and clicks the real
   item.
5. Auto-rehide after an idle interval (default 15 s).
6. Permission checks for Screen Recording and event posting (Accessibility).
7. `ice-rust probe`, a read-only CLI that enumerates and captures hidden items.
   It is the main verification harness.

Out of scope for v1 (Ice has them): always-hidden section, global hotkeys,
show-on-hover / show-on-click in empty space / show-on-scroll, moving items by
synthesized Command-drag, menu bar appearance, search panel, layout editor,
XPC source-PID resolution, Hide Application Menus.

## Observed mechanisms this design relies on

| # | Fact (observed) | Evidence |
|---|---|---|
| M1 | Hiding = a divider NSStatusItem set to length 10000. AppKit lays items right to left, so items left of an expanded divider move to large negative x, off every display. Showing = length -1 (NSVariableStatusItemLength). | Ice/MenuBar/ControlItem/ControlItem.swift:34-56; binary: one `setLength:` sender at 0x10004f9e0 builds 0x40C3880000000000 (10000.0) |
| M2 | On macOS 26.3 the system clamps the expanded divider to 5016 pt. Items still land off-screen. | runtime: Ice.ControlItem.Hidden wid 188 is 5016 wide on all 4 displays |
| M3 | Item order persists through AppKit defaults keys `NSStatusItem Preferred Position <autosaveName>` (distance from the menu bar's right end). Seeding 0 for the icon and 1 for the divider puts the divider directly left of the icon. On macOS 26 Ice also sets `NSStatusItem VisibleCC <autosaveName>` = true. AppKit deletes the position key on `setVisible(false)` and `removeStatusItem`. | ControlItem.swift:646-667, 680-681; runtime defaults values 415/496/5649 |
| M4 | On macOS 26 every status item window, including your own, is owned by Control Center at layer 25 (kCGStatusWindowLevel). The owner PID does not identify the app. | runtime R1; MenuBarItem.swift:62-65 |
| M5 | Each item has one window per display. The copy on the active menu bar display is titled with the item's autosave name; copies on other displays are titled with the bundle id or "". Titles of other processes' windows need Screen Recording. | runtime R2, R14 |
| M6 | Adjacent items abut exactly: an item's maxX equals its right neighbor's minX. Section membership can be computed from geometry relative to the divider. | runtime R4, R7 |
| M7 | `CGWindowListCreateImageFromArray(CGRectNull, ids, 9)` (boundsIgnoreFraming 1 + bestResolution 8) captures item windows by id, including off-screen ones, from a CFArray of raw window ids with NULL callbacks. On 26.3 some off-screen copies come back fully transparent; Ice drops transparent crops and retries per window. | binary: single call site 0x10011ce70, option 9 at 0x100077520; MenuBarItemImageCache.swift:53; runtime probe 42/83 non-transparent |
| M8 | Ice Bar panel: NSPanel, style mask 0x8080 (nonactivatingPanel 1<<7, fullSizeContentView 1<<15), level 25, collectionBehavior 322, no shadow, no animation, shown with `orderFrontRegardless`, content view returns YES from `acceptsFirstMouse:`. y = (screen.maxY - 1) - menuBarHeight - panelHeight. | IceBar.swift:182-257, 281-313 |
| M9 | Synthetic click on an item: kCGEventLeftMouseDown (1) / LeftMouseUp (2) at the item center from an HID-system-state source (1); integer fields 91 and 92 = item window id; field 1 (click state) = 1 on down, 0 on up; up is sent twice. Cursor hidden during the operation and warped back. | binary click builder 0x10008b9a8; MenuBarItemManager.swift:1106-1234, 1566-1615 |
| M10 | Posting events into other apps needs the Accessibility grant; window titles and images need Screen Recording. | Permission.swift:121-139; Q3-C20 (inferred), Q4-C27 |
| M11 | Menu bar height on this host is 30 pt (external) and 33 pt (notched built-in); `NSStatusBar.thickness` reports 22 and is wrong for layout. Use the height of the status/menu bar windows. | runtime R6 |

## Design choices (not Ice behavior)

| Choice | Why |
|---|---|
| Discover items with the public `CGWindowListCopyWindowInfo(kCGWindowListOptionAll)` filtered to layer 25, then walk the abutting chain from our divider. | Avoids Ice's private `CGSGetProcessMenuBarWindowList`, active-space and level SPI. Fact M6 makes geometry enough. The chain starts at our own divider's active-display copy. Replicas on a display that shares the same top y can still touch the chain's edges, so the leftward walk from an expanded divider skips on-screen windows (see Chain walk). |
| Collapsed divider shows a small chevron at variable length. | Ice's zero-width style deactivates a private AppKit constraint; a visible chevron avoids AppKit internals. |
| Ice Bar click-through reveals the whole hidden section, then clicks the real item once it is on screen. | Ice instead moves the single item with Command-drag events (undocumented field 51 and a private event-tap relay). Revealing is simpler and uses only M1 and M9. Cost: all hidden items appear briefly. |
| Rehide while revealed: hide when the pointer has been outside the menu bar and no pop-up menu window (layer 101) has been on screen for `rehide_interval` (default 15 s). | Approximates Ice's timed rehide and temp-show window tracking without source-PID resolution. |
| Stop the chain walk at any window wider than 1000 pt. | An expanded divider of another menu bar manager (for example Ice's own) must not count as an item. |
| `use_ice_bar` defaults to true. | The maintainer asked for the Ice Bar as the default. Ice defaults it to false. |

## Coordinates

Two global spaces exist and must not mix:

- CG space: origin top-left of the primary display, y grows down. Used by
  `CGWindowList*` bounds and CGEvent locations.
- AppKit space: origin bottom-left of the primary display, y grows up. Used by
  NSScreen and NSWindow frames.

Conversion: `appkit_y = primary_height - cg_y - height`, where `primary_height`
is the height of the screen whose AppKit frame origin is (0, 0). x is the same
in both spaces.

## Module layout

```
src/
  main.rs            CLI dispatch: no args = menu bar app; `probe` = read-only harness
  lib.rs             pub mods; platform mods behind cfg(target_os = "macos")
  model.rs           PURE. WindowRecord, Rect, Section, constants (layers, lengths, autosave names)
  geometry.rs        PURE. CG<->AppKit conversion, Ice Bar frame math, clamping
  chain.rs           PURE. find divider, walk hidden (left) and visible (right) chains
  bar_layout.rs      PURE. item image placement in the panel, overflow handling
  rehide.rs          PURE. rehide decision state machine with injected time
  image_check.rs     PURE. transparency test on raw pixel buffers
  settings.rs        PURE defaults + NSUserDefaults load/save (load/save behind cfg)
  platform/
    mod.rs
    windows.rs       CGWindowListCopyWindowInfo -> Vec<WindowRecord>; active display; menu bar height
    capture.rs       CGWindowListCreateImageFromArray FFI; CGImage -> PNG; transparency check
    events.rs        synthetic click (M9) with cursor hide/restore guard
    permissions.rs   CGPreflight/RequestScreenCaptureAccess, CGPreflight/RequestPostEventAccess, AXIsProcessTrustedWithOptions
    status.rs        status items, defaults seeding (M3), length toggling (M1)
    ice_bar.rs       NSPanel (M8) + item view class (acceptsFirstMouse, click detection)
    app.rs           controller: wiring, timers, monitors, notifications
scripts/
  bundle.sh          builds IceRust.app (LSUIElement, stable bundle id, release certificate or ad-hoc signature)
```

Pure modules have no macOS dependencies and carry unit tests (`cargo test`).

## Constants

| Name | Value | Source |
|---|---|---|
| `ICON_AUTOSAVE` | `IceRust.ControlItem.Visible` | ours |
| `DIVIDER_AUTOSAVE` | `IceRust.ControlItem.Hidden` | ours |
| `HIDE_LENGTH` | 10000.0 | M1 |
| `VARIABLE_LENGTH` | -1.0 | M1 |
| `STATUS_LAYER` | 25 | M4 |
| `MAIN_MENU_LAYER` | 24 | runtime R1 |
| `POPUP_MENU_LAYER` | 101 | kCGPopUpMenuWindowLevel |
| `ABUT_TOLERANCE` | 1.0 pt | M6 |
| `DIVIDER_MIN_WIDTH` | 1000.0 pt | ours |
| `CAPTURE_OPTION` | 9 | M7 |
| `PANEL_STYLE_MASK` | 0x8080 | M8 |
| `PANEL_LEVEL` | 25 | M8 |
| `PANEL_COLLECTION_BEHAVIOR` | 322 | M8 |
| `FIELD_CLICK_STATE` | 1 | M9 |
| `FIELD_WINDOW_UNDER_POINTER` | 91 | M9 |
| `FIELD_WINDOW_THAT_CAN_HANDLE` | 92 | M9 |
| `REHIDE_INTERVAL` default | 15 s | Ice default RehideInterval |
| `CLICK_SETTLE_TIMEOUT` | 1 s, polled every 20 ms | ours |

## Algorithms

### Chain walk (chain.rs)

Input: all layer-25 `WindowRecord`s, divider window id.

1. `d` = divider bounds. Candidates: layer 25, id != divider, width > 0,
   `|y - d.y| <= ABUT_TOLERANCE`, `|height - d.height| <= ABUT_TOLERANCE`.
   When walking left from an expanded divider (width >= `DIVIDER_MIN_WIDTH`),
   on-screen windows are excluded: everything left of an expanded divider is
   off screen, and on this host on-screen replicas from a display at the same
   y (display 3 at x -1920) can share an edge with the off-screen chain. The
   rightward walk keeps on-screen and off-screen windows, because items right
   of another manager's expanded divider are off screen by design.
2. Hidden chain: `cursor = d.min_x`. Repeat: collect the unvisited
   candidates with `|max_x - cursor| <= ABUT_TOLERANCE`. Stop if there are
   none, if one of them is the stop window (v1.1), or if one of them is at
   least `DIVIDER_MIN_WIDTH` wide. Otherwise pick the closest, then the
   lowest id. Push, `cursor = min_x`. Cap at 256 steps. Return left-to-right
   order.
3. Visible chain: the same walk rightward from `d.max_x` using `min_x`.
4. Ties (v1.1 review): where the section is shown (right of the hidden
   divider, left of a collapsed divider) on-screen windows win over
   off-screen ones before the stop checks look at the rest. Stopping on
   any touching divider-wide window, rather than only the picked one, can
   end a walk early; continuing through another display's copies adds
   items that belong to no section, so the walk stops.

Divider lookup: the layer-25 window titled `DIVIDER_AUTOSAVE`. If several
match, prefer the one whose y and height match the icon window titled
`ICON_AUTOSAVE`. If titles are unreadable, report missing Screen Recording.

### Order guard

At launch, if the stored positions put the icon left of the divider
(icon position >= divider position), both keys are removed and reseeded to
0 and 1. Otherwise an expanded divider would hide the icon with the section.
v1.1 checks three positions as they will be after seeding (see the v1.1
implementation notes).

### Without Screen Recording

Titles are unreadable, so the divider and icon windows cannot be found by
title. The icon rectangle falls back to the AppKit frame of the icon button's
window, converted to CG space. The Ice Bar then opens with a single
"Screen Recording permission needed" tile, and opens with "No hidden items"
when the chain is empty.

### Toggle

- Ice Bar mode off: revealed = divider length -1; hidden = divider length 10000.
- Ice Bar mode on: the divider stays at 10000. Toggle opens or closes the panel.

### Ice Bar

1. Enumerate windows, walk the hidden chain from our divider.
2. Capture each hidden item: composite capture of all ids with option 9,
   crop by each item's bounds relative to the union (scale = image width /
   union width). Any crop that is fully transparent is captured alone. Still
   transparent: draw a placeholder tile with the window title.
3. Panel height = menu bar height of the active display. Items laid out
   left-to-right with 4 pt padding. Panel x centered under the icon window,
   clamped to the screen. Panel y from M8.
4. Close on: click outside the panel (global left/right mouse-down monitor),
   active space change, screen parameter change, toggle.

### Click-through

1. Close the panel. Set divider length -1 (reveal). Every reveal, manual or
   from the Ice Bar, is handed to the rehide state machine; there is no
   separate temporary-show state.
2. Poll the window list every 20 ms (up to 1 s) until the item's window id is
   on screen with x >= the active display's min x.
3. Post the click (M9) at the item center: hide cursor, post down once and up
   twice to kCGSessionEventTap, warp the cursor back, show cursor.
4. The rehide state machine takes over.

### Rehide (rehide.rs)

State: `revealed: bool`, `last_active: Instant`. Each 0.5 s tick passes
`pointer_in_menu_bar`, `popup_menu_visible`, `panel_visible`, `now`. Any of
these true sets `last_active = now`. Decision: `Rehide` when revealed and
`now - last_active >= interval`; otherwise `Keep`. Manual toggle resets.

## Permissions

- Screen Recording: `CGPreflightScreenCaptureAccess`, request with
  `CGRequestScreenCaptureAccess`. Needed for titles (divider lookup) and images.
- Event posting: `CGPreflightPostEventAccess` / `CGRequestPostEventAccess`, and
  `AXIsProcessTrustedWithOptions` for the Accessibility prompt.
- TCC grants attach to the responsible process. Running the bare binary from a
  terminal uses the terminal's grants; `scripts/bundle.sh` builds a stable app
  bundle for standalone use.

## Verification

1. `cargo test` covers every pure module.
2. `cargo clippy --all-targets -- -D warnings` is clean.
3. `cargo run -- probe --divider Ice.ControlItem.Hidden --out <dir>` against
   the running Ice reproduces Ice's hidden section membership (cross-check
   with an independent window listing) and writes non-empty PNGs.
4. Live GUI run only with the maintainer's approval, because it changes the real
   menu bar while it runs.

## GUI test findings (2026-10-13, macOS 26.3, Ice quit)

Observed during the first live run of the menu bar app:

| # | Result | Evidence |
|---|---|---|
| G1 | Launch, divider expand to 5016 pt, Ice Bar open (974 x 30 pt at y 31), click-through to AirDrop, Quit, and Preferred Position persistence all work. | local screenshots 01 to 13 and driver state dumps (not included) |
| G2 | Timed rehide fires 16.0 s after a reveal with the pointer below the menu bar (15 s interval + 0.5 s tick). | timed poll |
| G3 | Menus that Control Center opens for the items it hosts (AirDrop) are layer **100**, not 101. Rehide closed an open AirDrop menu. Fixed: `model::is_menu_layer` covers 100 and 101. | on-screen window list while the menu was open |
| G4 | After a synthetic click the cursor stays on the item. `CGEvent::post` only queues; the warp back runs before the window server delivers the queued events. Blocking the main thread in a wait loop did not help, and in that run the AirDrop menu did not open. | pointer at (-525, 15) after click-through in two runs |
| G5 | `CGWindowListCreateImageFromArray` is proxied through ScreenCaptureKit/replayd on macOS 26. A second process running the same executable (probe while the GUI runs) blocks forever in `SLSWindowListCreateImageFromArrayProxying`. A copy of the binary at another path captures fine. Ice issue #777 reports the same stack freezing Ice's main thread on 26.1. | `sample` of the hung probe; copy-path test |
| G6 | With every display asleep, captures of on-screen windows (the visible section, the menu bar color sample) take about 1.0 s per call and return a fully transparent image; `SCShareableContent` lists no displays. Off-screen windows still capture in about 0.04 s with real pixels. This, not a code defect, made the visible section time out in the v1.1 final check. Open: confirm with the displays awake, and skip on-screen captures at once while `CGDisplayIsAsleep` is true. | local Swift capture probes (not included), 2026-10-13 |

## v1.1 scope

Decided with the maintainer on 2026-10-13: three sections like Ice, and a full
Ice-style settings window with a sidebar. Reference: a screenshot of Ice's
"Menu Bar Layout" pane supplied by the maintainer.

1. Fix G4: non-blocking synthetic input. Post events, then restore the cursor
   only after a listen-only session event tap (tail append) has seen the last
   tagged event (field 42 = unique tag), or after a 300 ms fallback timer.
   Nothing blocks the main thread.
2. Fix G5: run every window capture on a worker thread and wait at most
   `CAPTURE_TIMEOUT` (1 s). On timeout, fall back to placeholder tiles (GUI)
   or report "capture timed out" (probe). A timed-out worker thread is left
   to finish on its own; it must not hold locks the main thread needs.
3. Always-Hidden section (a third control item).
4. Settings window with a sidebar, including the Menu Bar Layout pane with
   drag and drop.

### Always-Hidden section

Mechanism facts come from the ledger (M1, M3 and section 2): Ice's
always-hidden divider sits left of the hidden divider, expands to the same
10000 length, and `show(alwaysHidden)` shows every section.

- Third status item `IceRust.ControlItem.AlwaysHidden`, seeded Preferred
  Position 2 (left of the hidden divider at 1), plus `Visible` and, on
  macOS 26, `VisibleCC` keys like the other two. The order guard checks
  icon < hidden < always-hidden positions and reseeds all three when the
  stored order is wrong.
- Collapsed look: a double chevron (`chevron.left.2`), distinct from the
  hidden divider's single chevron.
- States: hidden section shown = hidden divider collapsed, always-hidden
  divider expanded. Always-hidden shown = both collapsed. Hide = both
  expanded. Hiding only the always-hidden section re-expands only its divider.
- Chains: hidden section = walk left from the hidden divider, stopping at the
  always-hidden divider (by window id, whatever its width). Always-hidden
  section = walk left from the always-hidden divider (existing stop rules).
- Triggers: left click on the icon toggles the hidden section; Option-click
  toggles the always-hidden section (which shows everything). In Ice Bar
  mode, left click opens the bar with the hidden items and Option-click with
  the always-hidden items.
- Rehide hides both sections.

### Settings window

- Menu items: "Settings..." (Command-comma) and "Arrange Menu Bar Items..."
  (opens Settings on the Menu Bar Layout page). One window instance; reopening
  brings it front. While it is open the app uses the regular activation
  policy and activates; it returns to accessory when the window closes.
- Look (from the reference): full-size content view with a transparent
  title bar; a sidebar (vibrant `NSVisualEffectView`, sidebar material) with
  a large app title "ice-rust" and rows with an SF Symbol icon and a label;
  the selected row is highlighted; the content area on the right has the page
  title in the title bar area and the page below.
- Pages (only pages backed by real behavior):
  - General: "Use Ice Bar" switch, "Automatically rehide" switch, "Rehide
    after" interval (5 to 60 s). Changes save to defaults immediately and
    apply live.
  - Menu Bar Layout: see below.
  - Permissions: Screen Recording and Accessibility status with a button that
    requests the permission or opens the matching System Settings pane;
    refresh every second while the page is visible.
  - About: version, one line on what it does, and the note that it is a Rust
    port of Ice, not affiliated with or endorsed by Ice.
- No Hotkeys, Appearance, or Updates pages; those features do not exist.

### Menu Bar Layout pane

- Heading "Drag to arrange your menu bar items".
- Tip box (rounded, subtle border, `lightbulb` symbol): "Tip: you can also
  arrange menu bar items by Command + dragging them in the menu bar".
- Three labeled bars, top to bottom: "Visible Section", "Hidden Section",
  "Always-Hidden Section". Each bar is a rounded rectangle at menu bar height
  plus padding, filled with the logo's blue (#1C43AB) behind light item
  glyphs or its light tint (#E4E8F5) behind dark ones (issue #3, 0.2.3). The
  glyph tone comes from the captured item images, else from the average color
  of the active display's menu bar (composite of the layer-24 "Menubar" window
  over the wallpaper, sampled on the worker-thread capture path), else from
  the window's appearance. Item tiles are the
  captured item images, right-aligned in menu bar order like the real menu
  bar, with placeholders where capture fails.
- Our own control items do not appear as tiles; the bars themselves stand for
  the dividers. Locked tiles (not draggable): our icon if shown, `Clock`,
  `BentoBox-0` (Control Center). Locked tiles still act as drop neighbors.
- Dragging: mouse down on a tile, the tile follows the pointer, an insertion
  marker shows between tiles in the bar under the pointer; mouse up drops.
  A drop at the original position does nothing.
- Drop target rule (pure, `arrange.rs`): with the bar as it would look after
  the drop, the destination is `LeftOf(next item)` if a next item exists,
  else `RightOf(previous item)`. Into an empty bar: Visible -> `RightOf(icon)`,
  Hidden -> `LeftOf(hidden divider)`, Always-Hidden ->
  `LeftOf(always-hidden divider)`. A destination may never cross a divider:
  Visible stays right of the hidden divider, Hidden stays between the two
  dividers, Always-Hidden stays left of the always-hidden divider.
- After a drop the pane shows a busy state, calls the move engine, then
  re-reads the windows and rebuilds all three bars from the real menu bar
  (the source of truth), and shows an inline error if the order does not
  match. The pane also refreshes when it becomes visible and every 3 s while
  visible and idle.

### Move engine (`platform/mover.rs`)

Independent design (Ice uses a private relay and field 51 to move off-screen
items; this engine moves only on-screen items):

1. Show every section the move touches (both dividers collapsed if either the
   item or the target is in the always-hidden section), and wait (timer
   polling, 20 ms, 1 s cap) until both are on screen on the active display.
2. Wait until the user is idle: no mouse button down and no input in the last
   50 ms (`CGEventSourceButtonState`, `CGEventSourceSecondsSinceLastEventType`),
   bounded at 5 s, after which the move fails with "user busy".
3. Hide the cursor, then post at the HID tap from an HID-state source:
   mouse moved to the item center; `LeftMouseDown` with `kCGEventFlagMaskCommand`
   at the item center; `LeftMouseDragged` (Command) in about 8 steps to the drop
   point; `LeftMouseUp` (Command) at the drop point. All events carry a unique
   tag in field 42. Every posted down has a matching up on every path.
4. Drop point: for `LeftOf(target)` x = target.min_x + 2 when moving left, and
   target.min_x - 2 when moving right; for `RightOf(target)` x = target.max_x - 2
   when moving right, and target.max_x + 2 when moving left; y = target mid y.
   These offsets are a starting point to tune live.
5. Restore the cursor per fix 1. Then verify by polling the window list
   (20 ms, 500 ms cap) that the item now abuts the target on the requested
   side. Retry up to 3 times with the drop x nudged by 4 pt further in the
   travel direction. Report success or failure to the caller.
6. One move at a time; further requests queue. Afterwards restore the
   sections' previous shown/hidden state.

### v1.1 verification

- Pure tests: drop target rule (every bar edge case, divider crossing),
  drop-point geometry, verify predicate, tag matching, three-section state
  transitions, chains with two dividers (collapsed and expanded), order guard
  with three positions.
- Read-only probe against Ice's real dividers still works and finishes fast.
- Live tests only with the maintainer's approval, because moves change the real
  menu bar order persistently. Proposed live test: open Settings, look at the
  three bars, move one item right of its neighbor, verify, move it back.

## v1.1 implementation notes

Deviations from the v1.1 sections above, and choices the spec leaves open.

| Area | As built | Why |
|---|---|---|
| G4 click | The click posts two separate mouse-up events instead of one event posted twice. | Each event carries its own field-42 tag, so the batch's last tag marks the second up. |
| G4 restore | The restore runs on the next main run loop pass after the tap sees the last tag (`CURSOR_RESTORE_SETTLE`, 0 ms), not inside the tap callback. For paced batches the 300 ms fallback starts when the last event is posted. A batch requested while another is in flight is refused as busy: the move engine waits for idle again and click-through keeps polling. | The tap is torn down outside its own callback; every posted down still gets its up, and one input owner at a time keeps a click out of a drag. |
| G4 timers | The pacer, the fallback, the settle timer and a whole-batch deadline (step x events + 300 ms, which posts whatever is still queued) fire in the common run loop modes. A pacer tick more than 100 ms after the previous post posts the rest at once. Quitting (the Quit action and `NSApplicationWillTerminateNotification`) posts the rest of a batch in flight and restores the cursor. The move engine's 20 ms ticker stays in the default mode. | A status menu, the main menu or a slider drag runs a tracking mode, which stops default-mode timers; a posted mouse-down must not wait for it. No new drag should start while a menu is open. |
| G5 gate | A timed-out capture keeps one shared atomic gate closed until its thread ends. Captures requested meanwhile return at once as timed out (placeholders, or "capture timed out" in the probe). The worker starts no further single-item captures after the caller's 1 s deadline and marks those items timed out. | Stuck threads do not pile up; the main thread never waits on the gate. A review harness measured about 2 s for one single capture of an on-screen item on the development host, so a worker running every fallback held the gate for several seconds. |
| Layout captures | The pane captures each section separately (three bounded captures): hidden, always-hidden, visible, then the bar color. An image that timed out keeps the previous snapshot's image for the same window id, and the color likewise. After a refresh with a timeout the next periodic refresh is skipped. | One composite across all sections would span about 10000 pt. In `render-settings` on the development host the off-screen sections captured within about 0.7 s, while the on-screen visible items and the color sample timed out, and each timeout closes the gate for the captures after it. |
| Bar color | `CGWindowListCreateImageFromArray` with the Window Server `Menubar` window and the Dock `Wallpaper*` window of the active display, bounds = that display's menu bar strip, option 16 (nominal resolution); average of pixels with alpha of at least 0.5 after drawing into at most 64 x 4 pixels, and no color when fewer than half the pixels qualify. Without the wallpaper window (none on macOS 27; not found on the issue #3 reporter's Mac), the `Menubar` window and every window below it (`CGWindowListCreateImage`, on screen below the window, including it): the `Menubar` window alone is clear but for its text, and its average was that text's white. Since 0.2.3 the sample only decides the bar fill's tone when the item images do not. | The spec names the composite but not the call details. |
| Move pacing | The drag events are posted 10 ms apart by a timer (`MOVE_EVENT_INTERVAL`). | The spec gives no pacing; a burst may coalesce. |
| Move attempts | "Retry up to 3 times" means 4 attempts in total. The engine checks placement first and reports success without posting when the item already abuts its target. Verification starts when the cursor is back, or after 2 s without that report. | Avoids a needless drag; bounds a lost callback. |
| Drop rule | A drop right of the trailing Clock / Control Center tiles in the visible bar snaps to left of them. The insertion marker uses the same clamp (`arrange::clamp_insertion`). | Those items cannot move, so a destination right of them cannot be reached. |
| Icon tile | ice-rust's icon is a locked tile in the Visible bar, so the "empty Visible bar" rule (`RightOf(icon)`) applies only when the icon window is not found; without an icon it falls back to `RightOf(hidden divider)`. | "Our icon if shown" read as: shown as a locked tile. |
| After a move | The pane waits 300 ms before re-reading the menu bar. | The engine returns the sections to their previous state right before reporting. |
| Runtime order | Every state change goes through one check (hide, reveal, Option-click, rehide, the move engine's show and restore): a state that would expand a divider sitting right of the icon is refused, the sections stay fully shown, and a message asks to Command-drag the icon. An always-hidden divider right of the hidden divider is reported on stderr and left in place. The items are created collapsed; the first hide runs 500 ms after launch through the same check. | Ice moves the divider back with synthesized events; this build does not move control items on its own. Control Center owns the item windows (M4), so the delay lets it place them before the live check reads them. |
| Launch reseed | The launch check fills each missing position key with its seed (0, 1, 2) and removes all three keys when that order is wrong. | Checking only stored values missed a v1 upgrade: grown icon and divider values with no always-hidden key would seed the new divider at 2, right of the icon. |
| Moves and user input | While a move runs, a click on the icon or a rehide changes the state the move returns to instead of the dividers. The rehide idle count restarts when the move ends. Click-through waits until no move is queued and no batch is posting, then reads the item's bounds and clicks in the same pass. | The engine needs the sections it showed; the drag counts as activity. |
| Settings window | Fixed size, 820 x 600 pt. The Permissions button requests the permission on its first click and opens the System Settings pane on later clicks; the page says Screen Recording applies after a relaunch. A main menu (Settings..., Quit, Window > Close) is installed and shows while the app is regular, so Command-comma and Command-W work. The status menu also has Show/Hide Always-Hidden Items. The selected sidebar row turns gray while the window is not key. Closing the window activates the app that was in front when it opened. | Fixed frames keep the offscreen render and the live window identical. System Settings offers Quit & Reopen after a Screen Recording grant (not verified with a live grant here). |
| Layout bars | A section wider than its bar scrolls sideways: scroll wheel or trackpad over the bar, a drag near either inner edge, or a click on the "+N" hint at the cut-off edge. Placeholder tiles draw a neutral SF Symbol (`questionmark.square.dashed`); every tile has a tooltip with its name, and locked tiles use plain names (Control Center, the ice-rust icon). Without a color sample the bars use light or dark gray by the window's appearance. A message from a press or drop stays until the next press; refreshes change only a separate note. | A fixed window cannot show every item otherwise, and dropped tiles could not be dragged or used as neighbors. Bundle-id titles clipped to 26 pt did not name the item. |
| Rehide interval | The Settings slider offers 5 to 60 s; a stored value outside that range (0 to 3600 is still accepted from defaults) shows clamped until the slider moves. Only a slider change writes the interval. | The v1 defaults range is kept. |
| Probe | Also prints the always-hidden divider (the `--divider` title with `.Hidden` replaced by `.AlwaysHidden`) and its chain; the hidden walk stops at that divider by id. JSON gains `always_hidden_title`, `always_hidden_divider`, `always_hidden` and `capture_error`. `capture_error` is "capture timed out" for a timeout, a busy gate or items left at the worker's deadline, and "capture worker failed" when the worker could not start or panicked. | Read-only check of the two-divider walk against Ice. |
| Chain ties | See Chain walk step 4. | Read-only probe, display 3 active: copy 6957 ended at -5532 like Ice's always-hidden divider and copy 6964 started at -412 like Ice's icon, so the walks picked 12 copies into the hidden chain and three into the visible chain. |
| render-settings | Renders the window's frame view (title bar included) at 1x. Without `--divider-prefix` it tries `IceRust.ControlItem.` and then `Ice.ControlItem.`; with a prefix it uses only that one. `--width` and `--height` accept 200 to 4000. | An offscreen window has no screen backing scale. |

## Live test v1.1 (2026-10-13, macOS 26.3, Ice quit, maintainer approved)

| # | Result | Evidence |
|---|---|---|
| P1 | Pass: three status items created; upgrade from v1 seeded the always-hidden position 2; positions survive a restart. | `defaults read ice-rust`, driver state |
| P2 | Pass: Ice Bar for the hidden section ("No hidden items") and, with Option-click, for the always-hidden section (24 real icons). | local screenshots (not included) |
| P3 | Pass: menu (Show Hidden / Show Always-Hidden / Use Ice Bar / Settings / Arrange / Quit); "Arrange Menu Bar Items..." opens Settings on the layout page with live images in all three bars. | local screenshots (not included) |
| P4 | Pass: drag AirDrop from the Always-Hidden bar to the empty Hidden bar moved the real item (probe: hidden = [52]); dragging it back restored the exact original order (..., 312, 52, 68, ...). Sections returned to their prior state each time. | probe JSON before and after, local screenshots (not included) |
| P5 | Pass (G4): after a move and after an Ice Bar click-through the cursor returned to its exact start point (-785, 607); click-through opened the real AirDrop menu. | driver state, local screenshot (not included) |
| P6 | Pass (G6 confirmed): with the displays awake, on-screen captures read real pixels in about 0.06 s and the layout render takes 0.33 s with all 28 tiles. | local Swift capture probe and layout render (not included) |
| L1 | Open: seeding the always-hidden divider at position 2 places it directly left of the hidden divider, so every existing item lands in Always-Hidden and the Hidden section starts empty. Seed it at the far left instead (or leave it unseeded so AppKit places it leftmost). | probe after first launch |
| L2 | Open: the Settings window opens behind the frontmost app; it needed an explicit System Events activation in the test. | `lsappinfo front` still named the previously frontmost app after opening |
| L3 | Open: a bar with more items than fit shows "+N" and those items cannot be dragged (24 always-hidden items, "+10"). Needs horizontal scrolling or a resizable window. | local screenshots (not included) |
| L4 | **Fixed, verified live (critical):** the Command-drag left the Command modifier down system wide (`CGEventSourceFlagsState` 0x20100000) because the mouse-up carried `maskCommand`; Ice's mouse-up uses flags 0. Fix: mouse-up without flags plus a Command key-up after every Command batch (`events::release_command_key`). After the fix a move leaves flags at 0x20000000. | local `CGEventSourceFlagsState` probe before and after (not included) |
| L5 | Open, minor: a synthetic Escape did not close the status item menu opened through `performClick`; a click elsewhere did. A real keyboard Escape is untested. | menu stayed at layer 101 |
| L6 | Open: footprint with Settings open is 123 MB (50 MB graphics surfaces, 12 MB IOSurface, 11 MB CG image), versus 32 MB for v1 in menu-bar-only mode. Measure again after Settings closes and the app returns to the accessory policy. | `footprint` at 2 min uptime |
| T1 | Test-harness incidents, not app defects: a stray driver click at (0, 0) opened the Apple menu (closed with Escape, nothing selected); the driver's Option-click left Option down (released; the driver now posts an Option key-up). | the test run |

## v1.2 scope (fixes for L1, L2, L3; maintainer request 2026-10-13)

### L1: always-hidden placement

- Do not seed `NSStatusItem Preferred Position IceRust.ControlItem.AlwaysHidden`.
  Ice seeds only the visible (0) and hidden (1) items (ledger M3); a status
  item with no stored position is placed by AppKit, expected at the far left,
  so existing items stay in the Hidden section and Always-Hidden starts empty.
  Keep seeding the `Visible` and (macOS 26) `VisibleCC` keys for all three.
- One-time migration for v1.1 installs: if the defaults flag
  `AlwaysHiddenPlacementV2` is absent, remove the always-hidden Preferred
  Position key before creating the items, then set the flag. AppKit rewrites
  positions as distances when the app quits (observed: Visible 259, Hidden
  373, AlwaysHidden 537), so the old seed value cannot be detected by value.
- The order guard accepts a missing always-hidden position.
- After the items exist, check live geometry: log a warning if the
  always-hidden divider is not left of every other status item on its
  display. Do not try to move it.
- Live check (needs a live run): after launch the Hidden section holds the
  existing items and Always-Hidden is empty.

### L2: Settings window in front

- When Settings opens: set the regular activation policy, then on the next run
  loop turn (one-shot timer or main-queue dispatch) activate the app and
  `makeKeyAndOrderFront` + `orderFrontRegardless` the window. Use
  `NSApplication.activate()` on macOS 14+ and fall back to
  `activateIgnoringOtherApps(true)` (deprecated, allowed with a scoped
  `#[allow(deprecated)]` and a comment) when the app is still not active
  after the first attempt (check `isActive` on the following turn, retry once).
- Live check (needs a live run): the window is frontmost (`lsappinfo front`)
  after "Arrange Menu Bar Items..." and after "Settings...".

### L3: bars that do not fit

- Each layout bar becomes a horizontally scrollable strip (`NSScrollView`,
  horizontal only, no vertical scrolling, overlay scroller, transparent
  background so the bar color shows). Content is right-aligned when it fits;
  when it does not fit, the initial scroll position shows the right end, like
  the menu bar. Remove the "+N" pill from the layout pane (the Ice Bar keeps
  its own overflow handling).
- Drag and drop works across scrolled bars: convert pointer locations into
  each bar's document coordinates; the insertion index and marker use
  document coordinates; auto-scroll a bar while the dragged tile is within
  24 pt of its left or right edge (bounded step per timer tick, stops when
  the drag ends or leaves the bar).
- The Settings window becomes resizable with a minimum size of 820 x 600; the
  bars stretch horizontally with the window, the sidebar keeps its width.
- `render-settings` keeps working; with `--width` larger than 820 the bars
  stretch. Renders of a bar that overflows show its right end.
- Pure tests: right-aligned layout vs scrolled layout (document width,
  initial offset), insertion index in document coordinates with a scroll
  offset, auto-scroll step at edges and clamping.

## v1.2 implementation notes

Deviations from the v1.2 scope above, and choices it leaves open.

| Area | As built | Why |
|---|---|---|
| L2 check timing | The first attempt (`activate`, then `makeKeyAndOrderFront` and `orderFrontRegardless`) runs on the next run loop pass through a zero-delay timer. `isActive` is checked 250 ms later (`SETTINGS_ACTIVATE_CHECK`), not on the pass after that; when it is still false, one `activateIgnoringOtherApps(true)` follows and the window is ordered front again. Before macOS 14 both attempts use `activateIgnoringOtherApps(true)`, since `activate` does not exist there. | Activation is asynchronous (`activate` documents that the app may not be active right away), so a check on the next pass would read false almost every time and make the fallback the normal path. |
| L1 live check | Runs once, 500 ms after launch, right before the first hide, while the always-hidden divider is collapsed and on screen. It compares window centers: any other layer-25 window with the divider's y and height, centered on the divider's display, centered left of the divider triggers the stderr warning. It is skipped without a message when titles are unreadable or the divider is on no display. | The scope names the check but not when it runs or how "on its display" is decided; copies of items on other displays share the menu bar's y on this host. |
| L1 reseed | When the order guard finds a wrong order it still removes all three position keys, then seeds only the icon (0) and the hidden divider (1). | The always-hidden divider has no seed any more, so AppKit places it again. |
| L3 auto-scroll | The pointer, which the dragged tile follows, decides, not the tile's frame. Within 24 pt of a strip's edge the bar scrolls by 8 pt times the depth into that zone every 30 ms (a common-modes timer); a pointer past the edge, still within the bar's 12 pt vertical slack, scrolls at the full 8 pt. The timer stops when the offset reaches an end, when the pointer leaves the zone or the bar, on mouse-up, and when a new snapshot arrives. | The scope asks for a bounded step per tick without numbers. A timer keeps scrolling while the pointer rests near an edge, when no drag events arrive. |
| L3 position | A bar keeps its distance from the right end across the 3 s refresh, a snapshot after a move, and window resizes (`bar_scroll::keep_right_distance`). A bar that had nothing to scroll opens at its right end. The relayout reads each bar's offset before it changes the scroll view's frame. | The scope sets only the initial position; resetting to the right end on every refresh would undo the user's scrolling every 3 s. A clip view that grows clamps its origin against the old document width during `setFrame:`, so an offset read afterwards counted the growth as a scroll away from the right end (review, 2026-10-13: at 850 pt the hidden bar sat at offset 5 of 35). |
| L3 scroller | Each bar is a `BarScrollView`, an `NSScrollView` subclass whose `setScrollerStyle:` always passes the overlay style to super. Strips take their height from the scroll view's frame, not the clip view. The knob style follows the bar fill (dark knob on the light tint, light knob on the blue). | AppKit sends every scroll view the new preferred style when it changes (NSScroller.h), for example when a mouse connects with scroll bars set to show automatically. A legacy scroller shrank a 36 pt clip to 19 pt in an offscreen check, which would clip and then shrink the tiles. The default knob follows the window's appearance, which can disagree with the sampled bar. |
| L3 wheel | Scroll wheel events without precise deltas and with only a vertical step scroll a bar sideways (10 pt per line). Trackpad and horizontal events go to `NSScrollView`. A bar that fits has horizontal elasticity off. | A horizontal-only scroll view gives a plain mouse wheel nothing to do otherwise; a bar that fits should not bounce. |
| L3 views | Each bar's scroll view is inset 6 pt from the bar's ends, so tiles clip inside its rounded corners. The tiles and the insertion marker are drawn by the strip (the document view); the canvas draws labels, bar fills, notes and the message underneath; a `DragLayer` above the scroll views, which never takes clicks (`hitTest:` returns nil), draws the dragged tile. The strips pass mouse events to the canvas, which converts the pointer with `bar_scroll::document_x` and the clip view's bounds origin. Tooltip rectangles belong to the strips, in document coordinates. | The dragged tile moves between bars, so it cannot be drawn inside one scroll view. This replaces the v1.1 "Layout bars" scrolling (custom offset and "+N" hint). |
| Window size | Resizable with a minimum content size of 820 x 600 pt; it opens at that size each time and does not remember a larger one. This replaces "Fixed size, 820 x 600 pt" from the v1.1 notes. | The scope asks for resizing only. |
| render-settings | The window is built at 820 x 600 with the page loaded, then resized to `--width` and `--height` with `setContentSize:`, the path a live resize takes. For the layout page it prints each bar's scroll offset and the offset that shows its right end; after a resize an overflowing bar shows the two equal. | The offscreen render then exercises the autoresizing masks and the bar relayout after a resize, which no other offscreen check reaches. The printed offsets make the right-end rule checkable without reading the PNG. |

## Live test v1.2 (2026-10-13, macOS 26.3, Ice quit, maintainer approved)

| # | Result | Evidence |
|---|---|---|
| P7 | Pass (L1): first v1.2 launch ran the migration (`AlwaysHiddenPlacementV2 = 1`, the always-hidden position key removed); AppKit placed the always-hidden divider leftmost (x -11165); the Hidden section held all 19 items and Always-Hidden none; no placement warning. | `defaults read ice-rust`, local probe JSON (not included) |
| P8 | Pass (L2): "Arrange Menu Bar Items..." brought the Settings window to the front without outside help (`lsappinfo front` = ice-rust). `activate()` was refused first; the `activateIgnoringOtherApps` fallback brought it forward (log: "Settings did not come to the front; activating again"). | local app log (not included) |
| P9 | Pass (L3): the Hidden bar (19 items) opened at its right end with no "+N"; a horizontal scroll showed the left end; dragging the first item from the scrolled part to a middle slot moved the real item (143 between 478 and 10399); a drag held at the left edge auto-scrolled to the start and the drop restored the exact original order. | local screenshots and probe orders (not included) |
| P10 | Pass (L4 regression): modifier state stayed clear (0x20000000) after every move. | local `CGEventSourceFlagsState` probe (not included) |
| L6 | Measured: 70 MB with Settings open after two moves; 69 MB after closing it (app back to UIElement). The v1.1 figure of 123 MB was dominated by graphics surfaces (50 MB), which measured 2.2 MB this time. | `footprint` |
| L7 | Open: closing Settings does not release memory (CG image 16 MB and MALLOC_SMALL 24 MB stay). Drop the layout snapshot, tile images and window content on close, then re-measure. | `footprint` before and after close |
| L8 | Observation, not an ice-rust defect: across test rounds Ice's own dividers land at stored distances from the right end, so when items to their right change width an Ice section boundary shifts by an item (this round a third-party item moved from Ice's Visible to Hidden); item order itself was unchanged. | local probe JSON before and after (not included) |

## L9: move refused after a fresh Accessibility grant (2026-10-14)

Reported by the user with the release build: the Permissions page showed
Accessibility as granted, but Menu Bar Layout refused a move with
"Accessibility permission is needed to move items".

| Evidence | Observation |
|---|---|
| TCC (system database) | `kTCCServiceAccessibility` for `local.icerust.IceRust` allowed at 01:11:47; the running process started at 01:11:44. |
| lldb, inside the running process | `CGPreflightPostEventAccess()` = 0, `AXIsProcessTrustedWithOptions(NULL)` = 1, `CGPreflightScreenCaptureAccess()` = 1. |
| A new process of the same bundle (`open -n ... --args probe`) | post events, accessibility and screen recording all granted. |

Root cause: `CGPreflightPostEventAccess` keeps the answer it had when the
process started, while `AXIsProcessTrusted` is live. The Permissions page
counted either grant, but the move engine and click-through required the
post-event preflight, so the three disagreed until a relaunch.

Fix: `PermissionState::can_post_events` (either grant) is the single
predicate for the Permissions page, the move engine, click-through and the
launch-time request. Ice treats Accessibility as its one required permission
for moves and clicks and calls no post-event preflight (ledger section 7),
which is the basis for counting a live Accessibility grant. Not yet verified live:
a move in a process whose Accessibility grant arrived after launch; if the
window server dropped such events, the move engine's placement check would
report "not placed" instead of failing silently.

## v1.3 scope: Launch at login and automatic updates

Requested 2026-10-14. Decided: updater option 1. The user compared it with
Sparkle (which Ice uses) and a notify-only check that links to the release
page, then chose option 1: check GitHub Releases, ask, then download, verify
an Ed25519 signature and install in place. The "Automatic updates" section
below describes it.

The change that implements v1.3 also updates README.md: the "Launch at
login" and "Automatic updates" rows under Features > Other, the "Launch at
login" Roadmap checkbox, and the sentence that says Settings has no Updates
page. The Performance note "No SwiftUI, Combine or Sparkle updater" stays
only if option 1 or the notify-only option is chosen.

Naming: text that sends the user to macOS UI (Login Items, Privacy &
Security, Finder) names the bundle, IceRust, because macOS lists it by
`CFBundleName` and `CFBundleDisplayName` (`scripts/bundle.sh`). Text about
the app itself keeps ice-rust, as the Settings window title and the About
page do.

### Launch at login

- `SMAppService.mainApp` (macOS 13+, crate `objc2-service-management`):
  `register`, `unregister`, `status`.
- General page: a "Launch at login" switch that reflects `status` live:
  `enabled` = on; `requiresApproval` = on with the note "Approve IceRust in
  System Settings > General > Login Items" and a button that calls
  `SMAppService.openSystemSettingsLoginItems()`; `notRegistered` = off;
  `notFound` = off.
- Only a bundled app can register. When the binary runs outside an `.app`
  (for example `cargo run`), the switch is disabled with the note "Available
  when ice-rust runs as IceRust.app". Errors from register/unregister show
  inline on the page and on stderr; the switch then re-reads `status`.
- Nothing is registered automatically; the default is off.

### Automatic updates

Feed and assets:
- Feed: `https://api.github.com/repos/UnkAtreus/ice-rust/releases/latest`
  (published releases only; drafts and pre-releases are excluded by the API).
  For tests only, `ICE_RUST_UPDATE_FEED` may point at a local JSON file
  (`file://`); production accepts `https://` only.
- A release offers an update when its `tag_name` (`vMAJOR.MINOR.PATCH`) is a
  higher semantic version than the running `CARGO_PKG_VERSION`, and it has
  both assets `IceRust-<version>-arm64.zip` and
  `IceRust-<version>-arm64.zip.sig`. Never offer a lower or equal version.
- Signature: Ed25519 over the exact zip bytes; the `.sig` asset holds the
  64-byte signature in standard base64 (one line). The public key is a
  32-byte constant in the source. The private key lives only on the release
  machine (`~/.config/ice-rust/update-signing.key`, mode 600), never in the
  repository.

Checking:
- Setting "Automatically check for updates" on the General page, default on.
- One pure function decides when an automatic check is due:
  `update::check_due(now, launched_at, last_success, last_attempt,
  enabled, running)`. All times are wall-clock (`SystemTime`). A check is
  due when all of these hold:
  - the setting is on and no check is running;
  - `now` is at least 60 s after launch;
  - `LastUpdateCheck` is missing, at least 24 h before `now`, or later than
    `now` (the clock moved backwards; treat it as missing);
  - no attempt in this process, successful or failed, started within the
    last hour.
  The first automatic check after a launch is therefore due at
  max(launch + 60 s, LastUpdateCheck + 24 h), so relaunches (at login,
  after an update) do not each cause a request.
- Only a successful check writes `LastUpdateCheck` (seconds since the Unix
  epoch). A failed check writes nothing, and the one-hour attempt limit
  spaces out retries.
- `check_due` runs 60 s after launch, then from a repeating one-hour timer,
  and on `NSWorkspaceDidWakeNotification` (posted on the `NSWorkspace`
  notification center). Run loop timers count only time the Mac is awake,
  so a repeating 24 h timer would stretch to several days on a laptop that
  sleeps every night; comparing wall-clock time every hour keeps the
  interval close to 24 h. This is known run loop timer behavior, not
  measured on this host.
- "Check for Updates..." in the icon menu and a button on the About page
  check now, whatever the setting, and report "ice-rust is up to date" or
  the error. A successful manual check also writes `LastUpdateCheck`.
- Network and parsing run on a worker thread; results return to the main
  thread. Downloads use `/usr/bin/curl` with an argument vector (no shell),
  `--fail --location --proto =https --max-filesize 50M --max-time 120`.
- "Skip This Version" stores the version (`SkippedUpdateVersion`); automatic
  checks do not offer it again; a manual check does.

Offer and prompt:
- An automatic check never opens a window or an alert on its own. When it
  finds an offerable version, it adds "Install Update <new>..." to the icon
  menu and a banner with the same action at the top of the General page.
  Nothing else happens until the user acts.
- The prompt opens only from a user action: that menu item, the banner,
  "Check for Updates..." or the About page button. It opens Settings on the
  About page through `open_settings`, which already switches to the regular
  activation policy and activates on the next pass with the forced
  fallback (L2), and shows the prompt as a sheet on that window
  (`NSAlert.beginSheetModalForWindow:completionHandler:`).
- Never call `runModal`. An app-modal session runs the main run loop in
  `NSModalPanelRunLoopMode`, where the rehide ticker, the click-settle poll
  and the mover poll stop firing, because `timer::schedule` fires in the
  default mode only. A new timer that must keep running while a menu or a
  sheet is up uses `timer::schedule_common`.
- Prompt text: "ice-rust <new> is available (you have <current>)", the
  release notes (plain text, at most 1500 characters), the permissions note
  below, and buttons Install / Later / Skip This Version. When the install
  precheck (Install step 1) fails, the prompt shows its message and
  replaces Install with Open Release Page.
- Permissions note. IceRust is signed ad hoc (`codesign --sign -` in
  `scripts/bundle.sh`), so its designated requirement is the cdhash of one
  build, and every new build stops matching the Screen Recording and
  Accessibility grants. System Settings may still show IceRust switched on
  while the grant no longer applies; that follows from the cdhash
  requirement and was not reproduced on this host. The prompt and the
  README say: "After updating, turn IceRust off and on again in Privacy &
  Security > Screen Recording and in Privacy & Security > Accessibility, or
  remove it with the minus button and add it again." This holds for every
  update path (option 1, Sparkle, or a manual download after a notify-only
  prompt) while builds are signed ad hoc. Developer ID signing is the only
  change that keeps the grants across versions.
- First launch of a new version: the app stores `LastLaunchedVersion`. When
  the running version differs from it and `permissions::check()` reports
  Screen Recording missing or `can_post_events()` false, the app opens
  Settings on the Permissions page with the same instruction. The decision
  is a pure function with tests.

Install (`update::install`, also exposed for tests as
`ice-rust apply-update --app <path> --zip <zip> --sig <sig>`, which installs
into the given bundle path and does not relaunch):
1. Precheck, before the prompt offers Install and again at the start of
   `install`:
   - refuse when the running app is not a bundle, or runs from an App
     Translocation path (`/AppTranslocation/`): "Move IceRust.app to the
     Applications folder to install updates";
   - refuse when `access(2)` with `W_OK` fails on the bundle's parent
     directory or on the bundle: "Updating needs write access to <dir>. Ask
     an administrator to install ice-rust <new>, or move IceRust.app to a
     folder you own." A standard account cannot write to `/Applications`
     (`drwxrwxr-x root admin` on this host). The precheck is advisory; step
     5 still handles a failed rename.
2. Create a staging directory next to the bundle (same parent, named
   `.IceRust.update.XXXXXX` by `mkdtemp`, mode 0700) and download the zip
   and sig into it. Staging on the bundle's own volume keeps the step 5
   rename from failing with `EXDEV` when IceRust.app lives on another
   volume than `$TMPDIR` (`/private/var/folders`, on `/System/Volumes/Data`
   on this host). Every exit path, success or failure, removes the staging
   directory.
3. Verify the Ed25519 signature over the zip bytes before anything else
   touches the zip. Any failure stops the install.
4. Extract with `/usr/bin/ditto -x -k` into the staging directory, then
   require exactly one `IceRust.app` with bundle identifier
   `local.icerust.IceRust` and `CFBundleShortVersionString` equal to the
   offered version, and a valid signature (`codesign --verify --strict`).
5. Swap with `rename(2)` inside the parent directory: rename the current
   bundle to `<name>.app.previous`, then rename the new bundle from the
   staging directory into place; if the second rename fails, rename the
   previous bundle back and report the error.
   A bundle folder the user cannot write (for example `/Applications` for a
   non-admin account, which is root:admin 775) stops the install before the
   swap with an error that names the folder; there is no admin prompt.
6. Relaunch: start a detached `/bin/sh -c` helper that receives the pid and
   the bundle path as positional arguments (never interpolated into the
   script), waits for the pid to exit, then runs `open <path>`; then the app
   terminates through `NSApplication.terminate`.
7. On launch, a bundle that finds `<name>.app.previous` next to itself
   deletes it, and removes any `.IceRust.update.*` directory next to it
   that the current user owns (left by an interrupted install).

Release tooling:
- `ice-rust update-keygen --out <path>` writes a new private key file
  (mode 600, refuses to overwrite) and prints only the public key in base64.
- `ice-rust update-sign --key <path> <zip>` writes `<zip>.sig`.
- `scripts/release.sh <version>`: bump nothing itself; check that Cargo.toml
  has the version, build with `OUT_DIR=dist`, zip with ditto, write
  `.sha256` and `.sig`, verify the signature with the embedded public key,
  and print the `gh release create` command without running it.

Verification:
- Pure tests: version parsing and comparison, asset selection from a
  fixture of the GitHub JSON, notes trimming, `check_due` (no
  `LastUpdateCheck`; a recent check, so no request at launch + 60 s; a
  stale check; a clock moved backwards; before launch + 60 s; setting off;
  a check already running; a failed attempt within the last hour), skip
  handling, the first-launch permissions decision, Ed25519 verify with a
  test key (valid, tampered zip, wrong key, malformed base64),
  translocation detection, helper arguments.
- Tests on temporary directory trees: the write-access precheck on a
  directory with mode 0555 (refused; skipped when the tests run as root),
  staging created next to the bundle and removed on success and on each
  failure, swap and rollback, cleanup of a stale staging directory.
- `ice-rust check-update --json` performs one real read-only check.
- An end-to-end install test on scratch copies only: a 0.0.0 test bundle is
  updated from a locally signed zip with `apply-update`; the user's running
  IceRust is never touched. Live GUI checks need the user.

### v1.3 implementation notes (2026-10-14)

Deviations and choices the spec left open:

| Item | Implementation | Why |
|---|---|---|
| Where the zip is extracted | Downloads go to a private temporary folder; the verified bytes are written to a staging folder next to the bundle and extracted there. | The final move must be a rename on the same volume; extracting from bytes already in memory leaves no gap between verification and use. |
| Extra command | `ice-rust update-verify <zip>` checks `<zip>.sig` with the compiled-in key; `scripts/release.sh` calls it. | The spec asked release.sh to verify with the embedded key; a build with the placeholder key fails here, so it cannot be released. |
| Release zip | Holds `IceRust.app` only; LICENSE and NOTICE are inside the bundle's Resources. | The updater requires exactly one entry; the license still travels with every binary. |
| Not-installable releases | A newer release without both signed assets, served without https, or over 50 MB is reported with a button to the release page. | Covers 0.1.0-style releases and mistakes in a release. |
| Permission notice | Shown once per version, 5 s after launch, when Screen Recording or event posting is missing. | Ad-hoc signatures change the code identity on every update (TCC re-grant). |
| Automatic check pacing | First check 60 s after launch, then due once 24 h have passed on the wall clock (polled hourly, because run loop timers stop during sleep), at most one automatic attempt per hour. | Sleep and failed attempts would otherwise skip or repeat checks. |
| Non-admin accounts | Write access to the bundle and its folder is checked before Install; without it the prompt explains and links the release page. | No administrator prompt in this version. |

Verified on scratch copies (debug build with a test key): install, tampered
zip, wrong key, same version, extra zip entry, and a release build ignoring
the test key. Verified with the release build and the real key: a signed
0.0.1 test zip installed over a 0.0.0 copy, and a zip signed with the test
key was refused. One read-only `check-update` against GitHub reported
0.1.0 as current and up to date.

After publishing v0.1.1: the downloaded assets matched the local build
byte for byte. A copy of the code labeled 0.1.0 saw 0.1.1 as available
with both asset URLs, and its check and `download_and_install` path,
run once against GitHub, replaced a copy of the published 0.1.0 bundle
with 0.1.1 (strict code signature valid, binary identical to the
release, no staging or download folders left). `apply-update` with the
release binary refused the same version and a tampered zip.

Verified by hand with the 0.1.1 release bundle (2026-10-14): the
permission notice appeared after the first launch of the new version,
turning on Launch at login added IceRust to System Settings > General >
Login Items, and Check for Updates... reported the latest version both
from the menu and on the About page.

The update prompt and the relaunch were verified with the 0.1.2 release
(P15 below).

## Live test 0.1.1 (2026-10-14, macOS 26.3, Ice quit, maintainer approved)

Run against the release bundle in `dist/`. Settings was opened and closed
through Accessibility actions (no pointer events). Synthetic clicks put
the pointer back where it was. A small frontmost window logged any key it
received, so a stray Escape could not reach the user's apps. Every step
first checked that no user input had arrived since the previous step.
Modifier state was clear (0x20000000) afterwards.

| # | Result | Evidence |
|---|---|---|
| P11 | Pass (G3 hold): with the hidden section revealed and the AirDrop menu (Control Center, layer 100) open, the section stayed revealed for 31 s, twice the 15 s interval, and the menu stayed open. After Escape closed the menu, the section rehid 13 to 15 s later. The pointer stayed below the menu bar throughout. | hidden divider width polled every 2 to 3 s (26 pt revealed, 5016 pt hidden); on-screen menu windows |
| P12 | L5 not reproduced: a synthetic Escape (HID event tap) closed the ice-rust icon menu opened by a right-click (through `performClick`) in 2 of 2 runs, and closed a Control Center module (Battery) as a control. The key-logging window received no key, so the menu took the Escape. | on-screen menu windows before and after; key log |
| P13 | L7 confirmed, without growth: after each of three open and close cycles of Settings on the Layout page the footprint was 68 to 69 MB (MALLOC_SMALL 25 to 26 MB, CG image 15 MB); an open window added about 1 MB. The memory is kept, not leaked per cycle. | `footprint` |
| P14 | Ice Bar overflow not testable on this machine: the three sections together measure about 1220 pt and the narrowest display is 1512 pt, so the "+N" tile cannot appear. The overflow rule is covered by unit tests in `bar_layout.rs`. | read-only probe |

## L7 root cause and L10 (2026-10-14)

Measured on 0.1.1 from `dist/` with `footprint`, `heap`, `vmmap`,
`leaks --traceTree` and `malloc_history` (MallocStackLogging, one run).

| Evidence | Observation |
|---|---|
| `heap`, a fresh process | 45 to 53 MB footprint; CG image 17 to 23 MB; no Settings objects. |
| `heap`, 20 s and 50 s after closing Settings | Still alive: one NSWindow, the LayoutCanvas, 4 sidebar rows, 6 page views and 36 text field cells (3 before opening). The Rust side had dropped its references. |
| `leaks --traceTree` on that NSWindow | Referenced by NSApplication's `_previousKeyWindow` and by an autorelease pool page of the main thread, which was waiting in `_DPSNextEvent` for the next event. |
| `heap` after other events reached the app | The window and its views were gone. Live malloc 24.4 MB against 29.9 MB at launch; MALLOC_SMALL dirty pages 24 MB against 14 to 16 MB, which is freed memory the allocator keeps. |
| `malloc_history`, fresh launch | About 26 MB of purgeable blocks (1.8 MB and 0.5 MB each) come from `_NSCreateStatusItemShadowImage`: AppKit draws a shadow image for each status item (two `NSStatusBarShadowView`s per item window), and the expanded dividers are 5016 pt wide. They exist before Settings ever opens. |
| lldb, divider buttons hidden | No change: the shadow views still draw the full width. |
| lldb, expanded length 2700 pt | Footprint fell by about 7 MB and the hidden items stayed off screen on this display arrangement. |
| lldb, content view and delegate detached from the closed window | The views stayed alive: they were also in the main thread's autorelease pool, which NSApplication drains only after the next event. Timers and Accessibility actions run while it waits for that event. |

L7 had two parts. The closed window and everything in it stayed alive
until AppKit let go of it at a later event; with no other windows and no
input, that could take a long time. Fixed in 0.1.2 in two steps: when the
app drops the window it first detaches the content view and the delegate
(`SettingsWindow::release_content`), so only an empty window stays with
AppKit; and every timer callback, including the one that drops the window,
runs in its own autorelease pool (`platform/timer.rs`), so what AppKit
autoreleases there is freed when the callback returns instead of at the
next event. The rest of the footprint after closing is not Settings memory:
freed allocator pages that are reused later, AppKit caches (including the
16 menu bar windows AppKit creates on four displays while ice-rust is a
regular app), and the divider shadow images below.

| # | Finding | Status |
|---|---|---|
| L10 | AppKit's status item shadow images for the two expanded dividers take about 15 to 20 MB of the footprint from launch. A shorter expanded length would shrink them, but it could leave hidden items on an adjacent display when "Displays have separate Spaces" is off, which this machine could not test. Ice uses the same 10000 pt length. | Open, documented; length unchanged. |

## Live test 0.1.2 (2026-10-14, macOS 26.3, maintainer approved)

| # | Result | Evidence |
|---|---|---|
| P15 | Pass (update from 0.1.1): Check for Updates... in the running 0.1.1 showed "ice-rust 0.1.2 is available (you have 0.1.1)" with the release notes and Install, Skip This Version and Later. Install replaced `dist/IceRust.app`, and the app relaunched as 0.1.2 about 2 s later with a valid strict code signature. The relaunched app removed `IceRust.app.previous` and recorded the new version. All three permissions read as missing in the new process, as expected for a new ad-hoc signature. | buttons and text read through Accessibility; `pgrep`, `plutil`, `codesign`; lldb preflight calls |
| P16 | Pass (L7 fix): on 0.1.2, 3 s after closing Settings with its close button, the LayoutCanvas, the sidebar rows and the page views were freed, and text field cells fell from 36 to 4 (3 before opening). One empty NSWindow stays with AppKit. On 0.1.1 the same objects were still alive 50 s after closing. Footprint 62 MB before opening, 74 MB open, 69 MB 18 s after closing. Run before the permissions were granted again, so the layout page held no item images. | `heap`, `footprint` |

| # | Finding | Status |
|---|---|---|
| L11 | The update alert shows the release notes as raw Markdown ("##" headings, "**" emphasis). | Fixed in 0.1.3 (see below). |

## Issue #1 and L11 (2026-10-14)

[Issue #1](https://github.com/UnkAtreus/ice-rust/issues/1) (external
report, 0.1.0 era): Menu Bar Layout showed "IceRust.ControlItem.Hidden not
found" in all three bars. It does not reproduce on the development host,
where the divider windows carry their autosave names, and the report gives
no macOS version or display setup. `find_divider` returns NotFound only
when some status window titles are readable and none matches, so two
causes fit: the divider's window has another title on the reporter's
system, or the divider is not in the menu bar at all.

| Change in 0.1.3 | Covers |
|---|---|
| Every lookup of a divider (layout page, Ice Bar sections, the icon order check, the always-hidden placement check) falls back to the layer-25 window whose bounds match the divider's own window frame from AppKit, converted to CG space (`chain::find_divider_or_frame`). | A title that differs from the autosave name. |
| After creating the three status items, any item AppKit reports as not visible is made visible. The seeds fill only missing keys, so a stored `NSStatusItem Visible` false would otherwise keep a divider out of the menu bar. | A divider that is not in the menu bar. |
| The layout message reads "IceRust.ControlItem.Hidden is not in the menu bar. Quit and reopen ice-rust". | What remains. |

Verified: unit tests for the fallback (a different title, no frame, a
frame elsewhere, the title winning over the frame, unreadable titles
staying unreadable, rounding, other layers). On the development host the
title path still finds the dividers. The fallback itself could not run
live here; whether it fixes the reporter's case needs their confirmation.

L11 fixed in 0.1.3: release notes go through `plain_notes` before
`trim_notes`, so the update alert shows headings without `#`, bullets
instead of list markers, joined wrapped lines, and no `**`, `__`,
backticks or link targets.


## v1.4: menu bar icon styles (maintainer request 2026-10-15)

Request: the icon that shows and hides the hidden items had only the
ice-rust logo; offer several looks and let the user change it.

| Part | Choice |
|---|---|
| Styles | `menu_icon::IconStyle`: the logo (default), Ellipsis (three drawn dots), ten SF Symbols pairs (Dot, Ellipsis Circle, Chevron, Arrow, Eye, Door, Sunglasses, Snowflake, Grid, Star) and Custom Image. Ice offers a similar list (Arrow, Chevron, Door, Dot, Ellipsis, Ice Cube, Sunglasses, Custom) with its own artwork; SF Symbols ship with macOS, so ice-rust adds no image files. |
| Ellipsis | Added on maintainer request the same day: three dots with no circle. SF Symbols has only a solid `ellipsis`, so `menu_icon::dot_frames` lays out three 4 pt dots 2.5 pt apart (17 x 4 pt) and `platform::menu_icon` draws them as a vector template image, solid while items are hidden and outlined (1.1 pt line) while they are shown, like Ice's EllipsisFill and EllipsisStroke. The circled symbol pair became Ellipsis Circle. |
| States | Each style has an image for items hidden and one for items shown (a section revealed or the Ice Bar open), as the logo already had. Most fill in; Chevron and Arrow turn around, Eye and Door open, Sunglasses come off. Both images of a pair have the same width, so the menu bar does not shift by a point when the state changes. |
| Size | Symbols at their default size, the same as the divider chevrons. |
| Custom image | Converted once into a PNG fitted to 24 x 18 pt at 3x and stored in defaults (`CustomIconImage`), so the original file can move. Match menu bar color (`TintCustomIcon`) makes it a template image; its first value comes from `menu_icon::suggests_tint` (some clear pixels, at most 2 % colored pixels). |
| Fallback | Missing symbols, or Custom Image without a stored image, show the logo. |
| UI | A Menu bar icon group at the top of the General page: the style popup with each style's image, then Choose Image... and the Match menu bar color checkbox (enabled for Custom Image). Picking Custom Image with no stored image opens the panel; cancelling it puts the popup back. |

Verified on the development host (macOS 26.3): unit tests for the style
keys, the symbol pairs, the fit and the tint suggestion; every style's
images load with the template flag and the accessibility description; the
Custom fallback shows the logo; PNG, JPEG, HEIC, TIFF, PDF and SVG files
convert (a 4000 x 1000 image becomes 72 x 18 px, a 4 x 4 image grows to
54 x 54), the tint suggestion is on for black and white glyphs and off for
the colored app icon and opaque images, and a text file, an empty file and
a missing file are refused. `render-settings --page general` drew the
group with the logo, Eye and a colored custom image. A grid drawn from
`style_images` shows both images of every style, the Ellipsis dots solid
and outlined at 17 x 4 pt.

## Permissions after updates (L12) and issue #2 (2026-10-15)

Maintainer report: after installing a new build, ice-rust asked for its
permissions although both were granted. System Settings showed IceRust
switched on, turning the switches off and on did not help, and only
removing both entries with the minus button, then opening ice-rust and
granting again, worked.

Evidence (macOS 26.3, read-only queries of the system TCC database):

| Observation | Result |
|---|---|
| TCC rows for `local.icerust.IceRust` | `kTCCServiceScreenCapture` and `kTCCServiceAccessibility`, auth_value 2, auth_reason 4, both changed at 15:02 when the maintainer re-added them |
| Stored `csreq` (decoded with `csreq -t`) | `cdhash H"f42e5185..."` for both, the cdhash of the running dist build |
| Designated requirement of the 0.1.4 release | `cdhash H"ffbdd389..."`, which does not meet the stored requirement |
| `probe --json` launched with `open` from a copy of the dist build | Screen Recording, event posting and Accessibility true; titles readable; divider found |
| The same from a copy of the 0.1.4 release (same bundle ID) | all three false; "window titles are unreadable"; no divider |
| TCC rows before and after both probes | identical |

Cause (L12): every release up to 0.1.4 has an ad-hoc signature, whose
designated requirement is the cdhash of that build. TCC stores the
requirement of the granted build and checks later launches against it, so
each update loses both grants while System Settings still lists them as on.
Turning a switch off and on keeps the stored requirement; removing the
entry drops it. The README, the update prompt and the notice after a
version change told users to turn the switch off and on, which does not
work.

Fix (0.1.5):

| Change | Detail |
|---|---|
| Release certificate | Self-signed RSA 3072 code signing certificate "ice-rust Release Signing", valid until 2046-10-15, SHA-1 `A8F20285 6370885D BF698604 320FD846 F8145BF2`, SHA-256 `F8DD7B80 496B7372 3AF76990 70EBE9B9 5325C35A 7EE59BF4 3DE6FC68 78F3AFC3`. Key and certificate in `~/.config/ice-rust/` (mode 600), identity in the maintainer's login keychain. |
| Requirement | `identifier "local.icerust.IceRust" and certificate leaf = H"a8f20285..."`. A test with a throwaway certificate in a temporary keychain: two different builds signed with it each met the other's requirement; the ad-hoc 0.1.4 did not. |
| Scripts | `bundle.sh` signs with the identity when the keychain has it (found by SHA-1, since the certificate is untrusted), otherwise ad-hoc. `release.sh` fails without it and checks that the requirement names the certificate. |
| Texts | The update prompt says the grants carry over; the notice after a version change and a new line on the Permissions page say to remove the entry with the minus button, not to toggle it. |
| Updater | Unchanged: `codesign --verify --strict` accepts the certificate-signed bundle. |

Moving from 0.1.4 or earlier to 0.1.5 still needs one removal and new
grant, since the stored requirements are cdhashes.

Live test (2026-10-15, macOS 26.3, maintainer approved): dist/IceRust.app
was rebuilt with the certificate (cdhash `0980e178...`). A second build, the
0.1.4 binary signed with the same certificate (cdhash `702f45b2...`), served
as the next version. Its `probe --json`, launched with `open`, reported all
three permissions false and unreadable titles while the rows still held the
old cdhash. The maintainer removed both entries, opened the dist build,
granted both and relaunched it; both rows then stored
`identifier "local.icerust.IceRust" and certificate leaf = H"a8f20285..."`.
The second build, never granted itself, then reported Screen Recording,
event posting and Accessibility true, read the titles and found the hidden
divider with 3 hidden items. The probes left the rows unchanged. Grants
carry over between builds signed with the certificate.

Not done: a button that resets the entries with `tccutil reset`. `tccutil`
looks the bundle up through LaunchServices, which did not find test copies
under the scratch folder, and resetting the real bundle ID would have
removed the maintainer's working grants.

Issue #2 (external, macOS 27.0.1): the Ice Bar showed "Screen Recording
permission needed" while the Permissions page showed both permissions
granted. On macOS 26.3 a stale grant makes `CGPreflightScreenCaptureAccess`
return false, so the Permissions page would say missing; the report shows
the opposite, which L12 alone does not explain. Possible causes: a grant
given while ice-rust ran and no relaunch, or macOS 27 withholding status
window titles from a process that has Screen Recording. Open; a reply asking
the reporter to remove and re-add both entries and to send `probe --json`
output is drafted but not posted.

## v1.5: show on click, hover and scroll (maintainer request 2026-10-15)

Request: implement Ice's "Show hidden items when hovering over the menu
bar", "when an empty area in the menu bar is clicked" and "by scrolling or
swiping in the menu bar".

Ice's mechanism is in [ice-internals-evidence.md](ice-internals-evidence.md)
section 6: universal (local plus global) NSEvent monitors for mouse down and
scroll, a listen-only HID tap for mouse moves, and a four-part empty-space
test (menu bar, outside the app menus from AX, outside item windows,
outside the notch).

| Part | Choice |
|---|---|
| Monitors | Local plus global `NSEvent` monitors per enabled switch: mouse down for click (and the hover latch), scroll wheel, mouse moved for hover. Ice uses a CGEventTap for mouse moves; an NSEvent monitor needs no tap and sees the same moves. Monitors are reinstalled when the switches change; none run for disabled triggers. |
| Empty space | `triggers::is_empty_space`, pure, in CG space. The menu bar is the one with the ice-rust icon. App menus: the frame of the last child of the frontmost app's `AXMenuBar`, 0.1 s messaging timeout (`platform::menu_bar_space`). Items: on-screen layer-25 windows; the expanded dividers are not on screen (measured: `onscreen=false` for both while hidden). Notch: the gap between `auxiliaryTopLeftArea` and `auxiliaryTopRightArea`, plus 1 pt. |
| Click | Left mouse down in empty space, after 50 ms: `toggle(Hidden)`, Option `toggle(AlwaysHidden)`; Shift, Control or Command do nothing. In Ice Bar mode an open bar closes through the existing outside-click monitor and the reopen guard. |
| Scroll | `scroll_amount` signs the averaged deltas by `isDirectionInvertedFromDevice` (down or right positive, natural scrolling on or off) and counts wheel lines as 10 pt; `ScrollGesture` sums events less than 300 ms apart and acts once per gesture at +10 (show) or -10 (hide). Ice uses each event's raw average against 5. |
| Hover | `triggers::Hover`, pure with injected time. A move schedules one check after the delay (0.2 s default, 0 to 1 s); the check re-tests precisely and does nothing while a menu is open. Only hover-shown items hide on leaving (outside the menu bar and 10 pt from the Ice Bar); `release()` on toggle, hide, close_bar, click-through and any click in the menu bar plays the role of Ice's `showOnHoverAllowed` latch. |
| Settings | `ShowOnClick` (true), `ShowOnHover` (false), `ShowOnHoverDelay` (0.2), `ShowOnScroll` (true), Ice's keys and defaults. A new Hidden Items page holds them with Use Ice Bar and the rehide settings; General keeps the icon, launch at login and updates. |

Measured on the development host (macOS 26.3, four displays, Arc in
front): the AX read of every menu took 79 ms, of the last menu 25 ms; Arc's
menus end at x=3166 on the display at x=2560; the built-in display's notch
is CG (-847, 1080, 185, 32). Status items on the other displays are copies
owned by Control Center, so the triggers act on the active menu bar only.

Not done: Ice's secondary context menu on a Control-click in empty space;
triggers while ice-rust itself is in front (its own AX menus are not read
from its main thread).

Hand test (2026-10-15, macOS 26.3, maintainer): the dist build of 7b583df,
signed with the release certificate, kept both permissions without a new
grant. Show on click (plain and Option-click, other clicks unchanged), Show
on scroll (down shows, up hides, once per gesture) and Show on hover (shows
after the delay, hides on leaving, stays after a click in the menu bar) all
passed. The global monitors receive clicks and scrolls over the menu bar on
macOS 26.3.

## v1.6: hide application menus (maintainer request 2026-10-15)

Request: implement Ice's "Hide application menus when they overlap with
shown items".

Ice's mechanism ([ice-internals-evidence.md](ice-internals-evidence.md),
section 1, HideApplicationMenus): on a control item state change while a
section is shown, if the leftmost item's minX is at or left of the AX app
menu frame's maxX, Ice sets `.regular` and activates itself with its own
main-menu items hidden; with no section shown it sets `.accessory` and
deactivates. Off while Use Ice Bar is on. Default on.

| Part | Choice |
|---|---|
| Decision | `app_menus::menu_action`, pure: hide when enabled, Use Ice Bar off, a section shown, Settings closed and the items reach the menus; keep hiding while that holds; restore otherwise. `items_reach_menus` is Ice's leftmost-edge test. |
| Measuring | 200 ms and 600 ms after each state change, the setting changes and Settings closing. Items: on-screen layer-25 windows on the menu bar with our icon. Menus: the Show on click Accessibility read (last `AXMenuBar` child, fresh). |
| Hiding | Remember the frontmost app, hide every top-level item of our main menu, `.regular`, forced activation on the next run loop pass (a status item click does not activate the app; same pass failed for Settings, L2). |
| Restoring | Unhide the menu items, `.accessory`, activate the remembered app. Another app becoming active (`NSWorkspaceDidActivateApplicationNotification`) restores without refocusing. Opening Settings hands the remembered app to Settings and keeps ice-rust regular. |
| Interplay | While hiding, our own pid is in front; the empty-space test takes the app menus to end 60 pt from the left edge (Apple menu measured at 44 pt) instead of reading our own menus from the main thread. |
| Setting | `HideApplicationMenus` (true), a switch on the Hidden Items page, enabled while Use Ice Bar is off. |

Difference from Ice: switching to another app ends the hiding at once; Ice
stays `.regular` until the sections hide.

## L13: moves that need more room than the notch leaves (2026-10-15)

Maintainer report (screenshot): Menu Bar Layout showed "Move failed: the
item or its neighbor did not appear on screen within 1 s" while the
built-in display (1512 pt, notch) had the active menu bar.

Evidence (`probe --json`, read-only): visible 10 items, 491 pt; hidden 3
items, 104 pt; always-hidden 16 items, 585 pt. Notch CG (-847, 1080, 185,
32), so status items have 1512 - 850 = 662 pt right of it. A move shows the
sections it touches (`MenuBarState::covering`): visible plus hidden needs
about 625 pt and fits; any move into or out of the always-hidden section
shows every section, about 1240 pt, and macOS draws only what fits right of
the notch. The item or its neighbor never reaches the screen, and the move
times out. On the 2560 pt displays used before, everything fits. Hide app
menus cannot help: the limit is the notch, and Settings (ice-rust's own
short menus) is in front during a move.

Change: the move reads the room when it starts (`MoverHost::menu_bar_room`:
section widths from the chain walk, collapsed dividers counted as 30 pt
when expanded, and `geometry::status_item_room`, the width right of the
notch). A timeout with more room needed than free reports `NoRoom`: "not
enough room in the menu bar: this move shows sections that need about
1240 pt, and 662 pt are free right of the notch. Try it while a wider
display has the menu bar". Other timeouts keep the old message. Nothing is
refused up front, since the estimate could be off by a divider. The layout
page now wraps its message line, up to three lines.

Confirmed by the maintainer: the same moves fail with the menu bar on the
built-in display and work normally once a large display has it.

Not done: moving off-screen items without showing them, which Ice does
through a private event relay.

## L14: Hide app menus did not act (2026-10-15)

Maintainer hand test with a test app, MenuCrowd (16 menus, ending at
x = -252 on the built-in display, right of the notch at -662): nothing
happened. Then a live test with synthetic scrolls over the menu bar
(maintainer approved, idle guard before each step) and temporary stderr
logging found two faults.

| Fault | Evidence | Fix |
|---|---|---|
| The overlap test never fired. | Log: app menus end -252, but only 3 on-screen status items on the bar, leftmost -226. `probe`: the visible and hidden chains keep their laid-out positions (-619 to 0, contiguous) with `onscreen` false for every item left of -252, the icon and the hidden divider included. macOS 26 takes colliding status items off screen instead of drawing them under the menus. | Measure the laid-out positions from the full window list: the visible chain, plus each shown section's chain and collapsed divider. Also require, on a notched display, menus that end right of the notch (`menus_take_item_room`). |
| Activation was refused. | After the first fix the log showed "hiding application menus" and `lsappinfo` reported ice-rust as Foreground, but MenuCrowd stayed in front. `activateIgnoringOtherApps` is cooperative since macOS 14; the scroll went to MenuCrowd, not to ice-rust. | `NSRunningApplication.current.activateFromApplication:options:` with the remembered front app, as Ice does ("activates itself from the frontmost app", evidence section 1). |

Result after both fixes, same synthetic steps: a swipe down showed the
hidden section, ice-rust became the active app, the menu bar showed only
the Apple menu with every icon drawn (screenshot); a swipe up hid the items,
MenuCrowd came back to the front with its menus and ice-rust returned to
UIElement; switching to MenuCrowd while the menus were hidden ended the
hiding at once with the items still shown. The pointer was put back
afterwards.

While MenuCrowd's menus cover the icon, the icon cannot be clicked; Show
on scroll still shows the items, and the hiding then brings the icon back.

Hand test (2026-10-15, maintainer): with MenuCrowd in front on the
built-in display, a real two-finger swipe hid the app menus and showed every
icon, and hiding the items brought the menus and the focus back.

## v1.7: smoother Menu Bar Layout moves (maintainer request 2026-10-15)

Maintainer reports: dragging in Menu Bar Layout was not smooth like Ice
(a loading state, the Always-Hidden bar disappearing and coming back, "The
menu bar order does not match the requested order"); after a first fix,
dragging always-hidden items left to right made items on the right vanish
for a while after the drop with "The menu bar did not end up in the order
you dropped", and the pointer flickered and froze briefly.

| Finding | Evidence | Change |
|---|---|---|
| The settle check accepted reads that were missing items | Log: "layout settled after 2 reads (~303 ms)" while items were still sliding back; a probe a minute later showed every always-hidden item. | Settling needs two equal reads that also contain every item the bars show; otherwise the bars keep the dropped order and the periodic refresh catches up. |
| Every read captured images and the page dimmed while moving | Code review. | Order-only reads while settling; the dropped order shows at once (`apply_drop`), drags stay possible and queue. |
| Sections shown for a move went back between queued moves | Code review. | Queued moves keep the sections shown until the last one. |
| The pointer froze after moves | Ice's evidence: its event sources permit local mouse, keyboard and system events during suppression with a suppression interval of 0, and it posts only a Command mouse-down and mouse-up. | The same permits and interval on our sources, one kept for the run, and re-associating mouse and cursor after the warp back. |

Not adopted: Ice moves off-screen items through a private event relay with
a source-PID helper on macOS 26, a larger rework.

Hand test (2026-10-16, maintainer, macOS 26): "smoother". The log showed
four moves, each with the pointer hidden 101 to 106 ms, three placed on the
first drop and one on the second, and the bars settled after 2 to 4 reads
(304 to 606 ms).

## v1.8: the macOS 27 menu bar (issue #2, 2026-10-16)

macOS 27 draws the menu bar in `MenuBarAgent`: one window per display, each
status item a slot in it, no item windows in the window list, and a status
item as wide as half the display dropped instead of laid out. ice-rust 0.1.6
found no windows and showed "Screen Recording permission needed", and its
10000 pt divider was dropped, so nothing hid. The research, the VM
experiments and the design are in [macos27.md](macos27.md); this entry
records the decisions and the live test.

Approach A (no private API): hiding uses macOS's own overflow. The hidden
divider and a new spacer item directly left of it grow to the longest length
every display lays out (floor(narrowest width / 2) - 18), and MenuBarAgent
folds every item that no longer fits, from the left. Over-filling is safe:
an experiment with too little room folded the spacer and the divider too and
kept the visible items. The room is not measured. Before macOS 27 nothing
changes.

| Decision | Why |
|---|---|
| A visible spacer of length 0 (a 16 pt blank) left of each divider | One item at the cap covers at most half the display; with Finder in front on 1920 pt that left 7 test items on the bar. A hidden item keeps an old position value while MenuBarAgent recomputes the others, so it could drift right of the divider. The always-hidden spacer came after a maintainer report: with only the always-hidden divider growing, always-hidden items stayed on the bar while the hidden section showed, all of them with ice-rust's short menus in front. |
| Seeds icon 1, hidden divider 2, spacer = hidden + 1 | MenuBarAgent orders items by this value from the trailing end and reads 0 as no value; the macOS 26 seeds put the icon at the far left (folded first) and the divider at the far right. It reads an app's seed only for an item it has not placed yet. |
| Order and spacer place from AppKit frames, only when they show the item collapsed and 400 ms after the last change | AppKit reports the new width at once but the old position until MenuBarAgent lays out again; a folded item's frame is not where it is. Reading early made a hide right after a show leave the spacer at 0. |
| Show on click takes occupied space from MenuBarAgent's Accessibility tree, our expanded items excluded | Without item windows every click counted as empty space. A click on the empty-looking bar lands on our own expanded divider: the global monitor sees it on MenuBarAgent's window and MenuBarAgent forwards it to the button 23 ms later, so the local monitor ignores clicks on our own item windows (it showed and hid at once before). |
| Ice Bar and Hide app menus off, with "Not available on macOS 27 yet" | They need item windows (captures, the chain walk). The stored choices apply again before macOS 27. |
| Menu Bar Layout from MenuBarAgent's slots, with every section shown while the page is open | Folded items have no position to read or drag, and only the ones that would fit in the overflow are even listed. Ids are hashes of process, name and ordinal, since there are no window ids. A move is one Command-drag (it worked in one try in the VM) read back with `model_matches`. |
| `probe` lists MenuBarAgent's slots, folded or not | For bug reports; a folded slot sits left of the overflow control. |
| Our buttons carry their autosave name as accessibility identifier | AppKit's frames lag more than 500 ms behind a move, so finding our items in MenuBarAgent's tree by frame failed after a move (a move reported failed while it had worked). |
| Spacer repair: a handle (length 20, a title), one Command-drag, back to a blank slot | MenuBarAgent recomputed positions while IceRust was switched off under Allow in the Menu Bar and put the spacer right of the divider. A blank item of length 0 does not move under a Command-drag. |
| An alert when our items have no frames after launch | macOS draws nothing for an app switched off under Allow in the Menu Bar, and ice-rust has no icon to click then. |

Live test in a UTM VM (macOS 27.0.1 26A434, 1920 x 1010, maintainer
approved, synthetic clicks and drags only inside the VM after an idle
check), with a test app adding 16 dummy items:

- Upgrade path (MenuBarAgent kept 0.1.6's divider at 1, right of the icon):
  the order check kept everything shown with "Command-drag the icon right of
  the dividers"; the new spacer landed directly left of the divider. After
  one Command-drag of the icon, a click on the icon hid every item except the
  icon, Control Center and the clock, with no overflow control drawn.
- Show and hide: the icon toggles; a click in the empty-looking bar shows
  (Show on click); a click on another item does nothing.
- Fresh install (IceRust's MenuBarAgent entries and defaults removed,
  MenuBarAgent restarted): seeds 1, 2, 3; shown order input menu, Spotlight,
  spacer, divider, icon, Control Center, clock; the launch hide hid
  everything else.
- Settings render: Use Ice Bar and Hide app menus off and disabled with the
  note; Menu Bar Layout shows the note in each section.

Second session, same VM after it restarted (2026-10-16 evening):

- IceRust's items were missing: MenuBarAgent logged `isAllowed: false`
  for them. After the maintainer switched IceRust on under Allow in the
  Menu Bar the items came back with the spacer right of the divider.
- Spacer repair: the first version dragged the blank spacer, which did not
  move; with a handle one drag put it back (stored order spacer 191,
  divider 165), and the launch hide used the spacer.
- Menu Bar Layout: the page showed all three sections with item images;
  dragging a tile from Always-Hidden to Hidden, from Always-Hidden to
  Visible and from Visible back to Hidden each moved the real item in one
  Command-drag (about 105 ms with the pointer hidden) and settled after two
  reads; closing Settings hid the sections again. The move from
  Always-Hidden to Visible first reported a failure because the frame
  lookup lagged; identifiers fixed that.

Third session (maintainer report with screenshots): showing the hidden
section left always-hidden items on the bar. With the always-hidden
spacer, created new at the far left and dragged next to its divider at the
first hide (one drag), showing the hidden section folded every
always-hidden item with System Settings in front; both spacers kept their
places across a relaunch. Order and spacer readings now come from
MenuBarAgent's slots. Test note: the click helper shares ice-rust's bundle
id for its Accessibility grant, so while ice-rust is a regular app
(Settings open) launching it sends ice-rust a reopen instead.

Fourth session (maintainer report: sometimes partial hiding with hidden
and always-hidden items on the left, always-hidden items flashing on the
left when hiding). The log showed spacer readings flipping between "next
to its divider" and "not" without any move: edges were compared while
MenuBarAgent animated the change. Adjacency is now by order (no slot
between), and the always-hidden pair stays grown while everything hides.
Ten clicks on the icon at 0.7 to 2.5 s intervals then toggled correctly
every time with both spacers in use, and screenshots taken during a hide
showed no always-hidden items. The report that a click on the icon
sometimes does nothing did not reproduce; the log now records each icon
click.

Hand test of 0.2.0 in the VM (2026-10-16, maintainer): everything listed
as untested in the VM works, Option-click and Show on hover and scroll
included.

Not tested yet: a notched display, several displays, a real Mac on macOS
27.

## v1.9: global hotkeys (roadmap, 2026-10-17)

The first item of the ice-rust roadmap. Ice's design, from the evidence
document (section 6): Carbon `RegisterEventHotKey`, no hotkey by default,
the actions ToggleHiddenSection, ToggleAlwaysHiddenSection, EnableIceBar,
SearchMenuBarItems and ToggleApplicationMenus, stored as
`[keyCode, modifiers]` with control 1, option 2, shift 4, command 8, a
recorder that refuses combinations without a modifier, with Shift alone or
taken by the system, and hotkeys off while a menu tracks.

| Decision | Why |
|---|---|
| The two section actions and EnableIceBar | The roadmap item; ice-rust has no search panel, and ToggleApplicationMenus can follow. |
| Carbon, not an event tap or a global key monitor | Needs no permission (Input Monitoring or Accessibility), like Ice. |
| Option and Option-Shift alone refused from macOS 15 on | Carbon no longer delivers such hotkeys from macOS 15, so they would record and never fire. |
| One JSON string under `Hotkeys` | The same shape as Ice's entries in one value; our defaults domain is separate from Ice's. |
| Hotkeys off while recording, apart from the menu-tracking pause | A recorded combination must reach the recorder, not run its action; either reason alone keeps them off. |
| EnableIceBar disabled on macOS 27 | There is no Ice Bar there yet. |

Live test in the macOS 27.0.1 VM (2026-10-17, maintainer approved,
synthetic input inside the VM only):

- ToggleHiddenSection stored as Control-Option-H: "1 hotkey(s) registered,
  0 refused"; pressing it showed the hidden section and pressing it again
  hid it, both with the modifier keys pressed as keys and with the flags
  alone. The first tries did nothing because the VM's screen was locked.
- The Hotkeys page recorded Control-Command-K for the always-hidden
  section ("2 hotkey(s) registered"); the field showed ⌃⌘K, and pressing it
  showed the always-hidden section at once.
- Option-J was refused with "macOS does not pass on Option-only shortcuts;
  add Command or Control"; Escape kept ⌃⌘K; Delete cleared it and the
  stored value lost the entry.
- Command-Space never reached the page: Spotlight opened. Enabled system
  shortcuts are taken by macOS first, so the system check only guards.
- The Use Ice Bar row shows "Not available on macOS 27 yet" and is
  disabled.

Hand test on macOS 26 (2026-10-17, maintainer, dist build): recording,
using the hotkeys from another app, the Use Ice Bar hotkey, the refusal
message and clearing all passed.

## Item names in tooltips (maintainer request 2026-10-22)

Tooltips in the Ice Bar and Menu Bar Layout showed window titles such as
"Item-0" or a UUID, the autosave names apps give their items. They now name
the owning app; [technical.md](technical.md#item-names) describes the
behavior.

| Decision | Why |
|---|---|
| Match an app's `AXExtrasMenuBar` items to windows by center, within 1 pt | The frames differ in width: Shottr's item is 24 pt wide in a 38 pt window (x 4718 against 4711) on macOS 26.7.1. The centers agree. |
| Look up only the windows on show (the Layout model's tiles, the Ice Bar's items) | A probe over all 125 status windows matched copies on other displays and stale copies to the wrong apps, such as a 5016 pt wide ice-rust copy to Shottr. |
| A helper thread, owners cached by window id, a 60 s wait before an unclaimed window is asked again | The first Accessibility contact with an app took about 20 ms (1.4 s for 79 apps); later passes about 0.1 s. One lookup at a time, so lookups do not pile up behind an app that does not answer (0.1 s messaging timeout per app). |
| Other apps asked first | Apple's items mostly keep their own names, so the names that matter arrive first. |
| Tooltips read on hover (`NSViewToolTipOwner`), not set once | A name found after the bar opened still shows. Tooltip rectangles are removed before the tiles go, since the tiles own them. |
| Apple's items keep their own name unless it is made up | "WiFi" or "Clock" says more than "Control Center"; "Item-0" does not. |
| macOS 27 from MenuBarAgent's process ids | The slots carry the pid already; no lookup is needed. |

Host check (macOS 26.7.1, a probe that loads the Layout model and runs
the lookup): 21 of 29 tiles named, every third-party item right (Discord,
Notion, LINE, Shottr, CodexBar, RunCat Neo, Telegram, Tailscale, Raycast
and others), Apple's items unchanged, "Item-0" named TextInputMenuAgent and
`com.apple.menuextra.TimeMachine` shown as "Time Machine". The last name
arrived about 1.24 s after launch. The maintainer then used a build by hand
and found the tooltips right.
