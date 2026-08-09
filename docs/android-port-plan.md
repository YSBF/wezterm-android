# Android GUI port plan

Status: **toolchain proven (it links), porting not started. The binary has never
been run on a device and is known to abort at startup — see Phase 1.**

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

The probe worktree and its diff live at
`/home/ysbf/Archive/tools/wezterm-android-probe` (`android-probe.patch`).

### What the probe did *not* prove

It proved the binary **links**, not that it **runs**. Every method in the stub
backend is `todo!()`, and the process aborts before reaching any of them:

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
itself. That makes it an entry-point concern, addressed in Phase 1.

Assume there are more assertions like this on the startup path; `HOME_DIR` is
simply the first one. See the risks section for how to flush them out cheaply.

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

```bash
NDK=$HOME/Application/Android_SDK/ndk/28.0.12674087
BIN=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin
API=28

rustup target add aarch64-linux-android

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$BIN/aarch64-linux-android$API-clang
export CC_aarch64_linux_android=$BIN/aarch64-linux-android$API-clang
export CXX_aarch64_linux_android=$BIN/aarch64-linux-android$API-clang++
export AR_aarch64_linux_android=$BIN/llvm-ar
export RANLIB_aarch64_linux_android=$BIN/llvm-ranlib
export CFLAGS_aarch64_linux_android="--target=aarch64-linux-android$API -fPIC"

cargo build --target aarch64-linux-android -p wezterm-gui \
      --no-default-features --features vendored-fonts
```

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

## The four changes needed to build

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

**This is a stopgap, not a solution — see Phase 6.**

## Upstreamable independently of Android

One finding is worth a PR regardless of whether this port ever ships.

`config` depends on `wezterm-ssh` non-optionally, and its entire use is
`config/src/ssh.rs:112` — three calls that parse `~/.ssh/config` to enumerate
host names for auto-generated SSH domains:

```rust
let mut config = wezterm_ssh::Config::new();
config.add_default_config_files();
for host in config.enumerate_hosts() { ... }
```

Because everything depends on `config`, that one text-parsing call site pulls
openssl + libssh + libssh2 into every binary in the tree. Feature-gating it
would cut build times and the dependency surface for all platforms.

## Phased plan

Phase 0 is done. Phases 1–4a are the architectural work — entry point,
surface lifetime, process lifetime. Phases 4b–8 are where a usable terminal
actually lives. Phase 9 is close to free once the rest works.

### Phase 0 — cross-compile and link ✅

Done, with the caveat above: it links, it does not run.

### Phase 1 — APK shell, entry point, and environment bootstrap

Today the probe produces a `[[bin]]` with `fn main()`. An Android app needs a
`cdylib` loaded by an Activity.

- Add `crate-type = ["cdylib"]` for the Android target.
- Adopt `android-activity` with **GameActivity**, not NativeActivity. This is
  not a neutral choice and should be settled here rather than revisited later:
  NativeActivity's soft-keyboard/IME support is poor, while GameActivity ships
  GameTextInput, which maps far more directly onto the commit/composing-text
  handling Phase 5 needs. Switching later means redoing the input layer.
- Minimal gradle project + manifest; `cargo-ndk` to place `.so` per ABI.
- Ship `aarch64` only at first; add `armv7`/`x86_64` later.

**Environment bootstrap — do this before any code touches `config`.** Android
app processes have no `HOME`, no `TMPDIR`, and a `PATH` that is useless for
spawning shells. At minimum, set from the JNI entry point:

| Variable | Value |
|---|---|
| `HOME` | app internal files dir (`Context.getFilesDir()`) |
| `XDG_CONFIG_HOME` | a `config/` subdir of the above |
| `XDG_RUNTIME_DIR` | app cache dir |
| `TMPDIR` | app cache dir |
| `PATH` | the bundled prefix from Phase 4 |

Also decide here how a user edits `wezterm.lua` on device, because it
constrains the path layout: the internal files dir is not reachable by other
apps, so it needs either a scoped-storage/SAF import flow, an in-app editor, or
a documented `adb push` path for development. Termux performs a full
environment bootstrap for exactly these reasons; this is not optional polish.

Exit criterion: APK installs and launches to a blank surface, reaches
`Connection::create_new`, and panics there on `todo!()` — i.e. it got past
config initialisation.

### Phase 2 — EGL surface and first frame

- Implement `Connection::create_new` against `AndroidApp`.
- Implement `Window::window_handle` returning `RawWindowHandle::AndroidNdk`
  wrapping `ANativeWindow`.
- Implement `WindowOps::enable_opengl` via the shared `crate::egl::GlState`
  (`window/src/egl.rs:492`), mirroring `window/src/os/x11/window.rs:165`.
