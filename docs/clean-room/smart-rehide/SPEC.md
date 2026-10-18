# Smart and focused-app rehide: behavior spec

Status: reviewed. The owner's decisions are recorded in Part B
(2026-10-18). Nothing is built from this spec until the owner says go.

This spec describes how Ice decides to hide revealed menu bar items again
under its "Smart" and "Focused app" rehide strategies, so ice-rust can
rebuild the feature without reading Ice's code. Part A states Ice's observed
behavior as rules, each with its evidence. Part B lists the design choices
for ice-rust; those are decisions, not observations.

**Rebuild sessions:** read only this file. Do not open the evidence sources
listed below, Ice's repository, its forks, or the Ice binary.

## Sources

| Tag | Source |
| --- | --- |
| SRC | Ice source at tag `0.11.13-dev.2` (github.com/jordanbaird/Ice). Paths are relative to the repository root. |
| BIN | `/Applications/Ice.app` 0.11.13-dev.2 (build 1120), arm64 slice, SHA-256 `5fe28a59439a253435397105e9e735c110b25a266618b22caa30d2c13075b7e3`, analyzed with REA 6.4.0 and Ghidra 12.1.4. `ev_...` IDs are REA Evidence IDs. |
| RUN | Runtime observation on the developer's Mac (macOS 26) with Ice 0.11.13-dev.2 installed. |
| SDK | macOS SDK headers (`CoreGraphics.framework/Headers`). |

Binary evidence used below:

| ID | What it shows |
| --- | --- |
| `ev_d8b2adea` | Pseudocode of `FUN_10000e100`, the code that runs after the smart rehide wait: active Space comparison, pointer location, window filter, owner lookup, Dock check, active and regular check, and the hide steps. |
| `ev_c2c0d091` | `CGSGetActiveSpace` is referenced from `FUN_10000e100`. |
| `ev_cf833470` | `CGWindowLevelForKey` is referenced from `FUN_10000e100`. |
| `ev_19ebda1e` | Strings `RehideStrategy`, `_autoRehide`, `_rehideStrategy`, `_rehideInterval`, `Automatically rehide`. |
| `ev_f7152e31` | The three strategy descriptions shown under the Strategy picker. |
| `ev_8e0efed4` | Ice imports `NSWorkspace.frontmostApplication`. |
| `ev_3b5bf641` | `com.apple.dock` is absent from the string table; Swift stores strings of 15 bytes or fewer inline, and `ev_d8b2adea` holds it as two inline words. |
| `ev_71b7feec` | `FUN_1000b6cfc`, the "is hidden" test called before each hide step in `ev_d8b2adea`: it reads the divider state and which section the Ice Bar shows. |
| `ev_dc40edec` | `FUN_1000b84e8`, called at the end of each hide step: it invalidates the rehide timer, stops the rehide monitor, and clears both. |

Evidence quality labels: **observed** (read directly in SRC, BIN or RUN),
**inferred** (follows from observed code, such as the absence of a check),
**external** (platform documentation, to confirm in tests).

## Terms

- **Sections.** Ice has three: *visible* (always shown; holds the app's own
  menu bar icon), *hidden*, and *always-hidden*. Each has a divider item
  whose state is *show* or *hide*.
- **Shown section.** A section counts as shown when its divider state is
  *show*, or when the Ice Bar panel currently displays that section. The
  visible section counts as shown while the Ice Bar displays the hidden
  section. (SRC `Ice/MenuBar/MenuBarSection.swift:80-102`, observed)
- **Something is revealed.** At least one of the three sections is shown.
  (SRC `Ice/MenuBar/MenuBarManager.swift:62-64`, observed)
- **Hide everything.** Run the hide action (rules H1 to H4) on each of the
  three sections in order.
- **Pointer in the menu bar.** Rule M1.
- **Working screen.** Rule M2.

## Part A: Ice's behavior

### Settings

**S1. Three settings.** "Automatically rehide" (on or off, default on),
"Strategy" (Smart, Timed, Focused app; default Smart), and "Interval"
(seconds, default 15).
Evidence: SRC `Ice/Settings/Models/GeneralSettings.swift:57,60,64`
(observed); BIN `ev_19ebda1e` (observed); RUN `defaults read
com.jordanbaird.Ice` shows `AutoRehide = 1`, `RehideInterval = 15`,
`RehideStrategy = 0` (observed).

