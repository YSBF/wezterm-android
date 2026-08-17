# Android terminal sidebar and extra-keys plan

## Status

**Design proposal, revision 3.** This document describes the next user-interface
layer for the Android port: an in-terminal SSH host sidebar and a more usable
extra-keys row.  It does not change the existing terminal renderer, mux, or
Android activity architecture.

## Revision history

* **rev 1** (`6334bc7cb`) --- the proposal as originally written.
* **rev 2** (`b879893cf`) --- reconciled with the extra-keys behavior that has
  since shipped; corrected the key-row sizing rules, which rev 1 stated in a
  form that cannot be satisfied on a phone; added the credential and host-key
  prompts the connect flow requires; answered rev 1's open question about the
  runtime domain API; and recorded the window-geometry hazard that the pinned
  sidebar will hit.
* **rev 3** --- corrected rev 2's connect path, which named the wrong mechanism;
  specified domain lifecycle across edit and reconnect, which no revision had
  addressed; settled modifier ownership and the armed-to-locked transition,
  which rev 2 left contradictory and incomplete; and added the clipping and
  hit-test work that a pinned cluster requires and rev 2 assumed away.

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
button, so it must be accounted for in however the bar divides its width rather
than drawn over the result.

Note that the change which makes the bar share its width between tabs rather
than squashing them (`d801161fb`) is **not** on this branch --- it currently
lives only on `fix-remote-tab-focus-pingpong`.  Whether the menu button lands in
that division or in the older squashing layout depends on whether the branches
have merged by phase 4.  Check before implementing rather than assuming.

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

The decision is **accept a pasted key**: the native dialog gains a multi-line
field, and Rust writes the key into app-private storage with `0600`.  This needs
no Storage Access Framework picker and no new permission, and it keeps key
authentication --- the only form worth using against a real server --- available
in the first release.  A file picker remains out of scope.

This does pull a private key through the clipboard, which is in tension with the
credential dialog's rule against clipboard use.  The tension is accepted, because
the alternative is no key support at all, but it must be handled rather than
ignored: on Android 13 and later the clipboard is surfaced in a system preview
and retained in clipboard history, and a cloud-syncing IME may see it too.  So
the import field clears the clipboard after a successful import, marks itself
sensitive so the system preview does not display the contents, and the UI states
plainly that the key passed through the clipboard.  Generating a key on-device,
which avoids the clipboard entirely, is the better long-term answer and is out of
scope here.

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

As an estimate, the pinned cluster is five keys at 48 dp = 240 dp, leaving about
150 dp --- roughly three keys at the minimum size --- visible in the scrolling
region at rest, with the remainder a short pan away.

That arithmetic is an estimate and nothing may depend on it, for the same reason
the row measures itself: 48 dp is a floor, and a key whose label needs more grows
past it.  The boundary between the pinned cluster and the scrolling region is
computed from the measured width of the pinned keys at layout time, never from
the nominal target.

### Key behavior

| Key | Behavior |
| --- | --- |
| `ESC`, `TAB`, and the four arrow keys | Send one terminal key event immediately.  They do not retain state. |
| `CTRL`, `ALT`, and `SHIFT` | Three-state: see below. |
| `KBD` | Shows or hides the Android soft keyboard.  It does not alter modifier state. |

Rev 1 specified modifiers that stay on until tapped again.  That is a footgun on
a touch screen: an accidental `CTRL` silently corrupts everything typed
afterwards, and the existing one-shot latch was chosen precisely so that a
mis-tap has a cheap escape.  The model is three-state, and **each state is
reached by the same tap**:

```
off  --tap-->  armed  --tap-->  locked  --tap-->  off
                 |
                 +-- consumed by one key event --> off
```

Rev 2 assigned locking to a long press or double tap and left a tap on an
already-armed modifier undefined, which is the ambiguity that turns into an
argument on the device.  Cycling removes it, and costs nothing:

* the touch layer has **no** double-tap recognition today, and adding it means
  delaying the dispatch of every tap by the double-tap interval, which is a real
  latency regression on the most common interaction in the row;
* long press in the row is already spoken for --- see the gesture section --- so
  overloading it for lock would need that conflict resolved first.

Armed and locked must be visually distinct from each other and from off.  A
single "active" color, as rev 1 specified, cannot express the difference between
"this applies to the next key" and "this applies until you stop it".

