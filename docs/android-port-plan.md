# Android GUI port plan

Status: **all phases implemented and run on a device.**

Every phase below is written, cross-compiles, and has now been exercised on a
Xiaomi phone running Android 15 (MIUI, `aarch64`, 1080x2400). The per-phase
exit criteria below were written before any of it had run; what actually
happened on the device, including the six bugs it turned up, is recorded under
"What the device changed" and "What remains" at the end.

The goal is a native Android GUI backend for wezterm — a fourth sibling to
`x11`, `wayland`, `macos` and `windows` under `window/src/os/` — reusing the
existing renderer rather than reimplementing the terminal in Kotlin.

This also subsumes the earlier "remote mux client on a phone" idea: because
`wezterm-client` runs in-process, a native app gets both local panes *and*
`wezterm connect`, with client-side local echo and scrollback caching intact.

## What has already been established

A cross-compile probe was run against `aarch64-linux-android` (NDK 28,
API level 28). Result: **the entire GUI compiles and links into an Android
ARM64 executable.**

```
target/aarch64-linux-android/debug/wezterm-gui:
ELF 64-bit LSB pie executable, ARM aarch64, for Android 28,
interpreter /system/bin/linker64, built by NDK r28
NEEDED: libandroid.so, libdl.so, libc.so, libm.so
```

Four Android-native libraries, nothing else to ship.

Total cost: **38 insertions, 10 deletions across 9 files, plus a 140-line stub
backend.** Everything else — freetype, harfbuzz, zlib, libpng, cairo,
mlua/lua54, libssh, libssh2, openssl, and all ~37k lines of `wezterm-gui` —
cross-compiled unmodified. `portable-pty` and `termwiz` needed no changes at
all.

The probe's changes have since been superseded by the real port; what follows
records why each one was needed, because every one of them is a trap that is
easy to fall into again.

### What the probe did *not* prove

It proved the binary **links**, not that it **runs**. Every method in the stub
backend was `todo!()`, and the process aborted before reaching any of them:

```rust
// config/src/lib.rs:69
pub static ref HOME_DIR: PathBuf = dirs_next::home_dir().expect("can't find HOME dir");
```

`$HOME` is unset in an Android app process, so this panics during config
initialisation — long before EGL is touched. `compute_cache_dir`,
`compute_data_dir` and `compute_runtime_dir` (`config/src/config.rs:1753-1775`)
all fall back to `HOME_DIR` too.

The mitigating detail: `HOME_DIR` is a `lazy_static`, so setting `HOME` before
anything first touches `config` is sufficient and needs no change to `config`
itself. That makes it an entry-point concern, and it is what
`wezterm-gui/src/android/mod.rs` now does.

Assume there are more assertions like this on the startup path; `HOME_DIR` is
simply the first one that was *found*, and it was found by reading, not by
running. See "What remains" for how to flush out the rest cheaply.

### Why the GUI ports at all

`wezterm-gui` is already platform-agnostic. Grepping `target_os` across all of
`wezterm-gui/src` returns six hits, all trivial macOS keyboard/DPI trivia, and
the crate declares exactly one platform-specific dependency block (Windows).
The renderer, tab bar, overlays, box model and glyph cache port unchanged.

Note that `wezterm-gui/src/unicode_names.rs` is 140,378 of the crate's 177,518
lines and is a generated table. Real GUI code is ~37k lines.

The backend contract is small: `WindowOps` has 11 methods with no default body,
`ConnectionOps` has 3. Existing backends run 4,194–6,051 lines each.

`window/src/egl.rs` (724 lines) is shared and already EGL-based, which is what
Android uses natively. `raw-window-handle` 0.6 already has
`RawWindowHandle::AndroidNdk`.

## Reproducing the build

This is now recorded in `ci/android-env.sh`, which Gradle also sources so that
there is one definition of the environment:

```bash
rustup target add aarch64-linux-android
export ANDROID_NDK_HOME=$HOME/Application/Android_SDK/ndk/28.0.12674087

. ci/android-env.sh
cargo build --target aarch64-linux-android -p wezterm-android \
      --no-default-features --features vendored-fonts
```