**S2. Stored values.** The strategy is stored as an integer under
`RehideStrategy`: Smart 0, Timed 1, Focused app 2. A stored integer outside
0 to 2 is ignored and the default (Smart) stays. "Automatically rehide" is
stored as a boolean under `AutoRehide`, the interval as a number of seconds
under `RehideInterval`.
Evidence: SRC `Ice/Settings/Models/GeneralSettings.swift:94-95,102-106,
227-233` (observed); RUN key names as in S1 (observed).

**S3. Settings UI.** The "Automatically rehide" switch is always shown. The
Strategy picker appears only while the switch is on. Under the picker, one
line describes the selected strategy:
- Smart: "Menu bar items are rehidden using a smart algorithm."
- Timed: "Menu bar items are rehidden after a fixed amount of time."
- Focused app: "Menu bar items are rehidden when the focused app changes."

The Interval slider (0 to 30 seconds, step 1) appears only when the
strategy is Timed.
Evidence: SRC `Ice/Settings/SettingsPanes/GeneralSettingsPane.swift:
223-262` (observed); BIN `ev_f7152e31` (observed).

**S4. Strategy labels.** The picker shows "Smart", "Timed" and
"Focused app".
Evidence: SRC `Ice/Settings/Models/GeneralSettings.swift:238-243`
(observed).

### Smart strategy

**R1. Trigger.** Every left mouse-down is a candidate, whether it lands in
another app or in one of Ice's own windows. Ice listens with a global and a
local monitor together. Right and other mouse buttons never trigger smart
rehide. Ice's show-on-click handling runs first for the same event.
Evidence: SRC `Ice/Events/EventManager.swift:24-33`,
`Ice/Events/EventMonitor.swift:119-130` (observed).

**R2. Enabled only for Smart.** A candidate is dropped unless
"Automatically rehide" is on and the strategy is Smart.
Evidence: SRC `Ice/Events/EventManager.swift:195-200` (observed).

**R3. The app's own icon is ignored.** A click whose window is the window
of Ice's own menu bar icon is dropped. Clicks in other apps carry no window
of Ice's, so this check only affects Ice's own windows.
Evidence: SRC `Ice/Events/EventManager.swift:202-207` (observed).

**R4. The Ice Bar is ignored.** A click whose window is the Ice Bar panel is
dropped.
Evidence: SRC `Ice/Events/EventManager.swift:209-213` (observed).

**R5. Something must be revealed.** A candidate is dropped when no section
is shown.
Evidence: SRC `Ice/Events/EventManager.swift:215-218`,
`Ice/MenuBar/MenuBarManager.swift:62-64` (observed).

**R6. Clicks in the menu bar are ignored.** A candidate is dropped when the
pointer is in the menu bar of the working screen (M1, M2) at the moment of
the click. This keeps the items open while the user clicks them.
Evidence: SRC `Ice/Events/EventManager.swift:220-223` (observed).