#### Ownership

Rev 1 said the state is per pane.  Rev 2 required that it "must not leak across
a change of active pane or tab" and then reopened per-pane versus per-window as
undecided, which is a contradiction: the requirement is the decision.

It is settled here as **one mask per window, cleared when the active pane or tab
changes**.  It stays a single `Modifiers` on `TermWindow`, as today.

This satisfies the no-leak requirement exactly, and it is strictly better than a
per-pane map for this feature:

* no map, so no entry lifetime and no accumulation of state keyed by panes that
  have closed;
* clearing on focus change is also the safer default under the mis-tap
  philosophy above --- an armed modifier the user has forgotten about does not
  survive a context switch;
* it matches how an attached physical keyboard behaves, where per-pane modifier
  state would be surprising.

Modifier state survives soft-keyboard visibility changes and opening the
sidebar; only a change of active pane or tab clears it.

### Touch and layout rules

* Every key has a minimum 44dp target; 48dp is preferred for the arrows and
  modifiers.  Meeting this makes the row taller than the 34dp it is today, and
  that cost is accepted.
* The pinned cluster is always visible and never pans.
* The scrolling region pans horizontally, clamped at both ends, and its position
  is preserved while the row is rebuilt for a modifier state change.
* **The scrolling region must be clipped to its own rectangle.**  The row that
  shipped pans by shifting the first key's left margin and lets the window edges
  do the clipping, which needs no scissor rect because the only overflow is off
  the screen.  A pinned cluster breaks that: a key panned to the left of the
  boundary has nowhere to go and will be drawn on top of, or underneath, the
  pinned keys.  The scrolling region therefore needs a real clip, which is the
  one genuinely new rendering mechanism in this phase.
* Hit testing must respect the same boundary.  UI items are resolved by taking
  the last match in the list, so a scrolled key that has slid under a pinned key
  will otherwise steal its taps --- a `CTRL` that intermittently types `ESC` is
  the exact failure this prevents.
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

Introduce a small region registry: the GUI publishes the regions that claim
gestures along with what each does with them, and the touch layer routes on that.

**This is needed in phase 1, not phase 4.**  Long press is currently unconditional
--- `poll_long_press` begins a text selection wherever the finger is, with no
notion of region --- so a long press on the extra-keys row starts selecting
terminal text behind a strip of buttons.  That is wrong today, and phase 1 cannot
land a correct row without fixing it.  The registry is the fix; the sidebar is
its second consumer, not its first.

A region declares:

* an **anchor edge** and its extent along both axes.  Rev 2 said regions are
  published as sizes rather than positions, which is true of a bottom-anchored
  full-width row and not general: the sidebar is left-anchored, has a width *and*
  a vertical extent, and needs to state whether it begins below the tab bar and
  ends above the key row.  The backend still does the placing --- see the
  viewport section for why --- but it needs the anchor to do it.
* which gestures it **claims** (drag along an axis, long press, tap) and which it
  **declines**, so that a declined gesture falls through to the terminal rather
  than being swallowed.
* a **priority**, because regions overlap: an open overlay sidebar sits above the
  terminal and above part of the key row.

An open sidebar is not only its own rectangle.  The dimmed area outside it must
route taps to "close", so opening the sidebar changes gesture routing for the
whole surface, not just for the drawer.  Model that as a full-surface region at
a lower priority than the drawer itself, published while the sidebar is open.

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

Export is a share intent (`ACTION_SEND`) carrying the serialized host list,
which needs no storage permission and no file picker, and lets the user send it
to wherever they keep things.  It must exclude anything secret: profiles only,
never a key or a password.  Reset deletes the file and reloads an empty
repository.

### Connecting a stored profile

Rev 1 left this open; rev 2 answered it with the wrong mechanism.  There are two
distinct SSH paths and they are not interchangeable:

| `SshDomain.multiplexing` | Domain type | Requires on the remote host |
| --- | --- | --- |
| `None` | `RemoteSshDomain` (`mux/src/ssh.rs:180`) | nothing but an sshd |
| `WezTerm` | `ClientDomain`, via `wezterm-client`'s `ssh_connect` | a `wezterm` binary, run as `wezterm cli proxy` |

