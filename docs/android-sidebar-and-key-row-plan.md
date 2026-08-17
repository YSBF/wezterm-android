# Android terminal sidebar and extra-keys plan

## Status

**Design proposal, revision 2.** This document describes the next user-interface
layer for the Android port: an in-terminal SSH host sidebar and a more usable
extra-keys row.  It does not change the existing terminal renderer, mux, or
Android activity architecture.

## Revision history

* **rev 1** (`6334bc7cb`) --- the proposal as originally written.
* **rev 2** --- reconciled with the extra-keys behavior that has since shipped;
  corrected the key-row sizing rules, which rev 1 stated in a form that cannot
  be satisfied on a phone; added the credential and host-key prompts the connect
  flow requires; resolved rev 1's open question about the runtime domain API;
  and recorded the window-geometry hazard that the pinned sidebar will hit.

## Decision summary

The terminal sidebar and extra-keys row will remain Rust-rendered UI inside
WezTerm's existing GPU surface.  This preserves a single coordinate system for
the terminal, tabs, touch input, window resizing, and rendering.

Anything that needs ordinary text entry is different: it needs form fields and
reliable IME editing.  The Android `WezTermActivity` will present small native
Kotlin dialogs for those, then return validated data to Rust through JNI.  This
avoids reimplementing text fields, selection, cursor handling, and accessibility
in the renderer.  Rev 1 applied this only to add/edit of a host; it applies
equally to passwords and key passphrases, which are text entry with stricter
requirements (masking, no echo, no clipboard leakage).

We will not put a `DrawerLayout` or Compose drawer around the existing
`GameActivity` surface.  Doing so would introduce surface z-order, gesture
dispatch, and terminal-size synchronization problems.  A native dialog is
isolated from those concerns, while the sidebar itself remains part of the
terminal UI.

## Constraints measured on the reference device

Rev 1 asserted sizes without checking them against a device.  Measured on the
Xiaomi `fuxi` used for port verification:

| Quantity | Value |
| --- | --- |
| Screen | 1080 x 2400 px at 440 dpi |
| Density | 2.75 (1 dp = 2.75 px) |
| Usable width | **392 dp** |
| Surface height available to the window | 2235 px (insets applied by the Activity) |
| Current extra-keys row height | 94 px, i.e. **34 dp** |

Two consequences drive the key-row section below.  A row of ten keys at a 44 dp
minimum needs 440 dp before any gaps, against 392 dp of screen; and raising the
row to a 44--48 dp target makes it 30--40% taller than it is today, which is
screen taken from the terminal.  Both are deliberate trades, not oversights.

## Target layout

```
┌─────────────────────────────────────────────┐
│ [menu] tab 1     tab 2                 [+]   │
├───────────┬─────────────────────────────────┤
│ Hosts     │                                 │
│ • dev     │          Terminal grid          │
│ • prod    │                                 │
│ + Add     │                                 │
│           │                                 │
├───────────┴─────────────────────────────────┤
│ CTRL │ ← │ ↑ │ ↓ │ → ║ ESC │ ALT │ SHIFT │ …│
└─────────────────────────────────────────────┘
```

The double rule in the key row marks the boundary between the pinned cluster and
the scrolling remainder; see below.

The `[menu]` affordance shares the tab bar with the tab strip and the new-tab
button.  The tab bar now divides its width between tabs rather than squashing
them, so the menu button must be accounted for in that division rather than
drawn over it.

### Sidebar modes

* **Phone portrait:** the sidebar is an overlay drawer from the left, around
  80% of the available width.  Opening or closing it does not resize the
  terminal grid.  This is the common case and is deliberately the one that
  avoids the resize path entirely.
* **Landscape and tablets:** the user can pin the sidebar.  A pinned sidebar
  is 300--360dp wide, the terminal's usable width is reduced by that amount,
  and the active pane is resized accordingly.  See "Terminal viewport
  rectangle" for why this is the riskiest part of the plan.
* **Entry and dismissal:** a menu item in the tab bar opens the sidebar.  The
  first implementation supports explicit open/close controls and a dimmed
  overlay; edge-swipe gestures are deferred.  The deferral is not only about
  effort: the left screen edge is the Android system back gesture, so an
  edge-swipe drawer would be competing with the platform for the same drag.

### Sidebar contents

The first release is deliberately limited to simple SSH profiles:

* display name;
* host name or IP address;
* port, defaulting to `22`;
* user name;
* an optional key reference (see "Getting a key onto the device");
* connect, edit, and delete actions.

The sidebar also lists SSH domains supplied by `wezterm.lua`, marked as
configuration-file entries.  They are read-only in the first release.  Hosts
created in the app are managed separately.

Passwords must not be saved in the host profile.  Password authentication is
entered at connection time through the native credential dialog described
below.  Any future persistent secret support must use Android Keystore rather
than a plaintext configuration file.

#### Getting a key onto the device

Rev 1 offered a key reference while placing a file picker out of scope.  Those
two decisions together make key authentication unreachable for anyone without
adb: `HOME` is the application-private data directory, and release builds are
not debuggable, so `run-as` is refused and there is no supported path for a user
to place a private key there.

Resolve it one of two ways, and say which:

1. **v1 is password-only.**  Drop the key reference from the first release and
   state it plainly, or
2. **accept a pasted key.**  The native dialog gains a multi-line field; Rust
   writes the key into app-private storage with `0600`.  This needs no Storage
   Access Framework picker and no new permission, and is the cheaper of the two
   to build.

A file picker remains out of scope either way.

## Extra-keys row

The row that shipped after rev 1 was written already differs from what rev 1
describes: it measures its laid-out width, spreads the keys when they fit, and
pans horizontally when they do not, so that the trailing key stays reachable
(`a0ee6fb23`).  Rev 1's rule that no part of the row may "be moved out of view by
horizontal scrolling" contradicts that, and cannot be met anyway at rev 1's own
target sizes.  Rev 2 resolves the conflict in favor of a pinned core plus a
scrolling remainder, which satisfies both goals rather than trading one away.

### Key set

```
pinned:     CTRL │ ← │ ↑ │ ↓ │ →
scrolling:  ESC │ ALT │ SHIFT │ TAB │ KBD
```

`PgUp` and `PgDn` are removed: touch scrolling covers their principal use and
they displace more important controls.  Home, End, function keys, macros, and
free-form key configuration are explicitly out of scope for this phase.

### Why the row must scroll

At the plan's own accessibility targets the full set does not fit:

```
10 keys x 44 dp = 440 dp   >   392 dp usable
10 keys x 48 dp = 480 dp   >   392 dp usable
```

Removing `PgUp` and `PgDn` does not close a 48 dp shortfall, and this is a large
phone.  The options are to shrink targets below the accessibility minimum, to
scroll, or to pin the keys that must never move and scroll the rest.  The third
is the only one that keeps both the 44 dp floor and direct reachability of the
arrows, so it is the design.

The pinned cluster is five keys at 48 dp = 240 dp, leaving about 150 dp --- three
keys at the minimum size --- visible in the scrolling region at rest, with the
remainder a short pan away.

### Key behavior

| Key | Behavior |
| --- | --- |
| `ESC`, `TAB`, and the four arrow keys | Send one terminal key event immediately.  They do not retain state. |
| `CTRL`, `ALT`, and `SHIFT` | Three-state: see below. |
| `KBD` | Shows or hides the Android soft keyboard.  It does not alter modifier state. |

Rev 1 specified modifiers that stay on until tapped again.  That is a footgun on
a touch screen: an accidental `CTRL` silently corrupts everything typed
afterwards, and the existing one-shot latch was chosen precisely so that a
mis-tap has a cheap escape.  Rev 2 adopts the conventional three-state model
instead:

* **tap** --- armed for one key event, then cleared;
* **long press or double tap** --- locked until cleared;
* **tap while locked** --- cleared.

Armed and locked must be visually distinct from each other and from off.  A
single "active" color, as rev 1 specified, cannot express the difference between
"this applies to the next key" and "this applies until you stop it".

Modifier state survives soft-keyboard visibility changes and opening the
sidebar, and must not leak across a change of active pane or tab.

Rev 1 said the state is per pane without saying what owns it.  Today it is a
single `Modifiers` on `TermWindow`.  Keying it by pane requires an entry
lifetime --- entries must be dropped when a pane closes, or the map accumulates
state for dead panes for the life of the window.  Per-pane is also worth
justifying rather than assuming: it means switching panes silently changes
modifier state, where per-window matches how an attached physical keyboard
behaves.  Decide this before phase 1 rather than during it.

### Touch and layout rules

* Every key has a minimum 44dp target; 48dp is preferred for the arrows and
  modifiers.  Meeting this makes the row taller than the 34dp it is today, and
  that cost is accepted.