- Wire `invalidate` / `finish_frame` to the frame callback.

Exit criterion: the wezterm tab bar and a cell grid render on screen.

### Phase 3 — lifecycle and surface loss ⚠️ architectural

This is the one change that touches shared code rather than the new backend.

`ConnectionOps::run_message_loop()` assumes the app owns the loop and that the
window outlives the app. Android inverts both: the OS owns the loop, and the
`ANativeWindow` and its EGLSurface are destroyed and recreated on every
background, rotate, or config change.

Nothing in the `window` crate or in `TermWindow` (`gl: Option<Rc<glium::backend::Context>>`,
`wezterm-gui/src/termwindow/mod.rs:466`) models "surface lost, rebuild the GL
context". Glyph cache and atlas textures must be repopulated on recreate.

Plan: drive the loop from `AndroidApp::poll_events`, add explicit
`SurfaceCreated`/`SurfaceDestroyed` transitions, and give `TermWindow` a
context-rebuild path. `WindowOps::notify` (`window/src/lib.rs:255`) maps onto
posting a user event into the `ALooper` queue.

Exit criterion: rotate the device and switch apps briefly — rendering resumes
correctly on the recreated surface.

Note this covers **surface** lifetime only. Surviving a backgrounded app is a
different and larger problem, handled in Phase 4.

### Phase 4 — process lifetime, PTY, and exec

#### 4a. Process lifetime ⚠️ prerequisite

Surface recreation does not save you from process death. Android will kill a
backgrounded app, taking the mux and every child shell with it — so "check your
email, come back, your build is still running" does not work by default, no
matter how well Phase 3 goes.

This requires Java/Kotlin-side work with no Rust analogue in the tree:

- A **foreground Service** hosting the mux, with a persistent notification.
- Probably a partial `WakeLock` so long-running commands are not suspended.
- `POST_NOTIFICATIONS` permission handling (API 33+).
- A decision on what the notification shows — active pane count, running
  command, or a stop action.

Treat this as a prerequisite for "usable terminal", not a later refinement.

#### 4b. PTY and exec

`portable-pty` already cross-compiles unmodified and bionic has
`libc::openpty` (`pty/src/unix.rs:36`), so the PTY itself is expected to work.
The problem is *what gets exec'd*.

`portable-pty` already cross-compiles unmodified and bionic has
`libc::openpty` (`pty/src/unix.rs:36`), so the PTY itself is expected to work.
The problem is *what gets exec'd*.

Since API 29, apps may not exec binaries from writable app data (W^X), and
SELinux constrains the rest.

**Bundle the shell in the APK's native library directory.** That directory is
the one place an app may both read and execute from, so binaries are shipped
named `lib*.so` (`libbash.so`, `libcoreutils.so`) and `extractNativeLibs` is
left enabled. This is the standard Termux-on-modern-Android approach and needs
no root. It carries its own decisions:

- Which shell — `bash` is the expected default; a `mksh`/`busybox ash` fallback
  is much smaller.
- Which coreutils — a single multi-call `busybox`/`toybox` blob keeps size and
  build complexity down versus per-utility binaries.
- APK size budget, and per-ABI splits so an arm64 device does not carry armv7
  copies.
- A first-run step that populates the prefix and rewrites `PATH` (Phase 1).

Root/Magisk remains available to exec from an arbitrary prefix, but it makes
the app root-only; treat it as a fallback, not the design.

**Reusing an installed Termux prefix is not viable — do not spend time on it.**
Termux's `usr` tree lives in its private data directory, which SELinux makes
unreachable from another app irrespective of exec permissions. `sharedUserId`
would require Termux's signing key (and is deprecated), and its `run-command`
intent executes in *Termux's* process, which cannot give us a PTY master fd in
ours.

Exit criterion: an interactive shell in a pane, correct on resize
(`TIOCSWINSZ`), surviving a backgrounded app via 4a.

### Phase 5 — input

Terminals need Ctrl, Alt, Esc, arrows and function keys; Android supplies IME
text commits. `inputmap.rs` (811 lines) and `keyevent.rs` (871 lines) are
modelled on physical keyboards with modifiers.

- Map IME commit / composing text onto the existing dead-key and IME paths.
- Build an extra-keys row (Ctrl/Alt/Esc/Tab/arrows/Fn). `termwindow/box_model.rs`
  is a usable toolkit for this; it does not need to be Android-native UI.
- Support physical keyboards over Bluetooth/USB as a first-class path — it is
  the cheapest way to get a usable terminal early.

### Phase 6 — clipboard

`WindowOps::get_clipboard` / `set_clipboard` (`window/src/lib.rs:317-320`) have
no default body — every other backend implements them, and copy/paste is core
terminal functionality, so this is a phase item rather than a footnote.