For the APK, `cd android && ./gradlew installDebug`.

`--no-default-features` matters: the default feature set includes `wayland`,
which turns on `window/wayland`.

Two traps worth recording:

- **Build in a git worktree needs submodules seeded.** `deps/freetype/build.rs`
  checks for `zlib/.git` and shells out to `git submodule update --init` if
  absent. In a fresh worktree this hangs on network/credentials with no output.
  Copy `deps/freetype/{freetype2,libpng,zlib}` and `deps/harfbuzz/harfbuzz`
  from a populated checkout, or set `GIT_TERMINAL_PROMPT=0` to fail fast.
- **Do not build under `/tmp`** if it is a tmpfs. The target directory reaches
  ~7 GB and will exhaust RAM.

## The four changes needed to build ✅

All four are implemented as described. The reasoning is kept here because each
one is a trap that is easy to fall into again.

### 1. openssl must be vendored — for Android only

`openssl-sys` looks for a host openssl via pkg-config and cannot cross-compile.

The probe took the blunt path — flipping the workspace dependency in the root
`Cargo.toml` — but **do not do that**: it switches every platform to vendored
openssl, so desktop builds stop using the system library and pick up a slower
build and a separate CVE-patching burden.

The tree already has the right pattern. `async_ossl/Cargo.toml:14-18`:

```toml
[target.'cfg(not(any(windows, target_os="macos")))'.dependencies]
openssl.workspace = true

[target.'cfg(any(windows, target_os="macos"))'.dependencies]
openssl = { workspace = true, features=["vendored"] }
```

Add `target_os="android"` to that second cfg (and to the negation in the
first). Because the workspace uses `resolver = "2"`, features are unified
per-target, so enabling `openssl/vendored` on the Android target alone turns on
`openssl-sys/vendored` for that target — which also satisfies `libssh-rs-sys`
and `libssh2-sys`, since they link whatever `openssl-sys` produces. Host builds
are untouched.

Note `[workspace.dependencies]` itself cannot be made target-conditional; the
cfg has to live in the consuming crate, which is why `async_ossl` is the right
place.

### 2. Android must be excluded from the X11/Wayland path

Android is `target_family = "unix"`, so `window/src/os/mod.rs` currently routes
it straight into X11 and the `x11` crate's build script dies looking for a host
X11 via pkg-config. Every `cfg(all(unix, not(target_os = "macos")))` gate in
`window/` needs `not(target_os = "android")` added, in both `window/Cargo.toml`
and `window/src/os/mod.rs`, plus a new `#[cfg(target_os = "android")] pub mod
android;` arm.

### 3. `starship-battery` has no Android backend

It fails with `compile_error!("Support for this target OS is not implemented
yet!")`. Used only by `lua-api-crates/battery`, which exposes
`wezterm.battery_info` to Lua. Gate the dependency off Android and return an
empty vec. (A real implementation would read `/sys/class/power_supply` or go
through `BatteryManager` over JNI.)

### 4. fontconfig cannot be linked on Android

`wezterm-font/Cargo.toml:43` is the one place upstream mentions Android, and it
is exactly what breaks the build:

```toml
[target.'cfg(any(target_os = "android", all(unix, not(target_os = "macos"))))'.dependencies]
fontconfig.workspace = true
```

`deps/fontconfig/build.rs` resolves fontconfig via pkg-config and **silently
does nothing when it is not found** — there is a commented-out
`panic!("no fontconfig")`. So the Rust bindings compile against a library that
was never linked, and the failure only appears at link time as 20 undefined
`Fc*` symbols.

Fix: drop Android from that target block, gate `wezterm-font/src/fcwrap.rs` and
`wezterm-font/src/locator/font_config.rs` off Android, and make
`FontLocatorSelection::default()` (`config/src/font.rs:677`) return
`ConfigDirsOnly` on Android. `ConfigDirsOnly` already exists and maps to
`NopSystemSource`, and `vendored-fonts` is on by default, so the binary ships
with JetBrains Mono, Nerd Font symbols, Roboto and Noto Emoji compiled in.

This was the stopgap; Phase 7 replaced it with a real system font source, and
`FontLocatorSelection::Android` is now the default there.