* The pinned cluster is always visible and never pans.
* The scrolling region pans horizontally, clamped at both ends, and its position
  is preserved while the row is rebuilt for a latch change.
* The row retains theme-aware normal, pressed, armed, and locked colors.
* Pressed feedback must be immediate.  Haptics can be considered after the
  interaction model is verified on-device.
* Key widths are measured from the laid-out element, never estimated from label
  length.  The title font is proportional, so a character-count estimate
  understates the labels; because `min_width` is only a floor, the keys then grow
  past the estimate and the row silently overflows while the arithmetic insists
  it fits.  That defect is what made the last key unreachable.

## Architecture

### Rust UI and state

Add a sidebar state machine to `TermWindow`, with closed, overlay-open, and
pinned states.  Reuse the existing box model, `UIItem` hit testing, touch
handling, and GPU rendering used by the tab bar and current extra-keys row.

The sidebar renderer owns its geometry.  In overlay mode, it covers part of the
terminal surface.  In pinned mode, the terminal layout receives a reduced usable
rectangle rather than treating the full `ANativeWindow` as terminal content.

Any UI element that caches a computed element tree must be invalidated when the
glyph atlas is recreated, or it renders fragments of other glyphs.  The tab bar,
the modal, and now the key row do this; the sidebar must join them.

### Gesture ownership

The sidebar needs a scrollable host list, and the key row needs horizontal
panning.  Both require the gesture layer to know which region owns a drag, and
that layer currently carries hard-coded knowledge of the key row's height.  A
third such widget would mean a third special case.

Introduce a small region registry instead: the GUI publishes the rectangles that
claim gestures along with what each does with them, and the touch layer routes on
that.  Do this as part of the sidebar work rather than after it.

Regions are published as sizes, not absolute positions, and the backend places
them --- see below for why.

### Terminal viewport rectangle

This is the riskiest item in the plan and the reason pinned mode is scheduled
last.

`TermWindow::dimensions` is not reliably the size of the surface.  While the
client is attached to a remote pane it briefly holds the size the GUI would
*like* --- large enough for the remote pane's row and column count.  Attached to a
50-row pane, it read roughly 3960 px tall against a real surface of 2235 px, and
logcat shows the window declining to resize at all:

```
cannot resize window to match RowsAndCols { rows: 50, cols: 111 }
because window_state is FULL_SCREEN
```

An earlier attempt to place the key row's touch band from `dimensions` computed a
top edge of 3828 px on a 2235 px surface, so the band never matched a touch and
the row silently ignored every drag.  The fix was to hand the backend the row's
*height* and let it position the row against the surface it owns.

The pinned sidebar reduces a usable rectangle, which is the same class of
computation, so it will hit the same hazard.  Therefore:

* introduce an explicit terminal viewport rectangle, distinct from the window
  size, as the single input to grid recalculation and PTY resize;
* derive it from the surface the backend knows about, not from `dimensions`;
* treat "the window cannot resize" as normal on this platform rather than as a
  failure to be worked around.

### SSH host storage

Introduce an application-private `HostRepository` in Rust.  It stores a
versioned list of `HostProfile` records in `$HOME/.config/wezterm/hosts.toml`
or an equivalent private data file.  The exact format is an implementation
detail, but it must be atomic on write and reject invalid host names, ports,
and duplicate identifiers.

Note that this file is unreachable to the user on a release build: it lives in
app-private storage and `run-as` is refused when the package is not debuggable.
The UI is therefore the only editor it will ever have, which makes an export and
a reset path part of the feature rather than a nicety.

### Connecting a stored profile

Rev 1 left this as an open question.  It is answered: a profile can be connected
at runtime without touching `wezterm.lua` and without an app restart.

* `Mux::add_domain` (`mux/src/lib.rs:746`) registers a domain on a live mux, and
  `Mux::iter_domains` (`:1076`) enumerates what is registered.
* A `SshDomain` is a plain config struct that can be built at runtime and
  converted with `mux::ssh::ssh_domain_to_ssh_config`.
* `wezterm-client`'s `ssh_connect` is the client path, and is what the existing
  `ssh_domains` configuration already drives.

So the adapter is a conversion from `HostProfile` to `SshDomain` plus an
`add_domain` call.  The repository stays independent of `wezterm.lua`.

### Connection lifecycle and prompts

