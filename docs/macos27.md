# macOS 27 (Golden Gate) research

Working notes for issue #2 ("Does not work under macOS 27.0.1"). macOS 27
removed the mechanism every menu bar manager relied on, ice-rust and Ice
included. Apple published no documentation or API for the change; the
findings below come from other projects' measurements and from our own
macOS 27.0.1 VM. Status: approach A implemented in v1.8 (see Design) and
tested in the VM; not yet on a real Mac.

## What changed

| Change | Evidence |
|---|---|
| The menu bar is one window per display, owned by `MenuBarAgent` (bundle `com.apple.MenuBarAgent`). Items are scenes inside it, not windows. `CGWindowListCopyWindowInfo` returns no layer-25 windows. | Our VM: `probe` sees 0 status windows (1 when IceRust runs: its own 0x0 window). Reporter's probe on 27.0.1: `"status_windows": 1`. IceMelt #46, Stash README. |
| A status item whose window reaches half the display width is dropped from the layout instead of clamped. The 10000 pt divider is not in the bar. | Our VM: IceRust's AX extras list both dividers at 5002 pt, MenuBarAgent has no slot for them. Stash README: 848 pt hides, 849 pt does not on a 1728 pt display (848 + 16 pt chrome = half). Hidden Bar #403: about 939 pt on 1920. |
| `MenuBarAgent` filters some items: `Filtering out unsupported status item: com.jordanbaird.Ice` (34 times a minute). | Stash README, MenuBarAgent log. |
| Items that do not fit fold behind a system overflow control (`«` or `•••`) at the leading edge of the status area. Users cannot choose what folds. | IceMelt #46, Brow and Publicspace blogs. |
| `MenuBarAgent` reorders items on its own; stored `Preferred Position` values are not reliable. Command-drag still works. A new item's preferred position is honored roughly, measured from the trailing end. | Ice #995, IceMelt #47. |
| Accessibility exposes every item with its real frame (see the dump below). | Our VM, IceMelt #46, Hidden Bar #403. |
| Each display has its own copy of the items. | IceMelt #46, reporter's notes. |

### Accessibility tree in our VM (macOS 27.0.1, 26A434, one 1920 x 1010 display)

```
AXApplication MenuBarAgent (pid 419)
  AXWindow sub=AXDialog (0,0 1920x30)            one window for the bar
    <slot> (1753,0 26x30)
      AXGroup sub=AXHostingView
        AXMenuBarItem sub=AXMenuExtra id=com.apple.menuextra.controlcenter
    <slot> (1795,0 105x30)
      ... AXMenuExtra id=com.apple.menuextra.clock
    <slot> (1714,0 31x30)
      AXButton title=Spotlight (pid 464, the owning app)
    <slot> (1670,0 44x30)
      AXButton (pid 646, TextInputMenuAgent)
    <slot> (1636,0 34x30)
      AXButton desc=ice-rust (pid 937, IceRust)
  AXMenuBar desc=Menu Extras                    Apple's own extras
IceRust (pid 937) AXExtrasMenuBar:
  AXMenuExtra desc=ice-rust (1635,3 36x24)
  AXMenuExtra (1726,3 5002x24)                   hidden divider, not laid out
  AXMenuExtra (1584,3 5002x24)                   always-hidden divider, not laid out
```

Slots have an unknown role and a frame; the slot's child belongs to the
process that created the item. Apple items carry stable `AXIdentifier`s
(`com.apple.menuextra.*`).

### Experiment: a divider below the cap (VM, 2026-10-16)

A test app (BarProbe) adds one divider status item and N dummy items
created after it, so the dummies sit left of the divider like hidden items.
The divider's length is changed at runtime; MenuBarAgent's Accessibility
tree and a screenshot are read after each change. The visible items right
of it are the system items (Text Input, Spotlight, Control Center, clock).

| Divider length | Window width | Result (8 dummies, System Settings in front) |
|---|---|---|
| 20 to 900 | length + 16 | Divider laid out; dummies pushed left |
| 944 | 960 (half of 1920) | Divider laid out; D8 ends just after the app menus |
| 950, 1000 | 966, 1016 | Divider dropped (no slot); dummies back on the right |

The cap is exact: the divider is laid out while `length + 16 <= display
width / 2`, so the longest divider is `width / 2 - 16` (944 on 1920, 848 on
1728 per Stash).

With 16 dummies (Finder in front, menus end near x 385):

| Divider length | On the bar | Folded |
|---|---|---|
| 600 | D16 to D1 | none |
| 900 | `«`, D8 to D1 | D16 to D9 |
| 944 | `«`, D7 to D1 | D16 to D8 |

Findings:

- Items that do not fit fold into the system overflow from the leading
  (left) end, in order. Approach A works on macOS 27: a divider below the cap
  pushes the hidden section into the overflow.