## Upstreamable independently of Android ✅

One finding was worth a PR regardless of whether this port ever ships, and is
now implemented as an optional `ssh` feature on `config`, on by default.

`config` depends on `wezterm-ssh` non-optionally, and its entire use is
`config/src/ssh.rs:112` — three calls that parse `~/.ssh/config` to enumerate
host names for auto-generated SSH domains:

```rust
let mut config = wezterm_ssh::Config::new();
config.add_default_config_files();
for host in config.enumerate_hosts() { ... }
```

Because everything depends on `config`, that one text-parsing call site pulls
openssl + libssh + libssh2 into anything that only wanted to read
`wezterm.lua`.

One correction to the original claim: it is *not* every binary in the tree.
`mux` and `wezterm-client` depend on `wezterm-ssh` in their own right, so the
GUI and the mux server still link it regardless. What benefits is anything
wanting `config` without the mux — `sync-color-schemes` here, and any
downstream embedding of the crate.

## Phased plan

Every phase is implemented, and every exit criterion below has now been met on
a device. They are stated as originally written; the fixes the device forced
are listed under "What the device changed".

### Phase 0 — cross-compile and link ✅

Done. The four build changes are in, each scoped to the Android target so no
other platform is affected; `ci/android-env.sh` records the cross-compile
environment.

### Phase 1 — APK shell, entry point, and environment bootstrap ✅

`wezterm-gui/src/main.rs` became `src/lib.rs`, and `wezterm-android` is a
`cdylib` shim exporting `android_main`.

GameActivity was chosen over NativeActivity, and that decision is now baked
into the manifest and the input layer. It was not a neutral choice:
NativeActivity's soft-keyboard support is too thin for a terminal, while
GameActivity ships GameTextInput, whose commit and composing-text events are
what Phase 5's IME handling is built on. Note that the upstream GameActivity
C++ 'prefab' glue must *not* be enabled; android-activity supplies its own and
the two are incompatible.