Connecting is not a single request that succeeds or fails.  Two interactive
prompts sit in the middle of it, and rev 1 accounted for neither.

* **Host key verification.**  `wezterm-ssh` raises `SessionEvent::HostVerify`
  (`wezterm-ssh/src/host.rs:157`) and **blocks the connection** until the user
  answers yes or no, then writes `known_hosts`.  Every first connection to a new
  host reaches this, so with a sidebar full of hosts it is the common path, not
  an edge case.  A mismatch raises `HostVerificationFailed` and must be presented
  as a warning that cannot be dismissed by accident.
* **Password and passphrase entry.**  Presented through the native dialog, for
  the reasons in the decision summary, with the field masked and excluded from
  autofill and clipboard.

Both need to be reachable while the sidebar is open, and neither may be silently
auto-answered.

### Kotlin dialogs and JNI boundary

The Rust sidebar requests a dialog through a narrow JNI API.  There are two
kinds: a host editor containing name, host, port, and user fields, and a
credential prompt containing a single masked field.  On submit the activity
returns a JSON payload to Rust; Rust validates and persists or consumes it, then
invalidates the terminal UI.  Cancellation has no effect beyond failing the
operation in progress.

The JNI contract uses request identifiers so that stale callbacks from a
recreated activity cannot modify a later operation.  Credential payloads must not
be logged, retained in Kotlin after the callback returns, or written to the host
profile.

## Prerequisites

One existing defect must be fixed before the connect flow is worth building.

**A failed connection terminates the app.**  With the remote unreachable, the
process exits within about twenty seconds of launch and shows nothing; only the
teardown abort appears in logcat.  This is survivable today, when a remote
`default_domain` is a deliberate configuration choice, but a sidebar makes
failed connections routine.  Fix this before or with phase 2, and treat it as a
bug rather than as one of the connection states phase 5 renders.

## Implementation phases

1. **Extra-keys state and layout**
   * Decide per-pane versus per-window modifier ownership, and the entry
     lifetime if per-pane.
   * Separate momentary keys from three-state modifier keys; merge the modifier
     mask at the common key dispatch point, covering IME commits, on-screen key
     actions, and physical keyboard events.
   * Split the row into the pinned cluster and the scrolling remainder; remove
     `PgUp` and `PgDn`; raise targets to 44--48dp.
   * Add visual states for armed and locked, and regression tests for IME and
     physical-keyboard input.

2. **Host model, repository, and connect path**
   * Define `HostProfile`, validation, CRUD operations, and atomic private
     storage, with export and reset.
   * Read existing configured SSH domains for display without editing them.
   * Implement the `HostProfile` to `SshDomain` adapter over `Mux::add_domain`.
   * Fix the exit-on-failed-connection defect.

3. **Prompts and the native dialog boundary**
   * Add the JNI request/callback interface with request identifiers.
   * Implement the host editor and the credential prompt.
   * Wire host key verification and password/passphrase prompts to it.
   * Surface validation errors without losing unsaved input.

4. **Rust-rendered sidebar, overlay only**
   * Implement the state machine, tab-bar entry point, overlay, and close
     behavior.
   * Introduce the gesture region registry and render a scrollable host list
     with add, edit, delete, and connect actions.
   * Invalidate the sidebar's cached element tree on atlas recreation.

5. **Pinned mode**
   * Introduce the terminal viewport rectangle as the single input to grid
     recalculation and PTY resize, sourced from the surface rather than from
     `dimensions`.
   * Implement pinned width and the reduced usable rectangle on top of it.

6. **Device verification**
   * Test portrait, landscape, tablets, split screen, rotation, and keyboard
     visibility.
   * Test Ctrl+C, Ctrl+D, Alt combinations, and armed versus locked modifiers
     with both IME and Bluetooth keyboards.
   * Test first-connect host key verification, a deliberately mismatched host
     key, and a failed connection.
   * Test sidebar transitions while output is active and while the app is
     backgrounded, then validate terminal and PTY resize behavior in pinned
     mode.

Phases 1 and 2 are independent of each other.  Phase 5 is separated from phase 4
because it is the only part that touches window geometry, and it should not be
able to delay a working overlay sidebar.

## Out of scope

This plan does not include a fully native Android navigation shell, arbitrary
bottom-row key configuration, persisted passwords, a Storage Access Framework
file picker for keys, or edge-swipe drawer gestures.  Each can be added later
without changing the core sidebar, host repository, or key-state architecture
described here.