- The overflow control is an `AXButton` of MenuBarAgent, a direct child of
  the bar window (not inside a slot), `AXDescription` "Show Hidden Menu Bar
  Items" (localized, so do not match on it), frame about 18 x 27, placed
  right after the app menus. MenuBarAgent's "Menu Extras" `AXMenuBar` then
  starts at the overflow control instead of at Control Center.
- Folded items stay in the tree as slots, with the owning app's `AXButton`
  inside, but their frames are stacked left of the overflow control (several
  at the same x, overlapping). They keep `AXPress`. Rule: a slot is folded
  when its frame starts left of the overflow control's frame. One of the 16
  dummies (D16) was missing from the tree entirely.
- Every slot has `AXPress`; slots have no role, subrole or identifier. App
  items are the owning process's `AXButton` with `AXTitle` or
  `AXDescription`; Apple items are `AXMenuExtra` with an `AXIdentifier`.
- The cap limits what A can hide even on 1920: with Finder in front the room
  between the overflow control and the system items was about 1225 pt, so a
  944 pt divider still left 7 dummies (about 265 pt) visible. Hiding the
  whole section needs a second spacer between the hidden items and the
  divider (IceMelt's approach) or fewer visible items.
- Hidden and always-hidden items both fold into the same overflow; macOS 27
  has one overflow per bar, so A gives a single hidden section.

### Experiment: second spacer, placement and Command-drag (VM, 2026-10-16)

BarProbe v2 creates the items listed in a spec file, each with an autosave
name, and reads each spacer's length from a file. Order on the bar, left to
right: D16 ... D1, sp2, div (sp2 was created after div, the dummies after
sp2).

**Second spacer.** Divider at the cap (944), Finder in front, sp2 widened:

| sp2 length | Overflow control x | On the bar (left of div) | Folded |
|---|---|---|---|
| 0 | 406 | D7 ... D1, sp2 | D16 ... D8 |
| 100 | 419 | D4 ... D1, sp2 | the rest |
| 200 | 433 | D1, sp2 | the rest |
| 240 | 429 | sp2 | all 16 |
| 260 | 409 | sp2 | all 16 |
| 300 | 685 | nothing | all 16 and sp2 itself |

Two spacers below the cap hide the whole section. Too long and the spacer
folds too: the overflow control then sits next to the divider. The lengths
must fill the room exactly: from the overflow control's right edge to the
divider's left edge, with a few points of margin.

**Positions.** MenuBarAgent stores every item's position in
`~/Library/Group Containers/com.apple.MenuBar/Library/Preferences/com.apple.MenuBar.plist`,
key `TrailingItemPreferredPositions`, entries `status:<bundle id or
name>::<autosave name>`. The value is the sum of the widths of the items
right of it (clock 0, Control Center 105 = clock width, and so on), so it
orders items from the trailing end. Apple items are `module:<name>`.

- A relaunch keeps every saved position.
- A new item (no saved entry) with no preferred position lands at the far
  left, not by creation order.
- A new item whose app wrote `NSStatusItem Preferred Position <autosave
  name>` in its own defaults before creating it lands by that value:
  `div + 1` (207) put it immediately left of the divider. AppKit no longer
  writes that key back; the app's value is only read for an item
  MenuBarAgent has not saved.
- A written value does not move an item MenuBarAgent already saved (sp3
  stayed at the far left with 208 written).

**Command-drag.** A Command mouse-down on sp3 at the far left, ten drag
events and a mouse-up next to the divider moved sp3 there in one try
(posted from a helper with Accessibility, inside the VM).

**Overflow control.** Clicking it (the control turns into `»`) hides the app
menus except the app name and shows the folded items in their place, as
many as fit between the app name and the first item still on the bar; the
rest stay unreachable. The tree only lists the folded items that would fit
there, which explains the missing entries above. Folded spacers show
squeezed. The overflow is not a way to reach a long hidden section;
showing items means shrinking the spacers.

**Showing.** Setting both spacers back to 20 returns every item to the bar
and removes the overflow control.

## Approaches other projects use

**A. Size the divider so the system overflow hides items** (public APIs plus
Accessibility). IceMelt (merged, #46) and PWB97's Ice branch (#980). The
hidden-section divider is sized to the room between the application menus
and the visible items, capped below half the display width, so the hidden
items overflow into the system `«`. Limits: on wide displays the cap leaves
the leading hidden items on the bar (IceMelt: 3840 pt display, room about
2500 pt, cap about 1904 pt); the fix there is a second spacer placed
immediately left of the divider, which needs a native Command-drag of the
app's own item. Width must be re-measured when the app menus or the display
change (IceMelt: on every cache and app activation, hysteresis 8 pt). Fast
toggling leaves fading icons (Ice #980). Moving and clicking items needs
new code, since there are no item windows.

**B. Assessment-mode allowlist** (private `MenuBarClientCore`:
`MBAssessmentModeConfiguration`, `MBAssessmentModeAssertion`). Thaw, Barometer,
Stash, Hidden Bar's direct build (#403), Ice #995, Pelmet. An assertion lists
the apps allowed to draw status items; the rest are not drawn. Works at any
width. Limits: per app, not per icon; no reordering; one allowlist for all
displays. While an assertion is live, MenuBarAgent ignores clicks on its own
items (clock, battery, Wi-Fi) and Globe+N (Ice #995 holds the click, lifts the
assertion 80 to 120 ms and replays it); PWB97 rejected B because it broke
clock input. The configuration takes `NSArray`s (an `NSSet` raises); system
items are numeric IDs (2 clock, 5 sound, 6 Wi-Fi, 8 Control Center), and
`0...63` hides Screen Mirroring. The app's own item stays only for the copy
LaunchServices resolves (in practice `/Applications`); Ice #995 says only
Developer ID-signed builds keep it, Stash (ad hoc) contradicts that.

**Apple's own controls**: System Settings > Control Center chooses which
Apple items show; the overflow control has no settings.

### Experiment: over-filling, seeds and frames (VM, 2026-10-16)

- **Over-filling hides too.** A 500 pt item standing in for the visible
  section, and left of it a "divider" and a "spacer" both at the cap (944)
  with only about 700 pt of room: MenuBarAgent folded both and every item
  left of them, kept the visible item, and put the overflow control next to
  it. With room for the divider but not the spacer (sp2 at 300 above), only
  the spacer and the items left of it folded. Whatever does not fit folds
  from the left, so lengths at the cap hide everything left of the divider
  without measuring the room. The only rule that matters is the cap: a
  divider at half the display or wider is dropped and everything comes
  back.
- **Seeds.** A new item with a written preferred position of 0 landed at
  the far left like an item without one (0 means none). A value of 1 put the
  item right of every other status item, left only of Control Center and
  the clock, which stay at the trailing end. ice-rust's macOS 26 seeds
  (icon 0, hidden divider 1) therefore put the icon at the far left and the
  divider at the far right on macOS 27.
- **Frames.** For items on the bar, the AppKit frame of the button's window
  matches the Accessibility slot exactly. For folded items it is nonsense
  (x 186 and 150 for two folded 960 pt items), so the order of our items
  can only be read while they are collapsed.
- **Visibility.** `isVisible = false` removes an item and `true` brings it
  back in the same place, also across a relaunch; MenuBarAgent keeps its
  entry. A hidden item keeps its old value while others are recomputed, so
  it can drift to the other side of the divider.
- **Zero length.** An untitled item of length 0 is a 16 pt blank slot.

### Findings while testing v1.8 (VM, 2026-10-16)

- **Allow in the Menu Bar.** After a VM restart IceRust's items were gone:
  MenuBarAgent logged `Creating status item ..., isAllowed: false` for each
  and dropped their scenes about 100 ms later. The switch is in System
  Settings > Menu Bar (`x-apple.systempreferences:com.apple.ControlCenter-Settings.extension`)
  and stored in the `trackedApplications` data of
  `group.com.apple.controlcenter` (`isAllowed` per bundle id). How it got
  switched off is unknown. With the app not allowed, AppKit reports no
  frames for its items.
- **Values recomputed without our items.** After switching the app back
  on, MenuBarAgent had stored the hidden divider and the spacer at the same
  value (165), and the spacer came back right of the divider.
- **An untitled item of length 0 cannot be dragged.** A Command-drag of the
  16 pt blank spacer did nothing; with a length of 20 and a title it moved
  in one drag, and MenuBarAgent stored the new order.
- **AppKit frames lag behind a move** by more than 500 ms, so our items are
  found in MenuBarAgent's tree by their accessibility identifier (set to
  the autosave name on each button; it shows up as the slot's
  `AXIdentifier`).
- **Capture indicator.** Capturing the menu bar for Menu Bar Layout made
  macOS show its "Audio and Video Controls" item (the screen capture
  indicator) for a while.
- `MenuBarAgent`'s menu bar window (layer 24) is listed but not on screen;
  the Window Server "Menubar" window (layer 24, the one clicks land on) is.

### Experiment: press highlight and the slide (VM, 2026-10-16)

Maintainer report: a large whitish capsule shows while clicking the
empty-looking menu bar, and hidden items slide from the far left to their
place when shown, and back when hidden.

- **Press highlight.** The empty-looking part of the bar is our expanded
  item, and MenuBarAgent draws its press highlight (a capsule the size of
  the slot) when it is clicked. A test item held pressed for 1.5 s showed
  the same capsule with its button enabled, disabled, and with the cell's
  `highlightsBy` empty: the app's button state does not reach it. A click
  on the icon shows the usual small capsule.
- **Slide.** Hidden items come out of the overflow control at the far left
  and slide right as the divider shrinks, and slide left into it as the
  divider grows. Five frames per transition, three ways to change the test
  divider: a plain length change, a length change inside
  `NSAnimationContext` with duration 0 and no implicit animation, and
  `isVisible` false and true instead of a length change. All three slid
  the same way: MenuBarAgent animates its own layout.
- **Reduce Motion.** macOS 27's Accessibility > Display has no Reduce
  motion switch (the search term still exists; Dim flashing lights is
  there). Writing `reduceMotion` true to `com.apple.universalaccess` did
  not change the slide when showing and hiding with ice-rust, so it is not
  a workaround users could apply. The key was removed again.

## Design (v1.8, approach A)

On macOS 27 and later (`os_major >= 27`); macOS 26 and earlier keep the
window-list backend unchanged.

- **Spacers.** Two more status items, `IceRust.ControlItem.Spacer` right
  after the hidden divider and `IceRust.ControlItem.AlwaysHiddenSpacer`
  right after the always-hidden divider, each directly left of its
  divider. They stay visible (length 0, a 16 pt blank) so MenuBarAgent
  keeps them next to their dividers when values are recomputed. The hidden
  spacer's seed is the hidden divider's stored position + 1; the
  always-hidden spacer has none, like its divider, and lands left of it
  when both are new.
- **Seeds.** Icon 1, hidden divider 2, always-hidden divider none (it lands
  at the far left, where folding it does no harm). Only missing keys are
  written, as on macOS 26.
- **Lengths.** Cap = floor(narrowest display width / 2) - 16 - 2. All
  hidden: both dividers and both spacers at the cap (the always-hidden pair
  stays grown so its items get no room while the hidden divider grows; they
  flashed on the left when it collapsed first). Hidden shown: hidden divider
  collapsed, always-hidden divider and its spacer at the cap. All shown: everything collapsed, spacers 0. A
  spacer grows only while it sits directly left of its divider. The 10000
  pt length is never used.
- **Spacer repair.** A spacer found away from its divider gets a handle
  (length 20, a title), one Command-drag to directly left of the divider,
  and goes back to a blank slot, at most twice a run per spacer, before a
  hide. Order and spacer places are read from MenuBarAgent's slots (our
  items by identifier) when Accessibility answers, else from AppKit frames.
  A spacer is next to its divider when no slot lies between them in the
  bar's order; comparing edges read wrong while MenuBarAgent animated a
  change, and a spacer overlapping another slot (folded) says nothing.
- **Not in the menu bar.** When our items have no frames after launch,
  an alert says to turn on IceRust under Allow in the Menu Bar, with a
  button to that pane, and the hide waits until they appear.
- **Order check** from AppKit frames, taken while the items concerned are
  collapsed, and cached for the others. A divider right of the icon keeps
  everything shown with the existing "Command-drag the icon right of the
  dividers" message.
- **Show on click and hover** take the occupied parts of the menu bar from
  MenuBarAgent's Accessibility tree instead of the window list; our own
  expanded items count as empty space, as on macOS 26 where they are off
  screen. Without an answer nothing counts as empty.
- **Menu Bar Layout:** the bars from MenuBarAgent's slots, split by our
  dividers; every section stays shown while the page is open (folded items
  have no position), moves are single Command-drags read back afterwards.
- **Not yet on macOS 27:** the Ice Bar (items show in the menu bar
  instead) and Hide app menus.
- **Limits:** a divider and its spacer together cover almost a whole
  display, but the cap follows the narrowest display, so on a wider one the
  leftmost items of a section can stay on the bar; a spacer that is folded
  (a crowded bar) cannot be read or dragged back, and then its divider
  hides alone; clicking the system overflow shows only as many folded items
  as fit where the app menus were.

## Sources

- Ice #995, #980, #1006, #954: https://github.com/jordanbaird/Ice/pull/995,
  https://github.com/jordanbaird/Ice/pull/980,
  https://github.com/jordanbaird/Ice/issues/1006,
  https://github.com/jordanbaird/Ice/issues/954
- IceMelt #46, #47: https://github.com/pnikolaidis/icemelt/pull/46,
  https://github.com/pnikolaidis/icemelt/issues/47
- Hidden Bar #400, #403: https://github.com/dwarvesf/hidden/pull/400,
  https://github.com/dwarvesf/hidden/pull/403
- Stash: https://github.com/brentc22/stash
- CodexBar #2306 (MenuBarAgent hang on 27 beta):
  https://github.com/steipete/CodexBar/issues/2306
- Brow, macOS 27 menu bar: https://brow-app.com/blog/macos-27-menu-bar
- Apple Developer Forums, Virtualization Q&A (macOS 27 guest needs a 26.6 or
  later host): https://developer.apple.com/forums/thread/830118