The NDK exposes no clipboard API, so this is JNI to `android.content.ClipboardManager`:

- `setPrimaryClip` / `getPrimaryClip` marshalled across JNI, on the main thread.
- Map wezterm's `Clipboard::{Clipboard, PrimarySelection}` onto the single
  Android clipboard; primary-selection has no equivalent and should alias or
  no-op rather than error.
- Android 13+ shows a system copy confirmation and truncates large clips —
  worth checking against terminal-sized selections.
- Paste must sanitise clipboard text the same way the desktop paths do
  (bracketed paste, newline handling).

This interacts directly with Phase 8 (touch): long-press-to-select is only useful once
copy works, so the two are best built and tested together.

### Phase 7 — fonts and CJK ⚠️ blocking for CJK users

`ConfigDirsOnly` sees only vendored fonts and configured `font_dirs`. It will
not see `/system/fonts`, and the bundled JetBrains Mono has no CJK coverage.
Android has no fontconfig, so `font_config.rs` cannot be reused.

Write `wezterm-font/src/locator/android.rs`, roughly the size of `gdi.rs`:

- Enumerate `/system/fonts`, `/system/font`, `/product/fonts`.
- Parse `/system/etc/fonts.xml` for family and fallback ordering.
- Feed `ParsedFont` entries into the existing fallback machinery.

The whole cell-grid alignment problem that `wezterm-font`'s 10,266 lines solve
— CJK wide characters, Nerd Font fallback, grid alignment — does not go away on
Android; it just needs a different font source.

### Phase 8 — touch

`mouseevent.rs` (1,053 lines) is a mouse state machine: hover, drag-select,
click-to-focus-pane. None of it has a touch analogue in the existing backends.

Design a gesture layer before writing code: tap to focus, long-press to start
selection with a magnifier, drag to scroll with momentum, pinch to change font
size, edge-swipe for tab switching. Decide explicitly what maps to terminal
mouse reporting versus what the client consumes.

### Phase 9 — mux client

Largely free once the above works: `wezterm connect` uses `wezterm-client` in
process, so `renderable.rs`'s `lines: LruCache` (line 68) and
`predict_from_key_event` (line 212) provide local scrollback and local echo on
the device — the original motivation for choosing wezterm over a plain SSH
client.

Note `CODEC_VERSION` (currently 45, `codec/src/lib.rs:444`) is checked for
strict equality at `wezterm-client/src/client.rs:1160`. Because the app and the
mux server build from the same source tree, versions stay in sync by
construction — which is exactly why this beats reimplementing the protocol.

Also note `wezterm/src/main.rs:752` routes `connect`/`ssh` through
`delegate_to_gui`; on Android there is no separate GUI process to delegate to,
so the entry point needs its own path.

## Risks and open questions

- **Phase 3 is the real risk.** Surface-loss handling touches shared code and
  is the most likely source of upstream friction.
- **More desktop assumptions are hiding on the startup path.**
  `config/src/lib.rs:69` is the first `expect` that fires, not necessarily the
  last, and each one is only discoverable by running. Cheap mitigation, worth
  doing before Phase 1 design work: push the existing probe binary to a device
  and run it under `adb shell run-as` (or in a Termux shell with `HOME` set),
  and read `logcat`. It will abort immediately, but each iteration surfaces the
  next assumption in minutes. This turns "toolchain proven" into "binary runs",
  which is a materially stronger starting point.
- **Soft-keyboard quality is out of our control** and largely determines
  whether the result is pleasant. Validate early with a physical keyboard so
  this does not block Phases 2–4.
- **Binary size**: the debug build is 587 MB. Release + LTO + stripping needs
  measuring; vendored fonts and the Lua runtime are not small.
- **Battery and thermals**: continuous GPU compositing of a terminal is not
  something the desktop backends ever had to economise on. Damage-driven
  repaint may need revisiting.
- **Upstream appetite is unknown.** Phases 1–3 imply changes to shared
  abstractions. Worth raising in a GitHub Discussion before writing Phase 3.

## Suggested first milestone

Phase 0.5 first, because it is nearly free: push the existing probe binary to a
device and iterate on the startup-path assumptions until it reaches
`Connection::create_new` and panics on `todo!()`. No APK, no gradle, no
Kotlin — just `adb`, `logcat`, and environment variables. This converts the
biggest unknown ("what else assumes a desktop?") into a finite list before any
architectural work starts.

Then Phases 1–2: an APK that launches and renders the tab bar over EGL, with
input and PTY stubbed. That is the smallest artifact that proves the renderer
works on-device, and it is the point at which the remaining unknowns become
measurable rather than estimated.