**R7. Space snapshot.** At the click, Ice records the ID of the active Space
(the window server's active Space for its main connection).
Evidence: SRC `Ice/Events/EventManager.swift:225`,
`Shared/Bridging/Bridging.swift:162-163` (observed); BIN `ev_c2c0d091`
(observed).

**R8. Wait 250 ms.** Ice then waits 250 milliseconds without blocking, so
the clicked window has time to come to the front. The wait cannot be
cancelled; every click that passes R2 to R6 starts its own wait, and
pending checks do not replace each other.
Evidence: SRC `Ice/Events/EventManager.swift:227-229` (observed for the
delay; inferred for "cannot be cancelled" and "do not replace each other",
since no handle to the task is kept).

**R9. Settings are not read again after the wait.** Turning off
"Automatically rehide" or switching strategy during the 250 ms does not
stop a pending check.
Evidence: SRC `Ice/Events/EventManager.swift:227-266` (inferred: the
waiting task never reads the settings); BIN `ev_d8b2adea` (observed: no
settings read before the Space comparison).

**R10. Space changed: hide everything.** After the wait, if the active
Space ID differs from the snapshot in R7, Ice hides everything and the
check ends. No window check runs in that case.
Evidence: SRC `Ice/Events/EventManager.swift:231-236` (observed); BIN
`ev_d8b2adea` (observed: unequal IDs lead straight to the per-section hide
loop).

**R11. Pointer location after the wait.** Otherwise Ice reads the pointer
location again, at the end of the wait, in global coordinates with the
origin at the top left of the main display. If the location cannot be read,
the check ends with no change.
Evidence: SRC `Ice/Events/EventManager.swift:240-241`,
`Ice/Utilities/MouseHelpers.swift:21-22` (observed); BIN `ev_d8b2adea`
(observed: a fresh event is created and its location read; a null event
ends the check).

**R12. Candidate windows.** Ice takes the list of on-screen windows from the
window server and keeps the ones whose layer is below the cursor window
level (the level for key 19, `kCGCursorWindowLevelKey`).
Evidence: SRC `Ice/Events/EventManager.swift:242-243`,
`Shared/Bridging/Bridging.swift:390-416` (observed); BIN `ev_d8b2adea`
(observed: compares each window's layer with `CGWindowLevelForKey(0x13)`);
SDK `CGWindowLevel.h` (observed: key 19 is the cursor level).

**R13. Window under the pointer.** From the candidates, in list order, Ice
picks the first window whose bounds contain the pointer location and whose
title is present and not empty. A window without a title is skipped even
when it covers the pointer. If no window qualifies, the check ends with no
change.
Evidence: SRC `Ice/Events/EventManager.swift:242-245` (observed); BIN
`ev_d8b2adea` (observed: `CGRectContainsPoint`, then a string length
check, then the loop moves on). External: the on-screen list is ordered
front to back (SDK `CGWindow.h`, comment on
`kCGWindowListOptionOnScreenOnly`); Ice uses a private window server call
for the list, so its order is inferred to match.

**R14. Owning app.** Ice looks up the running app by the window's owner
process ID. If no running app matches, the check ends with no change.
Evidence: SRC `Ice/Events/EventManager.swift:245`,
`Shared/Utilities/WindowInfo.swift:35-36` (observed); BIN `ev_d8b2adea`
(observed: `runningApplicationWithProcessIdentifier:`, nil ends the check).

**R15. Dock exception.** If the owning app's bundle identifier is
`com.apple.dock`, Ice hides everything without further checks.
Evidence: SRC `Ice/Events/EventManager.swift:250-251` (observed); BIN
`ev_d8b2adea` (observed: comparison against the inline string
`com.apple.dock`), `ev_3b5bf641`.

**R16. Active regular app.** For any other app, Ice hides everything only
when that app is active (frontmost) and its activation policy is regular
(an app with a Dock icon and menus). Otherwise the check ends with no
change. This is what makes a click into a normal app window rehide, while a
click on a background agent's panel or an inactive window does not.
Evidence: SRC `Ice/Events/EventManager.swift:252-258` (observed); BIN
`ev_d8b2adea` (observed: `isActive`, then `activationPolicy` compared with
0, the regular policy).

**R17. Hiding order.** When R10, R15 or R16 hides everything, Ice applies
the hide action to the visible, hidden and always-hidden sections in that
order.
Evidence: SRC `Ice/Events/EventManager.swift:233-235,263-265`,
`Ice/MenuBar/MenuBarManager.swift:54-58` (observed).

### Focused app strategy

**F1. Trigger.** Each change of the frontmost application is a candidate.
Ice handles it on the main thread. The observation also delivers the
current frontmost app once when Ice sets it up.
Evidence: SRC `Ice/MenuBar/MenuBarManager.swift:113-115` (observed); BIN
`ev_8e0efed4` (observed). External: the Foundation key-value publisher
used here delivers the initial value by default.

**F2. Enabled only for Focused app, regardless of the switch.** A
candidate is dropped unless the strategy is Focused app. Ice does not check
"Automatically rehide" here, so a stored Focused app strategy keeps working
after the switch is turned off, even though S3 hides the picker then.
Evidence: SRC `Ice/MenuBar/MenuBarManager.swift:117-123` (observed for the
strategy check; inferred for the missing switch check).

**F3. Clicks in the menu bar are ignored.** A candidate is dropped when the
pointer is in the menu bar of the working screen (M1, M2) when the change
arrives.
Evidence: SRC `Ice/MenuBar/MenuBarManager.swift:122-123` (observed).

**F4. Hide after 0.1 s.** After a 0.1 second wait, Ice runs the hide action
on the hidden section, which by H3 hides every section. There is no Space
check, no window check, and no exception for Ice itself becoming frontmost.
Evidence: SRC `Ice/MenuBar/MenuBarManager.swift:125-128` (observed for the
wait and the action; inferred for the missing checks).

### Timed strategy, as it relates to this spec

**T1. Timed checks run only for Timed.** Ice starts its interval-based
rehide checks when a section is shown, and only when "Automatically
rehide" is on and the strategy is Timed. With Smart or Focused app, no
interval timer runs.
Evidence: SRC `Ice/MenuBar/MenuBarSection.swift:237-245` (observed).

The details of the Timed strategy are out of scope; ice-rust already has
its own.

### Hide action

**H1. No-op when already hidden.** Hiding a section that is not shown does
nothing.
Evidence: SRC `Ice/MenuBar/MenuBarSection.swift:211-214` (observed); BIN
`ev_d8b2adea`, `ev_71b7feec` (observed: the hide steps run only when the
"is hidden" test returns false).

**H2. Ice Bar closes.** Hiding a shown section closes the Ice Bar panel and
allows show-on-hover again.
Evidence: SRC `Ice/MenuBar/MenuBarSection.swift:216-217` (observed); BIN
`ev_d8b2adea` (observed: the panel is closed, its content cleared, and a
published flag set to true).

**H3. Scope.** Hiding the visible or the hidden section, or any section
while the Ice Bar is in use, sets every section's divider to *hide*. Hiding
the always-hidden section without the Ice Bar sets only its own divider to
*hide*.
Evidence: SRC `Ice/MenuBar/MenuBarSection.swift:219-226` (observed); BIN
`ev_d8b2adea` (observed: a loop over sections setting each state).

**H4. Timed checks stop.** Hiding stops any running Timed checks for that
section.
Evidence: SRC `Ice/MenuBar/MenuBarSection.swift:228,290-295` (observed);
BIN `ev_d8b2adea`, `ev_dc40edec` (observed).

### Helpers

**M1. Pointer in the menu bar.** The pointer counts as in the menu bar when
all of these hold:
1. Ice's own icon has a frame, and the top of that frame is not above the
   top of the working screen. If the icon is missing or above the screen,
   Ice assumes the menu bar is hidden and the answer is no.
2. The pointer location (bottom-left origin) is between the left and right
   edges of the working screen, inclusive.
3. Its vertical position is between the top of the screen's visible area
   (the area below the menu bar) and the top of the screen, inclusive.

Evidence: SRC `Ice/Events/EventManager.swift:447-464`,
`Ice/Utilities/MouseHelpers.swift:14-15` (observed).

**M2. Working screen.** When the active Space is a full-screen Space and a
screen contains the pointer, the working screen is that screen. Otherwise
it is the main screen (the one with the key window). If there is no working
screen, the click is not processed at all.
Evidence: SRC `Ice/Events/EventManager.swift:26-28,435-442` (observed).

### Quirks to decide on

These follow from the rules above and may be unintended:

- **Q1.** Focused app ignores "Automatically rehide" (F2).
- **Q2.** Windows without a title never count (R13). Clicking the desktop or
  a title-less window does not rehide. Window titles from the window server
  also depend on the Screen Recording permission on current macOS
  (external; to confirm in a test), so without it smart rehide would rarely
  fire.
- **Q3.** A click is judged by the window under the pointer 250 ms later,
  not at the click (R11). A fast pointer move can change the outcome.
- **Q4.** Focused app also rehides when Ice itself becomes frontmost, for
  example when its Settings window opens (F4).
- **Q5.** With "Hide application menus" on, Ice makes itself the active app
  when shown items reach the app menus, and steps back to the previous app
  when the sections hide. Each switch changes the frontmost app, so under
  Focused app a reveal while the pointer is outside the menu bar (a hotkey,
  for example) is hidden again 0.1 s later (F1, F3, F4).
  Evidence: SRC `Ice/MenuBar/MenuBarManager.swift:158-216,299-307,309-318`
  (observed: activation with the regular policy, deactivation to the
  accessory policy); the effect on F1 is inferred.
- **Q6.** Nothing checks for an open menu from a menu bar item. A click
  inside such a menu passes R6 (it is below the menu bar) and is judged by
  R13 like any other click.
  Evidence: SRC `Ice/Events/EventManager.swift:195-266` (inferred: no such
  check).
- **Q7.** Outside full-screen Spaces, the working screen is the main
  screen, not the screen under the pointer (M2), so on a second display
  R6 and F3 measure the wrong menu bar.
  Evidence: SRC `Ice/Events/EventManager.swift:435-442` (observed).

### Laya check

Each rule went to Laya 0.4.10 (English checkpoint `laya`, ModernBERT-large)
as a state of "Rule" plus "Evidence" (the cited source lines and a short
summary of the binary evidence), with one yes-or-no question: "Does the
evidence show that the rule is true?" The score is Laya's probability of
yes. Bands: 0.80 or more passes; 0.50 to 0.80 is unclear and was reviewed
by hand against the evidence; under 0.50 went back to REA for more evidence
and was scored again. The pairs stay outside this repository because they
quote Ice's source.

As a control, four pairs matched a rule with the evidence of a different
rule (one with a false rule). Laya scored all four between 0.14 and 0.20,
so it does separate matching from non-matching evidence. Laya's own
documentation says its confidence orders decisions and is not proof, so a
pass here does not replace the tests in step 4.

| Rule | Score | Round | Result |
| --- | --- | --- | --- |
| R17 | 0.96 | 1 | pass |
| F4 | 0.92 | 1 | pass |
| H2 | 0.91 | 1 | pass |
| R12 | 0.87 | 1 | pass |
| F1 | 0.86 | 2 | pass (0.46 in round 1) |
| F3 | 0.85 | 1 | pass |
| S1 | 0.84 | 1 | pass |
| H3, M2 | 0.84 | 1 | pass |
| H4 | 0.84 | 2 | pass (0.12 in round 1) |
| T1 | 0.83 | 2 | pass (0.35 in round 1) |
| M1 | 0.82 | 1 | pass |
| S4 | 0.81 | 2 | pass (0.11 in round 1) |
| R5, R13 | 0.797 | 1 | unclear, reviewed: supported |
| R16 | 0.76 | 1 | unclear, reviewed: supported |
| R2 | 0.76 | 1 | unclear, reviewed: supported |
| R9 | 0.74 | 1 | unclear, reviewed: supported (inferred rule) |
| R6 | 0.72 | 1 | unclear, reviewed: supported |
| R1 | 0.72 | 1 | unclear, reviewed: supported |
| R4 | 0.71 | 1 | unclear, reviewed: supported |
| F2 | 0.70 | 1 | unclear, reviewed: supported |
| R10 | 0.69 | 1 | unclear, reviewed: supported |
| S3 | 0.69 | 2 | unclear, reviewed: supported (0.42 in round 1) |
| H1 | 0.69 | 2 | unclear, reviewed: supported (0.13 in round 1) |
| R3 | 0.67 | 1 | unclear, reviewed: supported |
| R15 | 0.67 | 2 | unclear, reviewed: supported (0.45 in round 1) |
| R14 | 0.65 | 1 | unclear, reviewed: supported |
| S2 | 0.60 | 1 | unclear, reviewed: supported |
| R8 | 0.60 | 1 | unclear, reviewed: supported |
| R7 | 0.55 | 1 | unclear, reviewed: supported |
| R11 | 0.25 | 3 | not confirmed by Laya; accepted by the owner on SRC and BIN |
| Q5 | 0.84 | 1 | pass (activation claim) |
| Q7 | 0.77 | 1 | unclear, reviewed: supported |

Round 2 added binary evidence from REA (`ev_71b7feec`, `ev_dc40edec`) and
wider source excerpts. R11 stayed low through three rounds, including a
split into two simpler claims (0.22 and 0.25). Its evidence: the pointer is
read by creating a new event inside the task, after the 250 ms sleep, and a
null event ends the check (SRC and `ev_d8b2adea`). The hand review finds it
supported, and the owner accepted it on that evidence on 2026-10-18.
No runtime check was run. On macOS 27, MenuBarAgent filters out Ice's
status item (a MenuBarAgent log quoted in `docs/macos27.md`), so the VM
has nothing to reveal or observe. On the developer's macOS 26 Mac the check would need Ice
running with Accessibility and Screen Recording, plus synthetic clicks
that move the owner's own pointer. The step 4 tests pin R11 in the
rebuild instead.

### Forks

Eleven forks and similar apps were read (shallow clones of the default
branch, 2026-10-17; no commit history, nothing built or run). This section
records what they changed, as behavior, so Part B can decide. Rebuild
sessions must not open these repositories either: most are GPL-3.0 code
derived from Ice.

| Repo | License | Smart or focus rehide | Notes |
| --- | --- | --- | --- |
| holzcloud/holzBar | GPL-3.0 | Yes, changed | Fixes Q1, Q2, Q5, with unit tests |
| thaw-app/Thaw | GPL-3.0 | Yes, extended | Fixes Q1, Q4, Q6; smart also reacts to app switches |
| cavaldos/Ice | GPL-3.0 | Yes, changed | Fixes Q6 |
| teddychan/ice-2 | GPL-3.0 | As Ice | Fixes Q7 for its hit tests |
| pdurlej/fire-from-ice | GPL-3.0 | As Ice | Window list read off the main thread |
| RabenkoYevhenii/Ice | GPL-3.0 | As Ice | No rehide changes |
| fif7y/dimble (formerly pelmet) | GPL-3.0 | Click outside or delay | Ignores its own posted clicks; 0.75 s floor |
| benjustjammin/justhide | GPL-3.0 | Timed only | |
| dwarvesf/hidden | MIT | Timed only | |
| Superoutman/ShiftBar | none (binary only) | Click outside (README) | |
| vorssaint/vorssaint-utils | GPL-3.0 | No hiding feature | |

Behaviors worth considering, with where they come from:

- **FK1. Own activation is not a focus change (fixes Q4, Q5).** holzBar and
  Thaw skip their own app becoming frontmost. holzBar also skips the return
  to the app that was frontmost before it, and its tests pin both.
  Evidence: holzBar `holzBar/Core/FocusChangeRehide.swift:8-52`,
  `Tests/HolzBarCoreTests/FocusChangeRehideTests.swift`; Thaw
  `Thaw/MenuBar/MenuBarManager.swift:1273-1281`.
- **FK2. Focused app requires the switch (fixes Q1).** holzBar and Thaw.
  Evidence: holzBar `holzBar/Core/FocusChangeRehide.swift:29-37`; Thaw
  `Thaw/MenuBar/MenuBarManager.swift:242`.
- **FK3. Pick the clicked window without its title (fixes Q2).** holzBar's
  comment says macOS withholds other apps' window titles without Screen
  Recording, so Ice's rule never fired with Accessibility alone. Its rule:
  the first window, front to back, below the cursor level, containing the
  point, that is at layer 0 (a normal app window) or owned by the Dock;
  floating panels and other apps' desktop-level windows are passed over.
  Evidence: holzBar `holzBar/Core/RehideClickTarget.swift:9-48`,
  `Tests/HolzBarCoreTests/RehideClickTargetTests.swift`. The Screen
  Recording claim was not tested here.
- **FK4. No rehide while a menu bar item's menu is open (fixes Q6).** Thaw
  probes for an open item menu and ignores windows that stay up longer than
  30 s; cavaldos checks for a focused menu through Accessibility and for
  dropdown-shaped windows.
  Evidence: Thaw `Thaw/Events/HIDEventManager.swift:1064-1067`; cavaldos
  `Ice/Events/EventManager.swift:283-304,741-780`.
- **FK5. Ignore clicks the app posts itself.** holzBar tags its own clicks;
  dimble refuses its own by tag or within 0.25 s.
  Evidence: holzBar `holzBar/Events/HIDEventManager.swift:85-88,538`;
  dimble `Pelmet/Events/MenuBarBandMonitor.swift:599-602,698-712`.
- **FK6. Use the screen under the pointer (fixes Q7).** teddychan, holzBar
  and Thaw.
  Evidence: holzBar `holzBar/Events/MenuBarHitTesting.swift:19-36`;
  teddychan `Ice/Events/HIDEventManager.swift:555-605`.
- **FK7. Read the window list off the main thread.** fire-from-ice moved it
  after a recorded freeze.
  Evidence: fire-from-ice `Ice/Events/HIDEventManager.swift:255-262`.

Paths are relative to each repository. The full comparison, with line
numbers for every claim, is kept outside this repository.

## Part B: ice-rust design (decisions, not Ice behavior)

Decided with the owner on 2026-10-18. D1, D2, D3, D5 and D6 were
confirmed one by one; the others stand as written. D5 was revised twice
the same day, with the owner's approval, after D10 runs showed that the
earlier versions did not hold.

- **D1. Default strategy.** Ice defaults to Smart. ice-rust users today get
  Timed behavior. Decision: fresh installs default to Smart; an install
  that already has `AutoRehide` or `RehideInterval` stored but no
  `RehideStrategy` keeps Timed, so an update does not change behavior.
- **D2. Q1.** Decision: Focused app also requires "Automatically rehide",
  matching what the Settings UI suggests (as FK2).
- **D3. Q4, Q5.** ice-rust also activates itself to hide application menus
  and reactivates the previous app afterward
  (`src/platform/app/app_menus.rs`), so Q5 applies to ice-rust as well.
  Decision: adopt FK1. ice-rust becoming frontmost is not a focus change,
  and neither is a return to the last frontmost app other than ice-rust.
- **D4. Pointer in the menu bar.** Decision: reuse ice-rust's existing
  pointer-in-menu-bar test, which already works on macOS 27, in place of
  M1's icon-frame heuristic, measured on the screen under the pointer
  (FK6, fixes Q7).
- **D5. Which app was clicked (Q2, Q6).** Revised twice after D10 runs.
  The first version took FK3's window rule (no titles) with a stop for
  menus and relied on the window list alone. The second added an
  accessibility hit test but kept a list of system windows to pass over,
  and macOS showed more full-display overlays than the list held. The D10
  results below show both failures. Ice's rule (R13) is not used either,
  since it needs window titles and so Screen Recording. Decision, run
  after the wait:
  1. *Clicked app.* The process ID that an accessibility hit test at the
     pointer returns (the app that owns the element under the pointer,
     from `AXUIElementCopyElementAtPosition` on the system-wide element),
     read after the wait like the pointer (R11). When the hit test gives
     nothing (no Accessibility permission, an error or a timeout), the
     owner of the first window at layer 0, front to back, below the cursor
     level (R12), that contains the pointer. When neither exists, no
     change. A process ID with no running app gives no change (R14).
  2. *Dock.* When the clicked app has the Dock's bundle identifier, hide
     everything (R15).
  3. *Menu stop (Q6).* Among the on-screen windows below the cursor level
     that contain the pointer, take the first one, front to back, that the
     clicked app owns. When it is above layer 0 (a menu, a pop-up or a
     panel of that app), no change. Windows of other processes play no
     part, so their overlays (the full-display windows of the Dock,
     Notification Center or WindowManager) can neither stop nor decide a
     check.
  4. *Active app.* Otherwise hide everything when the clicked app is
     active with the regular policy (R16), and make no change when it is
     not.
  The hit test must not stall the main thread: give the system-wide
  element a messaging timeout of 0.25 s before the call, and treat a
  timeout as no result. Titles play no part, so Screen Recording is not
  needed; the hit test needs Accessibility, which ice-rust already asks
  for (Show on click).
- **D6. Own clicks.** Decision: adopt FK5. Clicks that ice-rust posts (the
  Command-drag moves of Menu Bar Layout) never trigger smart rehide.
- **D7. Not adopted.** Thaw's app-switch trigger and timed fallback for
  Smart (they change what Smart means), and FK7 for now (ice-rust reads
  the window list once per qualifying click, 250 ms after it; revisit if
  testing shows the main thread stalls).
- **D8. Storage.** Same keys and values as S2 in ice-rust's own defaults
  domain, so the meaning matches Ice.
- **D9. Settings UI.** The Strategy picker joins "Automatically rehide" on
  the Hidden Items page, with the S3 visibility rules and descriptions.
- **D10. macOS 27.** Run on 2026-10-18 in three rounds, one per version
  of D5; results below. The current D5 passed every case in the macOS 27
  VM.

### D10 results (2026-10-18)

Setup: the macOS 27.0.1 VM, a build of the first rebuild with a debug log
of every decision (not committed), synthetic left clicks posted at the HID
event tap by a helper app, and on-screen window lists from
`CGWindowListCopyWindowInfo`. The developer's Mac (macOS 26.7.1) was
compared by its window list only, with no input.

| Check | Result |
| --- | --- |
| D1: `AutoRehide` stored, no `RehideStrategy` | The Hidden Items page shows Timed with the interval. |
| D4: a click on the icon or on a menu title | Dropped (pointer in the menu bar). |
| Q6: a click on a separator in Finder's Go menu (menu window at layer 101) | No change; the items stayed shown. |
| F1 to F4: the frontmost app changed with `open -a` | Hidden about 100 ms later; dropped while the pointer was in the menu bar (F3). |
| D3: ice-rust's Settings opened, so ice-rust became frontmost | Dropped; the items stayed shown. Back to Finder (the last other app): dropped. Then TextEdit: hidden. |
| "Hide app menus" on macOS 27 | Not available there, so Q5 cannot occur on macOS 27; D3 was exercised through Settings. |
| D5 as first written | Failed. The Dock owns an on-screen window at layer 20 that covers the whole display, in front of every app window. The walk picked it for every click outside a menu, so smart rehide hid on any click, including clicks that should not hide. The macOS 26.7.1 Mac has the same window (layer 20, the full 2560 x 1440 display). |
| Desktop windows | On macOS 26 the windows at layer -2147483624 belong to the Dock. On macOS 27 the window at that layer belongs to WindowManager and the bottom one (layer -2147483626) to the window server, so the desktop is not a Dock window there. After a desktop click, WindowManager also shows a full-display window at layer 18. |
| D5 second version (hit test plus a list of system windows) | Right for the active app's window, the desktop (hit test: Finder, active after the click), a Dock icon (hit test: Dock), a Notification Center widget (no change; Notification Center is an accessory app) and a separator in the open Apple menu (no change). Wrong after the widget click: Notification Center then kept a full-display window at layer 21, which was not on the list, so it stopped every later check and a desktop click no longer hid. |
| D5 current version (the clicked app's own windows decide the menu stop) | Right in every case, with Notification Center's layer-21 window and the Dock's layer-20 window on screen: a desktop click (hit test: Finder, active) hid; a click on a TextEdit window that activated TextEdit hid; a Dock icon (hit test: Dock) hid; a Notification Center widget gave no change; a separator in the open Apple menu gave no change although the frontmost app (Calendar) was active and regular; the desktop click that closed that menu hid. |
| A click right after a desktop click | macOS 27 moves the windows aside on a desktop click, and the next click only brings them back: it did not activate the app under it, so that check gave no change. Ice's rule gives the same, since the app is not active 250 ms later. |
| R11: a click on the active TextEdit, then the pointer moved onto a Notification Center widget 30 ms later | The check read the pointer at the widget after the wait (hit test: Notification Center) and made no change. The same click without the move hid. |
| R10: Space switches with Control-Left and Control-Right (a full-screen TextEdit made the second Space) | The active Space ID changed 545 to 570 ms after the key, at the end of the animation, and a click sent during the animation reached no event monitor. A click 30 ms before the switch was checked while the Space ID was still the old one, so R10 did not fire. Setting Reduce Motion with `defaults` and restarting the Dock did not shorten the switch. No input in the VM made R10 fire; its rule is covered by the spec tests only. |
| D6: a drag in Menu Bar Layout | The three clicks that ice-rust posted to move the item were each recognized as its own and dropped. They were in the menu bar, so D4 would have dropped them as well. The drag's own mouse-down in Settings led to a hide that was skipped because a move held the sections. |
| Who owns a menu window | The app whose menu it is: Finder's Go menu (layer 101) belonged to Finder, and the Apple menu's window belonged to the frontmost app (Calendar), the same process the hit test returned. |
| Accessibility hit test | A TextEdit text area: TextEdit (`AXTextArea`). The desktop: Finder (`AXGroup`). Dock icons: Dock (`AXDockItem`). A Notification Center widget: Notification Center. The menu bar: the frontmost app's `AXMenuBar`. Each is the app that takes the click. |

On macOS 26.7.1 the owner tried Smart and Focused app by hand with
ice-rust 0.2.2 (2026-10-18) and both worked; there is no log of those
runs.

Not covered: R10 firing on a real Space change (see above) and a posted
click outside the menu bar for D6.

### Testable boundary

So the rules can be tested without a window server, the rebuild should
separate a pure decision from the platform reads:

- Smart, at click time: inputs are the settings, whether the click window
  is the app's icon or the Ice Bar, whether the click was posted by
  ice-rust (D6), whether something is revealed, and whether the pointer is
  in the menu bar. Output: drop, or start a check with the current Space
  ID.
- Smart, after 250 ms: inputs are the snapshot Space ID, the current Space
  ID, the pointer location (or none), the window list front to back
  (layer, bounds, owner process ID), the cursor level, the hit-test
  process ID (or none), and a lookup from process ID to bundle identifier,
  active flag and activation policy (none when no app runs with that ID).
  Output: hide everything, or no change.
- Focused app: inputs are the settings, the new frontmost process ID,
  ice-rust's own process ID, the last frontmost process ID other than
  ice-rust (D3), and whether the pointer is in the menu bar. Output: drop,
  or hide after 0.1 s, plus the updated last process ID.
- Hide action: input is the section states and Ice Bar use. Output: the
  new section states.