`wezterm-gui/src/android/mod.rs` sets `HOME`, `XDG_CONFIG_HOME`,
`XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, `TMPDIR`, `TERM`, `LANG`,
`PATH` and `SHELL` before anything touches `config`, and routes `log` to logcat
first of all so that the startup path is diagnosable.

On editing `wezterm.lua` on device: the config lives at
`$HOME/.config/wezterm/wezterm.lua` inside the app's private files directory.
For development that is reachable with `adb shell run-as`. A scoped-storage or
SAF import flow, or an in-app editor, is still needed before this is usable by
anyone who is not holding a USB cable — that is unbuilt.

Exit criterion (verified): APK installs and launches to a blank surface, and
gets past config initialisation.

### Phase 2 — EGL surface and first frame ✅

`Connection::create_new` binds to `AndroidApp`; `Window::window_handle` returns
`RawWindowHandle::AndroidNdk`; `enable_opengl` goes through the shared
`crate::egl::GlState` with `EGL_DEFAULT_DISPLAY`.

`enable_opengl` waits for the first `InitWindow` rather than failing, because
`TermWindow::new` may reach it before Android has produced a surface.

Exit criterion (verified): the tab bar and a cell grid render on screen.

### Phase 3 — lifecycle and surface loss ✅

This turned out smaller than feared, and the reason is worth recording: an
`EGLContext` **survives** the destruction of its `EGLSurface`. Textures,
programs and therefore the entire glyph atlas persist across a background,
rotate or configuration change. Only the surface has to be swapped.

So rather than giving `TermWindow` a context-rebuild path, `window/src/egl.rs`
gained `release_surface`/`rebuild_surface`, which destroy and recreate only the
`EGLSurface` against the retained `EGLConfig`. `TermWindow` is untouched, and
`gl: Option<Rc<Context>>` never has to become `None`.

The loop inversion is handled by driving `AndroidApp::poll_events` from
`run_message_loop`, which also has to service the spawn queue and
invalidation-driven painting. The spawn queue signals a pipe rather than the
`ALooper`, so a watcher thread translates pipe readability into wakeups; that
is less invasive than teaching `window/src/spawn.rs` about Android.

Exit criterion (verified): rotate the device and switch apps briefly —
rendering resumes on the recreated surface.

### Phase 4 — process lifetime, PTY, and exec ✅

#### 4a. Process lifetime

`MuxService` is a foreground service with an ongoing notification, a stop
action, and a partial `WakeLock` so that long-running commands are not
suspended when the screen goes off. `POST_NOTIFICATIONS` is requested at
startup on API 33+; a denial is not fatal.

#### 4b. PTY and exec

The machinery is in place; the binaries are not. `wezterm-gui/src/android/prefix.rs`
links every `lib<name>.so` in the native library directory to `<name>` in
`$HOME/.local/bin` and puts that first on `PATH`, asking multi-call binaries
for their applet lists. `SHELL` resolves to the best bundled shell, falling
back to `/system/bin/sh` — note that `/bin/sh`, which `portable-pty` reaches
for, does not exist on Android.

`android/app/src/main/prefix/<abi>/` is where prebuilt binaries go. Building a
static bash and busybox for Android is a separate exercise and has not been
done, so today the terminal runs whatever toybox provides in `/system/bin`.

Reusing an installed Termux prefix remains not viable; `android/README.md`
records why, so that nobody spends time on it again.

Exit criterion (verified): an interactive shell in a pane, correct on resize.

### Phase 5 — input ✅

`window/src/os/android/keyboard.rs` maps Android keycodes, consulting the
originating device's `KeyCharacterMap` so that non-US physical keyboards work,
and handling dead keys via `get_dead_char`. Ctrl is masked out of the character
map lookup so that Ctrl-A still resolves to `a`.

Soft keyboard text arrives separately, as whole-buffer GameTextInput updates
with a composing region. Committed text is recovered by diffing against the
previously seen buffer; text still inside the composing region is reported as
composition status rather than being sent to the pty.

The extra-keys row is `wezterm-gui/src/termwindow/keyrow.rs`, built from the
existing box model. Modifiers latch, and the latch is applied in
`key_event_impl` — the single point every key press passes through — which is
what lets Ctrl apply to a character that arrived from an IME with no modifiers
of its own.

### Phase 6 — clipboard ✅

JNI to `android.content.ClipboardManager`, marshalled onto the Java main
thread. `coerceToText` rather than `getText`, so URIs and intents paste as
text. `PrimarySelection` aliases the single Android clipboard.

Unvalidated: Android 13+ shows a system copy confirmation and truncates large
clips. Terminal-sized selections have not been tested against that.

### Phase 7 — fonts and CJK ✅

`wezterm-font/src/locator/android.rs` enumerates `/system/fonts` and its
siblings and parses `/system/etc/fonts.xml` for family and fallback ordering,
which is what makes CJK and emoji fallback work. Parsing is tolerant of all
three historical formats and of vendor variation, and degrades to plain
directory enumeration rather than failing. `FontLocatorSelection::Android` is
the default there.

### Phase 8 — touch ✅

`window/src/os/android/touch.rs` recognises gestures rather than pretending a
finger is a mouse: tap focuses, drag scrolls with fling momentum, long press
starts a selection, pinch changes the font size. Scroll gestures are consumed
by the client so that scrollback works inside `less`; tap and long-press
synthesise press/release/move, so a TUI that requested mouse tracking still
sees them.

### Phase 9 — mux client ✅

When `default_domain` names an ssh, TLS or unix domain, the Android entry point
builds the same `StartCommand` that `wezterm connect <name>` would, including
`attach` so that the client adopts the panes already on the server.
`always_new_process` is unconditional: there is only one process, and its
discovery socket lives where nothing else can reach it.

`CODEC_VERSION` stays in sync by construction, since the app and the mux server
build from the same tree.

## What the device changed

What was verified working: the environment bootstrap, config loading, EGL and
GL rendering, the Android system-font locator, glyph rendering, pty spawn, key
input, IME text, the pty tracking the window (46x52 on this phone), the tab
bar, all twelve extra-row keys with their latches, tap, drag-scroll,
long-press selection, the clipboard round trip, rotation, surface loss and
restore across backgrounding, and the `MuxService` keeping the process and its
shells alive under a foreground notification.

Six things it broke, each now fixed:

- **The cdylib would not load.** Nineteen undefined `egl*` symbols, because
  the generated bindings declare the entry points rather than dlsym-ing them.
  `libEGL` has to be on the link line.
- **Insets are not optional.** targetSdk 35 enforces edge-to-edge, so
  `setDecorFitsSystemWindows(true)` does nothing and the status bar sat on the
  tab bar. The Activity consumes the insets and pads the content view itself.
- **The pty stayed at 24x80** while the window painted at full size. Waiting
  for the surface before building the window fixed the size but silenced the
  announcement of it, since `resized` only speaks on a change; the size is now
  dispatched explicitly.
- **The extra keys row overflowed and was invisible.** `min_width` is a
  minimum, so equal shares did not hold the wider labels, and the keys were
  painted the same grey as the row behind them.
- **The first prompt was eaten on every launch.** Two independent causes: a
  cursor at the start of its own line was pulled onto the line above by
  `rewrap_lines`, so the next write landed on top of it; and mksh erases its
  prompt when the width changes under it, which on Android happens at every
  startup because the pane is spawned at `initial_cols` and immediately
  resized. The first is a terminal bug that any platform can hit and now has a
  regression test; the second is avoided by measuring the surface and
  presizing the first pane.
- **The extra keys row vanished when the soft keyboard came up.** It was only
  rebuilt on config changes, so after a resize the cached row was drawn at the
  old geometry, off the bottom of the shrunken surface.

Two notes for anyone debugging on similar hardware. MIUI drops app log output
almost entirely, so `adb logcat` shows the framework's lines about the process
and none of wezterm's own; the shell itself, writing to a file under the app's
data dir, turned out to be the reliable instrument. And MIUI refuses
`adb install`, so `adb push` plus `pm install -r -t` is the way in.

## What remains

- **Back quits without asking.** The Activity finishes, `android_main`
  returns, and the process exits with every pane in it — the `MuxService`
  does not save it, because the service dies with the process. Backgrounding
  with Home is safe; Back is not. It should either be ignored, sent to the
  pane, or confirmed.
- **Pinch-to-zoom is unverified.** `adb` cannot synthesise a second finger, so
  the pinch path is the one gesture that was never exercised.
- **The row is sparse in landscape.** Key widths follow their labels and the
  remainder becomes margin, which on a 2400px-wide window is a lot of margin.
  Capping the gap would look better.
- **No shell is bundled.** Phase 4b's machinery is in place but
  `android/app/src/main/prefix/` is empty. A static bash and busybox have to be
  built for Android. The system mksh works in the meantime.
- **No way to edit `wezterm.lua` on device** except `adb shell run-as`.
- **Binary size is unmeasured.** The debug APK is 87 MB. Release + LTO +
  stripping has not been measured; vendored fonts and the Lua runtime are not
  small.
- **Battery and thermals.** Continuous GPU compositing of a terminal is not
  something the desktop backends ever had to economise on. The loop blocks on
  the looper when idle and only spins for fling momentum and long-press
  timing, but this has not been measured.
- **Upstream appetite is unknown.** The `egl.rs` surface-swap change, the
  `effective_bottom_padding` helper and the `rewrap_lines` cursor fix touch
  shared code. All are small and only the last changes behaviour off Android —
  where it fixes a bug — which should help, but this is worth raising in a
  GitHub Discussion.

## Risks that did not materialise

- **Phase 3 was expected to be the real risk.** It was not, because the
  EGLContext survives surface loss. The shared-code change is confined to
  `egl.rs` (+140/-21, much of it the surfaceless guards on the glium `Backend`
  impl) and no other platform's behaviour changes.
- **The GUI needing porting.** It did not. `wezterm-gui` compiled unmodified
  apart from the entry point split and the new key row.

## Risks that remain

- **More desktop assumptions are hiding on the startup path.**
  `config/src/lib.rs:69` was the first `expect` that fires, not necessarily the
  last, and each one is only discoverable by running.
- **Soft-keyboard quality is out of our control** and largely determines
  whether the result is pleasant.
