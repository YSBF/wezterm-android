# Android terminal sidebar and extra-keys plan

## Status

**Design proposal.** This document describes the next user-interface layer for
the Android port: an in-terminal SSH host sidebar and a more usable fixed
extra-keys row.  It does not change the existing terminal renderer, mux, or
Android activity architecture.

## Decision summary

The terminal sidebar and extra-keys row will remain Rust-rendered UI inside
WezTerm's existing GPU surface.  This preserves a single coordinate system for
the terminal, tabs, touch input, window resizing, and rendering.

Editing an SSH host is different: it needs ordinary form fields and reliable
IME editing.  The Android `WezTermActivity` will present a small native Kotlin
dialog for add/edit operations, then return a validated host record to Rust
through JNI.  This avoids reimplementing text fields, selection, cursor
handling, and accessibility in the renderer.

We will not put a `DrawerLayout` or Compose drawer around the existing
`GameActivity` surface.  Doing so would introduce surface z-order, gesture
dispatch, and terminal-size synchronization problems.  A native edit dialog
is isolated from those concerns, while the sidebar itself remains part of the
terminal UI.

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
│ ESC │ CTRL │ ALT │ SHIFT │ TAB │ ← ↑ ↓ → │ KBD │
└─────────────────────────────────────────────┘
```

### Sidebar modes

* **Phone portrait:** the sidebar is an overlay drawer from the left, around
  80% of the available width.  Opening or closing it does not resize the
  terminal grid.
* **Landscape and tablets:** the user can pin the sidebar.  A pinned sidebar
  is 300--360dp wide, the terminal's usable width is reduced by that amount,
  and the active pane is resized accordingly.
* **Entry and dismissal:** a menu item in the tab bar opens the sidebar.  The
  first implementation supports explicit open/close controls and a dimmed
  overlay; edge-swipe gestures are deferred.

### Sidebar contents

The first release is deliberately limited to simple SSH profiles:

* display name;
* host name or IP address;
* port, defaulting to `22`;
* user name;
* an optional existing key reference or existing WezTerm SSH-domain reference;
* connect, edit, and delete actions.

The sidebar also lists SSH domains supplied by `wezterm.lua`, marked as
configuration-file entries.  They are read-only in the first release.  Hosts
created in the app are managed separately.

Passwords must not be saved in the host profile.  Password authentication is
entered at connection time.  Any future persistent secret support must use
Android Keystore rather than a plaintext configuration file.

## Extra-keys row

The current Android row is functionally correct, but it is deliberately
minimal: it has a fixed sequence of keys, small arrow targets, and one-shot
modifier latches.  The new row remains fixed rather than user-configurable,
but changes its layout and modifier semantics.

### Fixed key set

```
ESC | CTRL | ALT | SHIFT | TAB | Left | Up | Down | Right | KBD
```

`PgUp` and `PgDn` are removed: touch scrolling covers their principal use and
they displace more important controls.  Home, End, function keys, macros, and
free-form key configuration are explicitly out of scope for this phase.

### Key behavior

| Key | Behavior |
| --- | --- |
| `ESC`, `TAB`, and the four arrow keys | Send one terminal key event immediately.  They do not retain state. |
| `CTRL`, `ALT`, and `SHIFT` | Toggle persistent on/off state.  An enabled modifier applies to subsequent IME, on-screen, and physical-keyboard input until it is tapped again. |
| `KBD` | Shows or hides the Android soft keyboard.  It does not alter modifier state. |

Modifier state is per terminal pane.  It survives soft-keyboard visibility
changes and opening the sidebar, but must not leak when the active pane or tab
changes.  A visibly active color is required for every enabled modifier.

### Touch and layout rules

* Every normal key has a minimum 44dp target; 48dp is the preferred width and
  height for the arrows and modifiers.
* The arrow cluster is fixed and directly reachable.  It must not shrink to
  label width or be moved out of view by horizontal scrolling.
* On a narrow screen, only non-core controls may overflow into a horizontal
  scroller.  The initial fixed key set should fit typical phone widths without
  scrolling.
* The row retains theme-aware normal, pressed, and modifier-active colors.
* Pressed feedback must be immediate.  Haptics can be considered after the
  interaction model is verified on-device.

## Architecture

### Rust UI and state

Add a sidebar state machine to `TermWindow`, with closed, overlay-open, and
pinned states.  Reuse the existing box model, `UIItem` hit testing, touch
handling, and GPU rendering used by the tab bar and current extra-keys row.

The sidebar renderer owns its geometry.  In overlay mode, it covers part of
the terminal surface.  In pinned mode, the terminal layout receives a reduced
usable rectangle rather than treating the full `ANativeWindow` as terminal
content.  This must drive both grid recalculation and PTY resize.

Replace the current one-shot `key_row_latched` behavior with a persistent
per-pane modifier mask.  All keyboard paths must merge that mask at the common
Rust key-event dispatch point, including IME commits, on-screen key actions,
and physical keyboard events.

### SSH host storage

Introduce an application-private `HostRepository` in Rust.  It stores a
versioned list of `HostProfile` records in `$HOME/.config/wezterm/hosts.toml`
or an equivalent private data file.  The exact format is an implementation
detail, but it must be atomic on write and reject invalid host names, ports,
and duplicate identifiers.

The repository is independent of `wezterm.lua`.  Connecting a stored profile
requires a runtime path that converts it to the equivalent of an SSH domain or
client configuration; it must not rewrite `wezterm.lua` or require an app
restart.  The relevant mux/domain APIs need to be identified before the UI is
wired to the Connect button.

### Kotlin edit dialog and JNI boundary

The Rust sidebar requests an add or edit dialog through a narrow JNI API.  The
Kotlin activity presents a native dialog containing name, host, port, and user
fields.  On submit it returns a JSON host draft to Rust; Rust validates and
persists it, then invalidates the terminal UI.  Cancellation has no effect.

The JNI contract should use request identifiers so that stale callbacks from a
recreated activity cannot modify a later edit operation.

## Implementation phases

1. **Extra-keys state and layout**
   * Separate momentary keys from persistent modifier keys.
   * Remove `PgUp` and `PgDn`.
   * Give arrows and modifiers fixed, accessible target sizes.
   * Keep modifier state per pane and merge it at common key dispatch.
   * Add visual state and regression tests for IME and physical-keyboard input.

2. **Host model and repository**
   * Define `HostProfile`, validation, CRUD operations, and atomic private
     storage.
   * Read existing configured SSH domains for display without editing them.
   * Define the runtime connection adapter before exposing Connect in the UI.

3. **Rust-rendered sidebar**
   * Implement the state machine, tab-bar entry point, overlay, and close
     behavior.
   * Render a scrollable host list with add, edit, delete, and connect actions.
   * Implement pinned width and terminal usable-rectangle/PTY resize support.

4. **Native host editor**
   * Add the JNI request/callback interface.
   * Implement the Kotlin dialog and lifecycle-safe callback handling.
   * Surface validation errors in the sidebar without losing unsaved input.

5. **Connection lifecycle**
   * Launch the selected stored host through WezTerm's runtime SSH/mux path.
   * Show connecting, connected, authentication-required, and failed states.
   * Preserve existing local panes and remote-domain behavior.

6. **Device verification**
   * Test portrait, landscape, tablets, split screen, rotation, and keyboard
     visibility.
   * Test Ctrl+C, Ctrl+D, Alt combinations, and modifier toggles with both
     IME and Bluetooth keyboards.
   * Test sidebar transitions while output is active and while the app is
     backgrounded, then validate terminal and PTY resize behavior in pinned
     mode.

## Out of scope

This plan does not include a fully native Android navigation shell, arbitrary
bottom-row key configuration, persisted passwords, a file picker for keys, or
edge-swipe drawer gestures.  Each can be added later without changing the core
sidebar, host repository, or key-state architecture described here.