Rev 2 cited `ssh_connect` as "the client path".  That is the multiplexing
flavor: it computes `wezterm_bin_path` and executes `wezterm cli proxy` on the
far end, so it fails against an ordinary server.  A sidebar host profile is an
ordinary SSH login and **must** map to `multiplexing: SshMultiplexing::None`.

`update_mux_domains_impl` (`wezterm-mux-server-impl/src/lib.rs:39`) is the
existing precedent for registering both flavors from configuration, and
`run_ssh` (`wezterm-gui/src/lib.rs:178`) is the minimal example of the plain
one:

```rust
let domain = Arc::new(mux::ssh::RemoteSshDomain::with_ssh_domain(&dom)?);
mux.add_domain(&domain);
```

So the adapter is a conversion from `HostProfile` to `SshDomain` with
`multiplexing: None`, then `RemoteSshDomain::with_ssh_domain` and
`Mux::add_domain` (`mux/src/lib.rs:746`).  No `wezterm.lua` rewrite and no app
restart.  A future "remote WezTerm mux" profile type can opt into the other row
of that table, but it is not the first release.

#### Domain lifecycle

Registration is not reversible and not idempotent in the way the UI needs.

* `Mux` has `add_domain` and `get_domain_by_name` (`:742`) but **no removal**.  A
  domain lives for the life of the process.
* `update_mux_domains_impl` guards with `if mux.get_domain_by_name(..).is_some()
  { continue; }`.  Registration keyed by name is therefore first-write-wins, and
  silently so.

The consequence is a defect waiting to be built: connect to a host, disconnect,
edit its port, reconnect --- and the second connection uses the *first*
domain's configuration, because the name already exists and the new config is
dropped on the floor.  Nothing reports this.

The first release resolves it by never reusing a name.  A `HostProfile` carries
a stable identifier and a generation counter; the domain name is derived from
both, so editing a profile yields a name that has never been registered.  The
sidebar displays the profile's display name, not the domain name, so this stays
invisible.

The cost is that dead domains accumulate for the life of the process.  That is
acceptable for a first release --- they are small, and the alternative is a
removal API upstream in `Mux` --- but it must be a recorded decision rather than
an accident, and a long-lived session that edits profiles repeatedly is the case
to watch during phase 6.  Adding `Mux::remove_domain` upstream is the eventual
fix and is out of scope here.

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
   * Introduce the gesture region registry, and use it to stop long press inside
     the row from beginning a text selection.  This is a prerequisite for the
     rest of the phase, not a later tidy-up.
   * Separate momentary keys from cycling modifier keys (off, armed, locked);
     keep one mask per window and clear it on pane or tab focus change; merge it
     at the common key dispatch point, covering IME commits, on-screen key
     actions, and physical keyboard events.
   * Split the row into the pinned cluster and the scrolling remainder, with the
     boundary taken from measured widths; clip the scrolling region and order
     hit testing so a scrolled key cannot take a tap meant for a pinned one.
   * Remove `PgUp` and `PgDn`; raise targets to 44--48dp.
   * Add visual states for armed and locked, and regression tests for IME and
     physical-keyboard input.

2. **Host model, repository, and connect path**
   * Define `HostProfile`, validation, CRUD operations, and atomic private
     storage, with export and reset.
   * Read existing configured SSH domains for display without editing them.
   * Implement the `HostProfile` to `SshDomain` adapter with
     `multiplexing: None`, over `RemoteSshDomain` and `Mux::add_domain`, with
     generation-derived domain names so an edited profile never collides with
     the domain its previous version registered.
   * Fix the exit-on-failed-connection defect.

3. **Prompts and the native dialog boundary**
   * Add the JNI request/callback interface with request identifiers.
   * Implement the host editor and the credential prompt.
   * Wire host key verification and password/passphrase prompts to it.
   * Surface validation errors without losing unsaved input.

4. **Rust-rendered sidebar, overlay only**
   * Implement the state machine, tab-bar entry point, overlay, and close
     behavior.
   * Extend the gesture registry from phase 1 with anchor edges, priority, and
     the full-surface tap-to-close region; render a scrollable host list with
     add, edit, delete, and connect actions.
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
   * Edit a connected profile's port and reconnect, confirming the new
     configuration is used rather than the domain registered by the previous
     version, and watch domain accumulation across a long session of repeated
     edits.
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
