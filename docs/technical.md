# ice-rust technical reference

This page holds the detail behind the [README](../README.md): how ice-rust
behaves, its command-line tools, the defaults it stores, and the macOS 26
behavior it works around. The design contract and the findings of each test
run are in [design.md](design.md). The facts about Ice and macOS that the
design relies on are in [ice-internals-evidence.md](ice-internals-evidence.md).

ice-rust is a Rust port of Ice and a derivative of it under GPL-3.0. Its
features, UI text, settings keys and macOS mechanisms come from Ice, gathered
from Ice's source, the installed Ice binary and runtime observation (see the
README's "A Rust port of Ice"). The Rust code was written from that analysis,
plus Apple documentation and the documentation and sources of the Rust crates
it depends on, rather than translated line by line from Ice's Swift.

"The development host" below is the machine ice-rust was built and tested
on: macOS 26.3 on Apple Silicon, with four displays.

## Contents

1. [Behavior](#behavior)
2. [Code layout](#code-layout)
3. [App bundle](#app-bundle)
4. [Icon](#icon)
5. [Permissions](#permissions)
6. [Settings window](#settings-window)
7. [Moving items](#moving-items)
8. [Synthetic input without blocking](#synthetic-input-without-blocking)
9. [Probe](#probe)
10. [Render a Settings page](#render-a-settings-page)
11. [Show on click, hover and scroll (v1.5)](#show-on-click-hover-and-scroll-v15)
12. [Hide application menus (v1.6)](#hide-application-menus-v16)
13. [How hidden items are found](#how-hidden-items-are-found)
14. [Stored settings and item placement](#stored-settings-and-item-placement)
15. [Window captures on macOS 26](#window-captures-on-macos-26)
16. [Limitations and differences from Ice](#limitations-and-differences-from-ice)
17. [Verification history](#verification-history)

## Behavior

- Three status items, right to left: an icon that is always visible, the
  hidden divider directly left of it, and the always-hidden divider, which
  macOS places at the far left of the menu bar on first launch. Items between
  the two dividers form the hidden section; items left of the always-hidden
  divider form the always-hidden section, which starts empty.
- Hiding a section sets its divider length to 10000 pt. AppKit lays status
  items out from the right, so everything left of the divider moves off every
  display. Showing sets the length back to variable: the hidden divider draws
  a single chevron, the always-hidden divider a double chevron.
- Left click on the icon toggles the hidden section. Option-click toggles the
  always-hidden section; showing it shows everything, and hiding it again
  re-expands only its own divider. Right click (or Control-click) opens a
  menu: Show/Hide Hidden Items, Show/Hide Always-Hidden Items, Use Ice Bar,
  Settings..., Arrange Menu Bar Items..., and Quit ice-rust.
- Ice Bar mode (on by default): the dividers stay expanded and a click on the
  icon opens a panel under the menu bar with an image of each hidden item;
  Option-click opens it with the always-hidden items. Hovering an image
  shows the item's name (see [Item names](#item-names)). Clicking an image
  closes the panel, reveals that item's section, waits until the item is on
  screen (polling every 20 ms for up to 1 s), and clicks the real item.
- Revealed sections hide again by one of three strategies, as in Ice
  (rebuilt from [the clean-room spec](clean-room/smart-rehide/SPEC.md)).
  Smart: 250 ms after a left click outside the menu bar, everything hides
  when the active Space changed, or when the app under the pointer is the
  Dock or the active regular app. The app under the pointer comes from an
  Accessibility hit test (`AXUIElementCopyElementAtPosition` on the
  system-wide element, 0.25 s timeout), or without one from the frontmost
  normal window (layer 0) under the pointer. A click on the Dock always
  hides. For any other app, a menu, pop-up or panel of that app under the
  pointer (its frontmost window there is above layer 0) keeps the items;
  other processes' windows, such as full-display overlays, play no part.
  Timed: everything hides after the rehide interval (15 s by default) with
  the pointer outside the menu bar, no pop-up menu on screen, and no panel
  open. Focused app: 0.1 s after another app comes to the front, the
  hidden section hides, which hides everything, unless the pointer is in
  the menu bar or the new app
  is ice-rust itself or the app ice-rust hands focus back to. Clicks that
  ice-rust posts never trigger Smart, and nothing rehides while a move or
  Menu Bar Layout holds the sections.
- The Settings window has a General page, a Menu Bar Layout page with drag
  and drop, a Permissions page and an About page (see
  [Settings window](#settings-window)).
- `ice-rust probe` lists, walks and captures the sections without changing
  anything. It is the verification harness. `ice-rust render-settings` draws
  a Settings page into a PNG without showing a window.
- Running the app adds three status items. Quitting removes them, and the
  items they hid return.

## Code layout

The logic lives in pure modules with unit tests and no macOS dependencies:
`model`, `geometry`, `chain`, `sections`, `arrange`, `bar_layout`,
`bar_scroll`, `rehide`, `rehide_policy` (smart and focused-app rehide,
API in `docs/clean-room/smart-rehide/API.md`), `input_tag`, `worker`, `image_check`, `brand` (the
embedded artwork), `menu_icon` (the icon styles), `triggers` (show on
click, hover and scroll), `app_menus` (hide application menus),
`item_names` (app names for the item tooltips), and the defaults half of
`settings`. The macOS glue is under `src/platform/` and
compiles only on macOS. The binary (`src/main.rs`) parses the command line
and dispatches to the menu bar app, `probe` or `render-settings`.

```sh
cargo run              # the menu bar app (changes your menu bar while it runs)
cargo run -- probe     # read-only harness, see Probe
cargo run -- render-settings --page general --out general.png
cargo run -- --help
```

## App bundle

TCC grants attach to the responsible process. A binary started from a
terminal uses the terminal's grants. For standalone use, build a bundle:

```sh
scripts/bundle.sh      # builds target/IceRust.app; does not launch it
open target/IceRust.app
```

The bundle is `LSUIElement` (no Dock icon), has the stable identifier
`local.icerust.IceRust` (override with `BUNDLE_ID=...`), declares
`LSMinimumSystemVersion` 14.0. `scripts/bundle.sh` signs it with the
"ice-rust Release Signing" identity when the keychain has it, otherwise
ad-hoc (`SIGN_IDENTITY=-` forces ad-hoc, `SIGN_IDENTITY=<SHA-1>` picks an
identity). Releases from 0.1.5 on always use the certificate.

TCC stores the designated requirement of the build that was granted, as
`csreq` in its database, and later checks each launch against it. With the
certificate the requirement is
`identifier "local.icerust.IceRust" and certificate leaf = H"a8f202856370885dbf698604320fd846f8145bf2"`,
which every build signed with that certificate meets, so grants carry over
to rebuilds and updates. An ad-hoc requirement is `cdhash H"..."`, which
only the granted build meets. System Settings still shows a stale entry as
on; turning it off and on keeps the stored requirement, so the entry has to
be removed with the minus (-) button and granted again. The certificate is
self-signed and not trusted by the system, so `codesign` finds it by its
SHA-1 hash, not by its name.

## Icon

The artwork comes from `assets/logo-source.png`. `scripts/make-icons.py`
(Pillow and Xcode's `iconutil`) rebuilds everything under `assets/` from it:

```sh
python3 scripts/make-icons.py assets/logo-source.png assets
```

- `icon-1024.png`: the app icon artwork; `docs/images/icon.png` is a 256 px
  copy for the README.
- `AppIcon.icns`: the bundle icon (`CFBundleIconFile`), copied by
  `scripts/bundle.sh`.
- `app-icon-512.png`: embedded in the binary; the Dock icon while Settings is
  open (also under `cargo run`) and the About page image.
- `menubar-template.png` / `menubar-template-active.png`: the menu bar glyph,
  18 pt at 3x, drawn as a template image so AppKit tints it for the menu bar;
  the filled variant shows while a section is revealed or the Ice Bar is open.

The source artwork's stray specks and edge haze are not copied: the script
rebuilds the squircle on Apple's macOS icon grid (824 px body on a 1024 px
canvas) over a gradient sampled from the source.

### Icon styles (v1.4)

The Menu bar icon popup on the General page picks the icon's look. Each
style has one image while items are hidden and one while a section is
revealed or the Ice Bar is open:

| Style | Items hidden | Items shown |
|---|---|---|
| ice-rust Logo (default) | `menubar-template.png` | `menubar-template-active.png` |
| Dot | `circlebadge` | `circlebadge.fill` |
| Ellipsis | three solid dots, drawn | three outlined dots, drawn |
| Ellipsis Circle | `ellipsis.circle` | `ellipsis.circle.fill` |
| Chevron | `chevron.left` | `chevron.right` |
| Arrow | `arrow.left.circle` | `arrow.right.circle.fill` |
| Eye | `eye.slash` | `eye` |
| Door | `door.left.hand.closed` | `door.left.hand.open` |
| Sunglasses | `sunglasses.fill` | `sunglasses` |
| Snowflake | `snowflake.circle` | `snowflake.circle.fill` |
| Grid | `square.grid.2x2` | `square.grid.2x2.fill` |
| Star | `star` | `star.fill` |
| Custom Image | the chosen image | the same image |

The symbols are SF Symbols at their default size, the same size as the
divider chevrons, drawn as template images. SF Symbols has no outlined
`ellipsis`, so ice-rust draws the Ellipsis dots itself as vector template
images: three 4 pt dots 2.5 pt apart (17 x 4 pt), outlined with a 1.1 pt
line while items are shown. The two images of each pair
have the same width, so the items left of the icon do not move when the
state changes. A style whose symbols the system lacks, or Custom Image with
no image stored, shows the logo.

Choose Image... (or picking Custom Image before any image is stored) opens
an open panel limited to image files. ice-rust opens the file with
`NSImage`, so PNG, JPEG, HEIC, TIFF, PDF and SVG all work, draws it into an
RGBA bitmap that fits 24 x 18 pt at 3x, and stores that PNG in defaults, so
moving or deleting the original file later changes nothing. Match menu bar
color draws the image as a template image in the menu bar's text color. It
starts on when part of the image is clear and what is drawn is black, white
or gray, and off for a photo or a colored logo, which keep their colors. A
file that cannot be opened leaves the icon as it was and shows the reason
under Custom image.

## Permissions

| Permission | Needed for | API |
|---|---|---|
| Screen Recording | Reading other processes' window titles (to find the divider) and capturing item images | `CGPreflightScreenCaptureAccess`, `CGRequestScreenCaptureAccess` |
| Accessibility / event posting | Clicking the real item after a click in the Ice Bar, moving items from the Menu Bar Layout pane, reading where the frontmost app's menus end for Show on click and Show on hover, finding the clicked app for Smart rehide (without it, Smart falls back to the window list), and naming items by their app in tooltips (without it, tooltips show window titles) | `CGPreflightPostEventAccess`, `CGRequestPostEventAccess`, `AXIsProcessTrustedWithOptions`, `AXUIElementCopyAttributeValue`, `AXUIElementCopyElementAtPosition` |

On macOS 26 every status item window, including ice-rust's own, belongs to
Control Center, so even finding its own divider by title needs Screen
Recording. When titles are readable but none carries a divider's autosave
name, ice-rust matches the window whose bounds equal the frame of the
divider's own `NSStatusBarWindow` (issue #1). The app requests missing grants at launch. With Use Ice Bar off,
plain hide and show work without either grant; the check that the icon sits
right of the dividers then uses the stored positions. With Use Ice Bar on
(the default), a click on the icon without Screen Recording opens the Ice
Bar with a "Screen Recording permission needed" tile and reveals no section,
and the menu's Show Hidden Items does the same. Without event posting, a click in the Ice Bar reveals the section, asks for the
grant again, and shows a message tile instead of clicking the item.

## Settings window

Open it from the icon's menu with Settings... (Command-comma while the menu
is open), or with Arrange Menu Bar Items..., which opens it on the Menu Bar
Layout page. There is one window; opening it again brings it to the front.
While it is open ice-rust runs as a regular app (Dock icon, a main menu with
Settings, Quit and Command-W to close) and returns to a menu bar only app
when the window closes, giving focus back to the app that was in front
before. The selected sidebar row turns gray while the window is not key.

To come to the front over the app you were using, the window switches
ice-rust to a regular app first and activates it on the next pass of the
run loop, then orders the window front. If ice-rust is still not the active
app 250 ms later, it activates once more with the older forced activation
call.

The window has a transparent title bar over a vibrant sidebar with the
"ice-rust" title and one row per page. The selected row is highlighted and
the page title sits in the title bar area. The window is resizable down to
820 x 600 pt; the sidebar keeps its width while the pages and the layout
bars stretch.

- General: Menu bar icon and Custom image (v1.4, see
  [Icon styles](#icon-styles-v14)), Launch at login and Automatically check
  for updates (v1.3). Changes are saved and applied at once.
- Hidden Items (v1.5): Use Ice Bar, Hide app menus (v1.6, enabled while
  Use Ice Bar is off), Show on click, Show on hover, Hover delay
  (0 to 1 s in tenths), Show on scroll, Automatically rehide with its
  Strategy picker (shown while the switch is on, with a line describing
  the strategy), Rehide after (5 to 60 s, shown only for Timed), and a
  note on clicking the icon or empty menu bar space. See
  [Show on click, hover and scroll](#show-on-click-hover-and-scroll-v15).
  Changes are saved and applied at once; the event monitors follow the
  switches.
- Menu Bar Layout: "Drag to arrange your menu bar items", a tip about
  Command-dragging in the menu bar, and three bars (Visible, Hidden,
  Always-Hidden Section) filled with the logo's blue (#1C43AB) when the item
  images are mostly light, or its light tint (#E4E8F5) when they are mostly
  dark, so white menu bar text stays readable in light mode (issue #3). The
  tone comes from the captured images, else from the menu bar color sample,
  else from the window's appearance; the Ice Bar uses the same fill. Each bar shows
  the real item images right-aligned in menu bar order, with placeholder
  tiles (a dashed question mark) where a capture fails; hovering a tile
  shows its name (see [Item names](#item-names)). Every item is in its bar: a section wider than its bar
  scrolls sideways like a horizontal scroll view, with a trackpad swipe, a
  scroll wheel (vertical wheel steps scroll sideways) or the overlay
  scroller. A bar that overflows opens at its right end, like the menu bar,
  and keeps its distance from the right end across refreshes and window
  resizes. While a tile is dragged within 24 pt of a bar's left or right
  edge, that bar scrolls (up to 8 pt every 30 ms, faster closer to the
  edge) until the drag moves away, leaves the bar or ends, so items that
  are scrolled out of view can be reached as drop neighbors. The dividers
  are the bars themselves; ice-rust's icon, the Clock and Control Center
  are locked tiles that cannot be dragged but can be dropped next to. Drag a
  tile to a new place; an insertion marker shows where it will land, already
  moved left of the Clock and Control Center when the drop would land there.
  After a drop the pane shows a spinner, asks the move engine to move the
  real item, rebuilds all three bars from the real menu bar, and shows an
  inline error when the order does not match. That message stays until the
  next press. The pane also refreshes when it opens and every 3 s while it
  is visible and idle, skipping one refresh after a capture timed out; an
  image that timed out keeps the one from the previous refresh.
- Hotkeys (v1.9): one row per action (toggle the hidden section, the
  always-hidden section, Use Ice Bar) with a field that records a shortcut
  and a button that clears it. See [Hotkeys](#hotkeys-v19).
- Permissions: the state of Screen Recording and Accessibility, refreshed
  every second. Accessibility changes show within a second; a Screen
  Recording grant applies after ice-rust relaunches, as the page says. The
  first click on Grant Access asks for the permission; later clicks open the
  matching System Settings pane.
- About: version, what the app does, and the note that it is a Rust port of
  Ice, not affiliated with or endorsed by Ice.

## Hotkeys (v1.9)

Like Ice's (evidence section 6): Carbon `RegisterEventHotKey` on
`GetEventDispatcherTarget()` with the signature `'IcRs'` and the action as
id, and one `InstallEventHandler` for hotkey presses, installed with the
first hotkey (`platform::hotkeys`). Carbon hotkeys need no permission. The
handler runs the action on the next pass of the run loop: the hidden
section toggles like a click on the icon, the always-hidden section like an
Option-click, and Use Ice Bar flips like its switch (ignored on macOS 27,
with a log line). None is assigned by default.

The hotkeys are stored under the defaults key `Hotkeys` as a JSON string,
`{"ToggleHiddenSection": [keyCode, modifiers]}`, with Ice's action names
and modifier bits (control 1, option 2, shift 4, command 8). All hotkeys
are unregistered while one of our menus tracks (Ice does the same) and
while the Hotkeys page records; the two reasons are kept apart, and the
hotkeys come back when neither holds.

Recording (`platform::hotkeys_pane`): a click on a field starts it, a local
key-down monitor takes the next key and swallows it. Escape cancels, Delete
clears, a modifier key alone is ignored, and anything else goes through
`hotkeys::check_combo`: a modifier other than Shift, not Option or
Option-Shift alone from macOS 15 on (Carbon no longer delivers those), not
another action's combination, and not an enabled system shortcut from
`CopySymbolicHotKeys`. Enabled system shortcuts such as Command-Space never
reach the page, because macOS acts on them first. A combination that
`RegisterEventHotKey` refuses, for example one another app registered, is
reported under the rows. Changing pages, closing the window or the window
losing key status ends a recording. Fields show the modifier symbols and
the key's name in the current keyboard layout (`UCKeyTranslate`), or a
fixed name for keys that type no character.

## Moving items

The move engine moves one on-screen item next to another with a synthesized
Command-drag. For each move it:

1. shows every section the move touches and waits (20 ms polls, up to 1 s)
   until the item and its target are on screen on the active menu bar
   display;
2. waits until no mouse button is down and there was no input in the last
   50 ms, giving up after 5 s with "user busy";
3. hides the cursor and posts, at the HID event tap, a mouse move to the item,
   a Command mouse-down, 8 Command drag events to the drop point and a
   mouse-up there, 10 ms apart, each tagged in field 42, then a Command
   key-up so no modifier stays down (finding L4 in [design.md](design.md));
4. restores the cursor (see below) and polls the window list for up to
   500 ms until the item touches its target on the requested side, retrying
   up to 3 times with the drop point 4 pt further in the travel direction;
5. returns the sections to their previous state.

Moves queue and run one at a time; while queued moves wait, the sections the
last move showed stay shown and go back only after the last one (v1.7).
Every event is built before the first one is posted, so a mouse-down is
never posted without its mouse-up. A click on
the icon during a move changes the state the move returns to, not the
dividers the move needs. A click in the Ice Bar during a move waits until
the move has ended, then clicks the item where it is at that moment.

### Menu Bar Layout drops (v1.7)

A drop changes the bars at once (`arrange::apply_drop`, the drop rule's
order) and the real move runs in the background; further drops queue, and
the bars are not dimmed while moves run. After the last move the page reads
only the order (`layout_pane::load_order`, no captures) every 150 ms until
two reads agree and contain every item the bars show, for at most 3 s, then
loads the full snapshot once and says whether the menu bar ended up in the
dropped order (`orders_agree`). A read that misses items keeps the bars as
dropped. The periodic refresh shows a changed order only when a second read
150 ms later (1 s when items are missing) agrees. The log says how many
reads settling took, and for each move how long each phase took and how
long the pointer stayed hidden.

## Synthetic input without blocking

`CGEvent::post` only queues events, so warping the cursor back right after
posting leaves it on the item (finding G4 in [design.md](design.md)). Each batch of
events (a click or a drag) carries unique tags in field 42
(`kCGEventSourceUserData`). A listen-only, tail-append event tap at the
session level watches for the batch's last tag; the cursor goes back when the
tap sees it, or after a 300 ms fallback timer when it does not. The tap, the
timers and the callbacks all run on the main run loop. The timers fire in
the common run loop modes, so an open menu or a slider drag does not hold a
posted mouse-down or the hidden cursor, and a deadline posts whatever is
still queued once the whole batch is overdue. One batch runs at a time; a
second request is refused as busy and retried by its caller. Quitting posts
the rest of a batch in flight and restores the cursor first.

Like Ice, the posting source leaves local mouse, keyboard and system events
live during suppression and sets the local events suppression interval to
0, and a combined-session source with the same settings lives for the whole
run, so the user's own mouse is not ignored after a move (v1.7). After
warping the cursor back, `CGAssociateMouseAndMouseCursorPosition(true)`
ends the short pause in which the window server ignores the hardware mouse
after a warp.

A click-through click is one left mouse-down and two left mouse-ups at the
item's center from an HID-state event source, posted at the session event
tap, with fields 91 and 92 set to the item's window id and field 1 (click
state) set to 1 on the down and 0 on the ups.

## Probe

```
ice-rust probe [--divider <autosave-title>] [--out <dir>] [--json]
```

- `--divider` is the autosave title of the divider window to start from.
  The default is `IceRust.ControlItem.Hidden`. Pass `Ice.ControlItem.Hidden`
  to inspect a running Ice.
- `--out` writes one PNG per captured hidden item, named
  `<index>-<window id>.png`, with the index zero-padded to two digits (for
  example `00-312.png`). A row whose capture returned no image reports an
  error instead.
- `--json` prints one JSON object instead of text.

It prints the permission states, the displays, the active menu bar display
and its menu bar height, the divider window, the always-hidden divider (the
title with `.Hidden` replaced by `.AlwaysHidden`), the hidden, always-hidden
and visible chains (left to right: id, title, x, width, onscreen), and one
capture row per hidden item with the capture method and whether the image is
fully transparent. The exit code is 0 when the divider was found, 1 when it
was not, and 2 for bad arguments.

The capture runs on a worker thread and the probe waits at most 1 s for it.
When it does not finish, the probe prints "capture timed out" under the
captures line, marks each row with method "timed out", writes no PNGs, and
still exits normally. The same line appears when the worker reached its 1 s
deadline before capturing some items on their own; only those rows are
marked. A worker that could not start or panicked shows "capture worker
failed" instead. In JSON the same shows as `"capture_error"`.

The probe is read-only. It calls only the preflight permission checks (no
prompts), `CGWindowListCopyWindowInfo`, the display list functions and
`CGWindowListCreateImageFromArray`. It posts no events, creates no status
items, does not start an `NSApplication`, and writes no defaults. The only
files it writes are the PNGs under `--out`.

Output against a running Ice on the development host (macOS 26.3, four
displays, display 3 at x -1920 active). Rows are elided with `...`, and a
third-party item title is replaced with a placeholder:

```
divider  Ice.ControlItem.Hidden  id 9121 x -5428 y 0 w 5016 h 30 onscreen false
icon     Ice.ControlItem.Visible  id 9117 x -412 y 0 w 33 h 30 onscreen true
always   Ice.ControlItem.AlwaysHidden  id 9125 x -10548 y 0 w 5016 h 30 onscreen false
hidden chain (3, left to right)
  #   id      title                                        x  width  onscreen
  0   312     Item-0                                   -5532     32  false
  1   52      AirDrop                                  -5500     34  false
  2   68      Item-0                                   -5466     38  false
always-hidden chain (15, left to right)
  #   id      title                                        x  width  onscreen
  0   192     <third-party title>                     -11085     34  false
...
  14  164     Item-0                                  -10580     32  false
visible chain (8, left to right)
  #   id      title                                        x  width  onscreen
  0   9117    Ice.ControlItem.Visible                   -412     33  true
  1   446     Item-0                                    -379     38  true
...
  7   54      Clock                                     -137    137  true
captures -> <dir>
  0   id 312     composite   32x30   transparent no  00-312.png
  1   id 52      composite   34x30   transparent no  01-52.png
  2   id 68      composite   38x30   transparent no  02-68.png
summary  3 hidden, 15 always-hidden, 8 visible, 0 transparent
```

An independent listing of the same windows (a separate Swift script using
`CGWindowListCopyWindowInfo`) found exactly windows 312, 52 and 68 between
`Ice.ControlItem.AlwaysHidden`'s right edge and `Ice.ControlItem.Hidden`'s
left edge, in the same order.

## Render a Settings page

```
ice-rust render-settings --page <general|hidden-items|layout|hotkeys|permissions|about> --out <png>
                         [--width W --height H] [--divider-prefix <prefix>]
                         [--default-settings]
```

Builds the Settings window offscreen and writes the chosen page, title bar
included, as a PNG (default 820 x 600 pt, rendered at 1x). The bitmap is
converted from the display's color space to sRGB before encoding, so the PNG
carries no display ICC profile. The window is built at 820 x 600 with the
page loaded, then resized to `--width` and `--height` (200 to 4000) the way
a live resize does, so a larger size shows the stretched layout bars. It
never orders a window on screen, creates no status items, posts no events,
writes no defaults and moves nothing. The General page shows the stored
settings, or the defaults with `--default-settings`. The layout page reads
the live window list and captures item images like the probe: it uses the
dividers titled `<prefix>Hidden` and `<prefix>AlwaysHidden`, trying
`IceRust.ControlItem.` and then Ice's `Ice.ControlItem.` when no prefix is
given. It prints which dividers it used, the tile and image counts per
section, any capture timeout, the sampled bar color, and each bar's scroll
offset next to the offset that shows its right end.

The README screenshots were made this way:

```sh
cargo build --release
./target/release/ice-rust render-settings --page about --out docs/images/about.png
./target/release/ice-rust render-settings --page general --default-settings --out docs/images/general.png
./target/release/ice-rust render-settings --page hidden-items --default-settings --out docs/images/hidden-items.png
```

## Show on click, hover and scroll (v1.5)

Like Ice, ice-rust shows the hidden items from the menu bar itself. Each
switch on the Hidden Items page installs a pair of `NSEvent` monitors with
the same mask, a global one for events sent to other apps and a local one
for events sent to ice-rust (Ice's "universal" monitor). Our own synthetic
clicks and drags (the move engine, click-through) are ignored.

Empty menu bar space (`triggers::is_empty_space`) is inside the menu bar of
the display with the ice-rust icon (`geometry::menu_bar_display`), right of
the frontmost app's menus, outside every on-screen status item window (an
expanded divider is not on screen) and outside the notch plus 1 pt. Where
the app menus end comes from the Accessibility API: the `AXMenuBar` of the
frontmost app, the frame of its last child (`platform::menu_bar_space`),
with a 0.1 s messaging timeout so a hung app cannot stall the main thread
for long. The last menu only takes four requests; reading every menu of a
browser took 79 ms on the development host, the last one 25 ms. Menus on
another display, a failed request (no Accessibility) or ice-rust itself in
front (its own menus cannot be read from its main thread) leave nothing
empty.

| Trigger | Monitor | Behavior |
|---|---|---|
| Show on click (on) | left and right mouse down | A left click in empty space acts like a click on the icon 50 ms later: plain shows or hides the hidden section, Option the always-hidden section; other modifiers do nothing. In Ice Bar mode it opens or closes the Ice Bar. Any click in the menu bar also takes hover-shown items over. |
| Show on scroll (on) | scroll wheel | `(dx + dy) / 2`, signed by `isDirectionInvertedFromDevice` so that down or right is positive with natural scrolling on or off, and with wheel lines counted as 10 points, is summed over a gesture (events less than 300 ms apart). The first time the sum reaches +10 the hidden items show, at -10 they hide; the rest of the gesture does nothing. Only a gesture that starts over the menu bar counts; empty space is not required. |
| Show on hover (off) | mouse moved | On each move a cheap test (the cached menu bar rectangle, re-read at most every 2 s) feeds `triggers::Hover`. With every section hidden and the pointer in the menu bar, a check runs after the hover delay: if the pointer is in empty space and no menu is open, the hidden items show. Items that hovering showed hide again, after the same delay, once the pointer is outside the menu bar and more than 10 pt from the Ice Bar. Anything else that shows or hides items, or a click in the menu bar, takes them over, and leaving no longer hides them. |

Hover checks reuse the app menus' end for up to 2 s per frontmost app; a
click reads it again.

## Hide application menus (v1.6)

Like Ice's HideApplicationMenus. When a section is shown in the menu bar
and the items do not fit next to the frontmost app's menus, macOS draws
only what fits. With Hide app menus on and Use Ice Bar off, ice-rust makes
the room:

1. After every menu bar state change, the setting changes and Settings
   closing, ice-rust checks 200 ms and 600 ms later, once the items have
   moved. Ice's test: the leftmost item on the menu bar with the ice-rust
   icon sits at or left of where the frontmost app's menus end
   (`app_menus::items_reach_menus`, the same Accessibility read as Show on
   click). The items are the visible items, the shown sections and their
   collapsed dividers at their laid-out positions from the full window
   list: macOS 26 takes the items that collide with the app menus off
   screen, so on-screen windows never reach the menus (L14 in
   [design.md](design.md)). On a display with a notch the menus must also
   continue right of the notch (`app_menus::menus_take_item_room`);
   menus that end left of it do not take the items' room.
2. If so, ice-rust remembers the app in front, hides every top-level item
   of its own main menu, switches to a regular app and, on the next pass
   of the run loop, asks to be activated from the app in front
   (`activateFromApplication:options:`, as Ice does). Since macOS 14
   activation is cooperative, and a plain or forced request from a
   background app is refused. The menu bar then shows only the Apple menu,
   and macOS lays the items out in the freed space. ice-rust's icon shows
   in the Dock meanwhile, as Ice's does.
3. It keeps the menus hidden while any section stays shown
   (`app_menus::menu_action`). Once every section hides, the setting goes
   off or Use Ice Bar comes on, its menus come back, it returns to an
   accessory app and gives focus back to the remembered app.
4. Switching to another app ends the hiding at once and leaves focus there.
   Opening Settings ends it too; Settings then gives focus back to the
   remembered app when it closes.

When an app's menus already cover the ice-rust icon, show the items with
Show on scroll (or Show on click in empty space, if any is left); the
menus then hide and the icon is drawn again.

While the menus are hidden, Show on click and Show on hover take the app
menus to end 60 pt right of the menu bar's left edge, past the Apple menu
(44 pt measured), since ice-rust cannot read its own menus from its main
thread.

## How hidden items are found

1. List every window with `CGWindowListCopyWindowInfo` and keep layer 25
   (`kCGStatusWindowLevel`).
2. Find the divider by its title. Each status item has one window per
   display; only the copy on the active menu bar display carries the autosave
   name, so the walk starts on that menu bar.
3. Walk left from the divider's left edge: the next item is the window on the
   same menu bar (same y and height within 1 pt) whose right edge touches the
   current edge within 1 pt, closest first, then lowest id. Stop at a gap, at
   the always-hidden divider, or at any window 1000 pt or wider (another
   manager's expanded divider) among those touching the edge, even when an
   item touches it too. Left of an expanded divider every item is off screen,
   so on-screen windows (other displays' copies that share the y) are skipped
   there. Where the section is shown (left of a collapsed divider, and the
   visible chain right of the hidden divider) on-screen windows win over
   off-screen copies at the same edge. The visible chain is the same walk to
   the right. The walk stops after 256 steps.
4. Capture all hidden items in one `CGWindowListCreateImageFromArray` call
   (option 9: ignore framing, best resolution), crop each item by its bounds
   relative to the union, and capture any item whose crop is fully transparent
   on its own. The panel draws a placeholder tile with the item's title when
   no usable image remains.

The active menu bar display is the display containing the icon window's
center, or failing that the divider's right edge. Without Screen Recording
the icon's position comes from the frame of its AppKit button window, and
as a last resort the display under the pointer decides. When no display is
"active" in the CoreGraphics sense (for example, all displays asleep), the
online display list is used, skipping mirrors.

## Item names

The tooltips of the Ice Bar images and the Menu Bar Layout tiles name each
item by the app that owns it. On macOS 26 every status item window belongs
to Control Center and is titled with the autosave name its app chose, often
"Item-0" or a UUID, so the owner comes from Accessibility instead: each
running app with a user interface lists its items under
`AXExtrasMenuBar`, and the item whose frame has the same center as a
window, within 1 pt, is that window's item. Only the windows the pane or
the Ice Bar shows are looked up; copies on other displays and stale copies
could match the wrong app.

The lookup runs on a helper thread when the Ice Bar opens or the pane
loads, for the windows without an owner yet, asking other apps before
Apple's and stopping once every window is claimed. A window no app claimed
is looked up again after 60 s at the earliest. ice-rust's own process is
never asked (its items have names). The tooltip text is read when the
pointer rests on a tile, so a name found after the bar opened still shows.
On the development host (macOS 26.7.1, 79 apps) the first lookup after
launch took about 1.2 to 1.4 s, later ones about 0.1 s, and the main thread
spent about 10 ms on it.

On macOS 27 MenuBarAgent reports each item's process (see
[The macOS 27 menu bar](#the-macos-27-menu-bar-v18)), so the app name comes
from `NSRunningApplication` without a lookup. That path is not tested live
yet.

Other apps' items show the app's name. Apple's items keep their own name
(WiFi, Battery, Clock), which says more than the app's (Control Center),
unless it is a made-up "Item-N"; then the app's name shows (an input menu
"Item-0" became TextInputMenuAgent). An identifier such as
`com.apple.menuextra.TimeMachine` is spelled out as "Time Machine". Without
Accessibility, or before the lookup ends, a tooltip shows the window title.

## Stored settings and item placement

Stored in the process's standard user defaults:

| Key | Default | Meaning |
|---|---|---|
| `UseIceBar` | true | Toggle opens the Ice Bar instead of revealing the section |
| `AutoRehide` | true | Hide a revealed section after the idle interval |
| `RehideInterval` | 15 | Idle seconds before hiding (0 to 3600 stored; the Settings window offers 5 to 60) |
| `RehideStrategy` | 0 | 0 Smart, 1 Timed, 2 Focused app; another value means Smart. While absent, an install with `AutoRehide` or `RehideInterval` stored uses Timed |
| `AlwaysHiddenPlacementV2` | absent | Set once the v1.2 always-hidden placement migration has run (see below) |
| `AutomaticallyCheckForUpdates` | true | Daily update checks (v1.3) |
| `IconStyle` | `ice` | Icon style (v1.4): `ice`, `dot`, `ellipsis`, `ellipsis-circle`, `chevron`, `arrow`, `eye`, `door`, `sunglasses`, `snowflake`, `grid`, `star` or `custom`; an unknown value means `ice` |
| `TintCustomIcon` | true | Draw the custom image in the menu bar's text color |
| `ShowOnClick` | true | A click in empty menu bar space shows or hides items (v1.5) |
| `ShowOnHover` | false | The pointer resting in empty menu bar space shows items |
| `ShowOnHoverDelay` | 0.2 | Seconds the pointer rests before hovering acts (0 to 1; another value means 0.2) |
| `ShowOnScroll` | true | Scrolling or swiping over the menu bar shows or hides items |
| `HideApplicationMenus` | true | Hide the frontmost app's menus while shown items reach them (v1.6; Use Ice Bar off) |
| `CustomIconImage` | absent | The custom image as PNG data, fitted to 24 x 18 pt at 3x |
| `LastUpdateCheck` | absent | Seconds since 1970 of the last successful update check |
| `SkippedUpdateVersion` | absent | Version that "Skip This Version" set; automatic checks do not offer it |
| `LastLaunchedVersion` | absent | Version of the last launch; a change with a missing permission shows the re-grant notice once |

Before creating its status items the app writes the AppKit keys
`NSStatusItem Preferred Position <name>` (0 for the icon, 1 for the hidden
divider), `NSStatusItem Visible <name>` for all three items, and on macOS 26
and later `NSStatusItem VisibleCC <name>` for all three, only when they are
absent. The names are `IceRust.ControlItem.Visible`,
`IceRust.ControlItem.Hidden` and `IceRust.ControlItem.AlwaysHidden`. Like
Ice, it writes no position for the always-hidden divider: AppKit places a
status item without a stored position, which on the development host is
expected at the far left, so the items already in the menu bar stay in the
hidden section and the always-hidden section starts empty. If the positions
as they will be after seeding (stored values, and the seed for each missing
key) break the order icon, hidden divider, always-hidden divider (right to
left), all three position keys are deleted first and the icon and hidden
divider seeds return. A missing always-hidden position is accepted.

v1.1 seeded the always-hidden divider at 2, directly left of the hidden
divider, which put every existing item in the always-hidden section. AppKit
rewrites the positions as distances when the app quits, so that seed cannot
be recognized by its value. On the first launch without
`AlwaysHiddenPlacementV2`, the app therefore removes the stored always-hidden
position once, before it creates the items, and then sets the flag. Later
launches keep whatever position AppKit stored, including a Command-drag.

About 500 ms after launch, while every section is still shown, the app
checks the live window positions and prints a warning on stderr when the
always-hidden divider is not left of every other status item on its
display. It does not move the divider.

The icon must stay right of both dividers, or expanding a divider would push
it off screen with the sections. The items start with every section shown,
and every change of the dividers (hide, show, Option-click, rehide, and the
move engine's show and restore) first checks the order (live window
positions when titles are readable, otherwise the stored positions). A state
that would expand a divider right of the icon is refused: the sections stay
shown and a message asks to Command-drag the icon back. The first hide runs
500 ms after launch through the same check. An always-hidden divider right
of the hidden divider is reported on stderr; hiding still proceeds.

## Launch at login and updates (v1.3)

Launch at login uses `SMAppService.mainApp` (`src/platform/login_item.rs`;
status mapping and notes in `src/login_item.rs`). The switch is disabled
when the binary does not run from an app bundle, and the General page
re-reads the status every second while it is visible, so an approval in
System Settings shows up without reopening the window.

Updates are split the same way: `src/update.rs` holds the pure rules
(versions, the GitHub feed, scheduling, Ed25519 verification, install
checks, swap with rollback, the relaunch helper's arguments) with their
tests; `src/platform/installer.rs` runs the blocking steps (curl, ditto,
plutil, codesign, renames) by absolute path with argument vectors; and
`src/platform/updater.rs` schedules checks, shows the prompts and drives the
installer on worker threads, draining results on the main thread.

Install order: download zip and `.sig` into a private temporary folder;
read the zip into memory (50 MB limit) and verify the signature over those
bytes; refuse an installed version that is equal or newer; write the same
bytes into a staging folder next to the bundle (`.IceRust.update.XXXXXX`,
same volume) and extract them there with `ditto`; require exactly one
`IceRust.app` that is a real folder, has bundle identifier
`local.icerust.IceRust`, the offered version and a valid code signature;
remove a stale `IceRust.app.previous`; rename the bundle to
`IceRust.app.previous` and the new bundle into place, renaming back if the
second step fails; start `/bin/sh -c` with a constant script that waits for
the old process to exit and runs `open` on the bundle (pid and path are
positional arguments); terminate. The next launch removes the previous
bundle and any stale staging folder.

Debug builds accept two test overrides that release builds ignore:
`ICE_RUST_UPDATE_FEED=file:///...` for a local feed and
`ICE_RUST_UPDATE_TEST_KEY=<base64>` for a test public key. Release builds
take the feed from `https://` URLs only and trust only `UPDATE_PUBLIC_KEY`.

## Window captures on macOS 26

On macOS 26 `CGWindowListCreateImageFromArray` goes through a ScreenCaptureKit
proxy, and a second process running the same executable (for example the
probe while the menu bar app runs) blocked forever inside it during testing
(finding G5). Every capture therefore runs on a worker thread and the caller
waits at most 1 s. On a timeout the Ice Bar and the Menu Bar Layout pane show
placeholder tiles, and the probe reports "capture timed out". The stuck
thread is left to finish on its own; until it does, further captures fail at
once instead of starting more threads. The worker starts no single-item
fallback captures after the 1 s deadline, so it ends soon after the caller
stops waiting unless one call itself hangs. Running the probe or
render-settings from a copy of the binary at another path avoids the hang.

In `render-settings` on the development host, the hidden and always-hidden
items (off screen) captured within about 0.7 s, while the visible items and
the menu bar color sample (on screen) timed out. The layout pane therefore
captures the off-screen sections first. Finding G6 in [design.md](design.md) traced
those timeouts to the displays being asleep: with every display asleep, a
capture of an on-screen window takes about 1.0 s and returns a fully
transparent image, while off-screen windows still capture in about 0.04 s.
With the displays awake (P6), on-screen captures took about 0.06 s and the
layout render 0.33 s with all 28 tiles.

`CGWindowListCreateImageFromArray` is deprecated, and its Swift initializer
is obsoleted in the macOS 15 SDK. The C function is still exported and
works on macOS 26.3. ice-rust calls it through `objc2-core-graphics` with
`#[allow(deprecated)]`. Ice keeps the same API because, per its source
comment, ScreenCaptureKit cannot produce composite images of off-screen
menu bar items.

## The macOS 27 menu bar (v1.8)

From macOS 27 `MenuBarAgent` draws every status item as a slot of one
window per display (`crate::agent_bar`, `platform/app/agent.rs`, research in
[macos27.md](macos27.md)). `status::os_major() >= 27` switches:

- **Items.** Two spacers, untitled at length 0: `IceRust.ControlItem.Spacer`
  created after the hidden divider and
  `IceRust.ControlItem.AlwaysHiddenSpacer` after the always-hidden divider
  (`agent_bar::Spacer`). Seeds (`settings::seed_positions`): icon 1, hidden
  divider 2, hidden spacer = stored hidden position + 1, always-hidden
  spacer = stored always-hidden position + 1 or none.
- **Lengths** (`agent_lengths`): all hidden puts both dividers and each
  spacer that sits directly left of its divider at `max_length`
  (floor(narrowest display / 2) - 16 - 2); hidden shown does that for the
  always-hidden pair only; collapsed dividers show their chevrons and the
  spacers go back to 0. Spacer adjacency is by order in MenuBarAgent's
  slots (`spacer_next_to_divider`), not by edges. The macOS 27 log lines
  carry seconds since launch and note each icon click.
  MenuBarAgent folds whatever no longer fits into its overflow control.
- **Order.** `read_agent_layout`, at least 400 ms after the last change,
  reads MenuBarAgent's slots (`read_from_slots`: our items by identifier,
  expanded ones where they are, folded ones absent) and keeps the last
  reading for anything absent; without Accessibility it falls back to
  AppKit frames that show collapsed items (width up to 100 pt, the spacer
  up to 17 pt), hidden spacer only. A read follows 500 ms after each show.
  A divider right of the icon keeps everything shown, as before macOS 27.
- **Spacer repair** (`start_spacer_repair`): when a spacer is read away
  from its divider before a hide, it gets a handle (`set_spacer_handle`,
  length 20 and a title), is Command-dragged to `drop_point` left of the
  divider (frames from MenuBarAgent), goes back to length 0, and the hide
  runs once a read (every 250 ms, up to 8) finds it next to the divider. At
  most twice a run per spacer.
- **Not in the menu bar.** With no frames for our items the hide polls
  every second, and after the second try an alert points to Allow in the
  Menu Bar (System Settings > Menu Bar).
- **Identifiers.** Each button's accessibility identifier is its autosave
  name, which MenuBarAgent's tree shows as the slot's `AXIdentifier`;
  `agent_layout` finds our items by it, by frame only as a fallback.
- **Empty space.** `menu_bar_space::agent_slots` reads the slots of
  MenuBarAgent's window on the menu bar (Accessibility, 0.1 s timeout); our
  expanded items count as empty. The local click monitor ignores clicks on
  our own item windows, which MenuBarAgent forwards after the global monitor
  has seen them.
- **Menu Bar Layout** (`platform/app/agent_layout.rs`,
  `layout_pane::read_agent`): the bars come from MenuBarAgent's slots on
  the icon's display, split by our dividers' frames (`agent_layout`);
  ids are hashes of the item's process, name or identifier and ordinal
  (`slot_ids`), stable across reads. Apple's items
  (`com.apple.menuextra.*`) are locked. Images are cropped from one capture
  of MenuBarAgent's menu bar window, else the Window Server "Menubar"
  window, else the screen, the first crop that is not fully transparent.
  Loading the page shows every section (folded items have no position) and
  reloads after 500 ms; Settings closing restores the earlier state, and
  nothing rehides meanwhile. A move is one Command-drag to
  `arrange::drop_point` next to the target (the hidden section's end is
  left of the spacer), read back after 500 ms with `model_matches`, up to
  `MOVE_RETRIES` drops; moves queue one at a time.
- **Off:** the Ice Bar (sections show in the menu bar) and Hide app menus;
  their Settings controls say "Not available on macOS 27 yet".
- **Probe.** `agent_slots` in the JSON (x, y, width, height, pid, name,
  overflow_control, folded) and a slot table in the text report.

## Limitations and differences from Ice

- Out of scope: global hotkeys, menu bar appearance and search. The
  Settings window has no Hotkeys, Appearance or Updates pages.
- Smart and focused-app rehide follow Ice with the changes in the spec's
  Part B: Smart finds the clicked app with an Accessibility hit test and
  falls back to the window list, never reading window titles, so Screen
  Recording is not needed; Focused app needs Automatically rehide; and
  the menu bar test uses the display under the pointer. Two earlier
  versions of the clicked-app rule failed in the macOS 27 VM because of
  full-display overlay windows (spec D10 results); the current rule, which
  looks only at the clicked app's own windows, has not been run in the VM
  yet.
- Show on click and Show on hover need Accessibility to find where the app
  menus end, act only on the menu bar of the display with the ice-rust icon,
  and do nothing while ice-rust itself is in front. Show on scroll is
  signed by the gesture's physical direction and sums each gesture; Ice uses
  raw per-event deltas. A Control-click in empty space does nothing; Ice
  opens its secondary context menu.
- The move engine moves only on-screen items: it shows the sections a move
  touches first. Ice moves off-screen items through a private event relay.
  The move timings and drop offsets are starting values; live runs moved
  items correctly (P4, P9 in [design.md](design.md)), but the values have not been
  tuned.
- Drops right of the Clock or Control Center land left of them, since those
  items cannot move.
- If the always-hidden divider ends up right of the hidden divider, ice-rust
  reports it and leaves it; Ice moves it back with synthesized events.
- Click-through reveals the item's whole section for a moment (for an
  always-hidden item, everything), then clicks the item. Ice instead moves
  the single item into the visible section with synthesized Command-drag
  events.
- If the revealed section does not fit next to the app menus or the notch,
  macOS draws only what fits. An item that never reaches the screen is not
  clicked; the app gives up after 1 s. A move that times out the same way
  reports "not enough room" with the width its sections need (from the
  layout read when the move starts, collapsed dividers counted as 30 pt
  when expanded) and the width free on the active menu bar (right of the
  notch, or the whole display without one). With Use Ice Bar off, Hide app menus
  frees the room the app menus take (see
  [Hide application menus](#hide-application-menus-v16)); nothing frees the
  notch.
- When the hidden items are wider than the display, the Ice Bar keeps the
  items nearest the divider and shows a "+N" tile, which ignores clicks, in
  place of the rest. Those items are reachable only with Ice Bar mode off.
  The layout bars scroll instead.
- Discovery uses the public window list and geometry only. Ice uses private
  CoreGraphics calls for the menu bar window list, spaces, and the active menu
  bar display, and an XPC helper with Accessibility to attribute windows to
  apps. Copies of items on other displays can share the menu bar's y and
  overlap the hidden section's x range; on the development host they sit
  within 1 pt of real item edges. Left of an expanded divider the walk skips
  them because they are on screen. Where the section is shown, on-screen
  windows win a tie. A divider-wide window touching the walk's edge ends the
  walk even when an item touches it too: a copy of another display's
  divider at a real item's edge would cut the chain short there, which the
  probe would show.
- A state that would expand a divider sitting right of the icon is
  refused. A drop of the icon onto an expanded, invisible divider while the
  sections are hidden is not detected while the app runs; relaunching resets
  the order if AppKit saved it.
- The Menu Bar Layout pane reads the menu bar and waits for its captures on
  the main thread, up to 1 s per capture, when it opens and on each refresh.
- The shown dividers draw chevrons (single for hidden, double for
  always-hidden). Ice's default collapses them to zero width through an
  AppKit layout constraint.
- `render-settings` draws at 1x, the sidebar's vibrancy renders as a flat
  fill offscreen, and a switch that is on draws a gray track.
- The Settings window does not remember its size; it opens at 820 x 600 pt
  each time.
- `UseIceBar` defaults to true here; Ice defaults it to false.
- On macOS 26.3 the expanded divider measures 5016 pt, not the requested
  10000 pt. Items still leave displays up to about 5000 pt wide.
- Many per-display copies of status items capture as fully transparent on
  macOS 26.3: 77 of 113 status windows in a single-window pass on the
  development host (displays asleep at the time). The copies on the active
  menu bar, which the chain walk selects, captured correctly: 18 of 18
  across two probes.
- Memory ([design.md](design.md), L7 and L10): since 0.1.2 the Settings
  window's content is detached when it closes, because AppKit keeps a
  closed window until a later event, and every timer callback runs in its
  own autorelease pool (`platform/timer.rs`), because timers fire inside
  the pool NSApplication drains only after the next event. About 15 to
  20 MB of the footprint from launch on is AppKit's shadow images for the
  two expanded dividers.
  After Settings has been open the footprint stays a few MB higher (freed
  allocator pages, AppKit caches); repeated cycles do not add to it.
- Open findings from the live runs: on-screen captures are not skipped at
  once while the displays sleep (G6). L5 (Escape and the icon menu) did
  not reproduce in 0.1.1 (P12).

## Verification history

On macOS 26.3 (Apple Silicon, four displays) against a running Ice:

- `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`.
- `ice-rust probe --divider Ice.ControlItem.Hidden` matched an independent
  window listing, and the PNGs show the real icons.
- `ice-rust render-settings` rendered all four pages; the layout page showed
  Ice's three live sections with captured item images.
- `scripts/bundle.sh` builds and signs the bundle.

A first live run of the v1 menu bar app (with Ice quit) confirmed launch,
divider expansion, the Ice Bar, click-through, Quit and position persistence,
and found G3 to G5 (see [design.md](design.md)).

A live run of v1.1 (Ice quit, with the maintainer's approval) passed P1 to
P6 in [design.md](design.md): the three status items and their stored positions, the
Ice Bar for both sections, the menu, the Settings window with live images in
all three bars, a drag-and-drop move and its reverse, and the cursor
returning after a move and after a click-through. It found L1 to L6; L4
(Command left down after a move) was fixed and verified in that run.

v1.2 addresses L1 to L3. It was first verified offscreen only: unit tests
for the pure parts (strip layout and initial offset, insertion with a scroll
offset, auto-scroll steps and clamping, the placement migration decision,
the order guard without an always-hidden position, the leftmost check), the
read-only probe, and layout renders at 820 and 1200 pt wide. Against Ice's
live sections the Visible bar (12 items) and the Hidden bar (16 items)
overflow at 820 pt and open at their right end; at 1200 pt both stretch and
every item fits.

A live run of v1.2 (Ice quit, with the maintainer's approval) then passed
P7 to P10 in [design.md](design.md): the always-hidden placement after the migration,
the Settings window coming to the front, a horizontal scroll in an
overflowing bar, a drag from the scrolled part of a bar, drag auto-scroll at
a bar's edge, drops that restored the original order, and clear modifier
state after every move. That run did not record live window resizing,
tooltips in a scrolled bar, or which scroll input device was used.
