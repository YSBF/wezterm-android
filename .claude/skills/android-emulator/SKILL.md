---
name: android-emulator
description: Launch the Android emulator and install/run the wezterm android-port app on it. Use when asked to start the emulator, test the Android build, install the APK, or debug why the app crashes on Android.
---

# Android emulator for the wezterm port

## Launching

```bash
.claude/skills/android-emulator/launch-emulator.sh
```

It returns once `sys.boot_completed` is `1`, and prints the serial and ABI list.
Run it in the background — it stays in the foreground while the emulator lives.

Two things it handles that a bare `emulator -avd ...` does not:

**Display.** Claude Code and most ssh/tmux shells start with `XDG_SESSION_TYPE=tty`
and no `DISPLAY`. The emulator's bundled Qt ships no wayland plugin — only
`vnc, offscreen, xcb, minimal, linuxfb` — so on a GNOME/Wayland session it has
to go through Xwayland, and mutter's Xwayland only accepts a private cookie at
`/run/user/1000/.mutter-Xwaylandauth.XXXXXX`. That filename is regenerated on
every login, so it must be scraped from the running `Xwayland` process's `-auth`
argument rather than hardcoded. Without this the emulator aborts with:

```
Fatal: This application failed to start because no Qt platform plugin could be initialized.
```

**`-gpu host`.** Left on `auto` the emulator logs `Your GPU drivers may have a
bug. Switching to software rendering` and selects swANGLE, which exposes only
GLSL ES 1.00. wezterm's lowest shader version is `300 es`, so it panics with
`No OpenGL` at `wezterm-gui/src/termwindow/mod.rs` before drawing anything.
With `-gpu host` the emulator picks the discrete GPU and gets real GLES.

## The AVD

`reverse_eng` — x86_64, API 36, google_apis, rooted. Serial `emulator-5554`.
SDK at `~/Application/Android_SDK`.

It reports `ro.product.cpu.abilist = x86_64,arm64-v8a` and carries
`libndk_translation.so`, so an `arm64-v8a` APK **will** install and run, just
translated. That is slow and muddies native crash traces — build `x86_64`
instead.

## Building for the emulator

`wezterm.abis` defaults to `arm64-v8a` in `android/gradle.properties`, which is
right for shipping. Override it on the command line rather than editing the
file:

```bash
cd android && ./gradlew assembleDebug -Pwezterm.abis=x86_64
```

Nothing else needs changing: `rustTargetFor` in `app/build.gradle.kts` already
maps `x86_64` to `x86_64-linux-android`, and `ci/android-env.sh` already exports
that target's linker, `CC`, `AR` and `CFLAGS`. The one prerequisite is the Rust
target itself:

```bash
rustup target add x86_64-linux-android
```

`abiFilters` is driven by the same property, so a stale `arm64-v8a` directory
left in `app/build/rustJniLibs/debug/` from an earlier build is excluded from
the APK rather than doubling its size.

The debug cdylib is ~710MB per ABI and the shared cargo target directory
reaches several GB. Do not build under a tmpfs `/tmp`.

## Install and run

Switching ABI needs a real uninstall first; `install -r` will not replace a
package whose native library moved to a different ABI directory.

```bash
adb -s emulator-5554 uninstall org.wezfurlong.wezterm
adb -s emulator-5554 install android/app/build/outputs/apk/debug/app-debug.apk
adb -s emulator-5554 logcat -c
adb -s emulator-5554 shell am start -n org.wezfurlong.wezterm/.WezTermActivity
```

Confirm which ABI actually loaded:

```bash
adb -s emulator-5554 logcat -d | grep 'nativeloader.*libwezterm'
```

A path under `lib/x86_64/` is native; `lib/arm64/` means it is being translated.

Package is `org.wezfurlong.wezterm`. Check it survived with
`adb -s emulator-5554 shell pidof org.wezfurlong.wezterm` — an empty result
means it aborted.

## Reading the logs

```bash
adb -s emulator-5554 logcat -d -s wezterm_gui:V wezterm:V window:V config:V
```

The startup path logs at info: `wezterm_gui::android` prints the resolved
`AndroidDirs` (home, config, cache, native_lib) on the way up, which is the
fastest way to tell whether the environment bootstrap came out right.

Rust panics land in three places, and only one of them is useful:

- `wezterm_gui::termwindow` at error level — **the actual cause**
- `env_bootstrap: panic at ... - <message>` followed by 50 lines of `<unknown>`
  frames, since the stripped cdylib has no symbols
- `cannot access a Thread Local Storage value during or after destruction` then
  `fatal runtime error: thread local panicked on drop, aborting` — this is
  always secondary fallout from the first panic unwinding. Chasing it is a dead
  end; find the earlier panic instead.

To skip straight past the noise:

```bash
adb -s emulator-5554 logcat -d | grep -E 'wezterm_gui|RustPanic' | grep -v '<unknown>'
```

## GLES differences the port has to absorb

The renderer was written against desktop GL and three things had to give before
it would draw on GLES. All three are fixed; they are recorded here because the
symptoms are misleading and each will recur on real devices.

**Shader version.** `renderstate.rs` tries `330 core`, `330`, `320 es`,
`300 es` in order and panics `No OpenGL` when all four fail. `330*` are desktop
GLSL and are always rejected on GLES; that is expected and not the problem.

**Context version.** `window/src/egl.rs` used to request only
`CONTEXT_MAJOR_VERSION`, which yields ES 3.0, and a 3.0 context rejects
`#version 320 es` as `'320' : client/version number not supported` before it
even reads the shader. It now walks 3.2 → 3.1 → 3.0 and keeps the first
granted. Desktop GL still gets the bare major-version request, because asking
for 3.2 there would cap it below the 330 shaders.

**Dual-source blending.** The glyph fragment shader used
`GL_EXT_blend_func_extended` unconditionally, for the second colour output that
drives subpixel antialiasing. This emulator does not have that extension, and
plenty of real GLES drivers do not either. The shader now compiles either way
behind `#ifdef DUAL_SOURCE_BLENDING`; `compile_prog` tries the dual-source
variant first at each version, returns which one took, and `draw.rs` refuses to
select the subpixel path when it is unavailable. The cost is losing subpixel AA,
which only applies when `freetype_render_target` is an LCD target anyway.

**Buffer mapping.** `glMapBufferRange` returned null and glium dereferenced it,
which surfaces as a *non-unwinding* panic — `null pointer dereference occurred`
at `glium/src/buffer/mod.rs`, with no wezterm frame in sight. The cause was in
logcat under a different tag:

```
E emuglGLESv2_enc: ... s_glMapBufferRange:3424 GL error 0x502
```

0x502 is `GL_INVALID_OPERATION`. glium always sets `GL_MAP_FLUSH_EXPLICIT_BIT`
alongside the write bit, and GLES forbids that combination when the read bit is
also set; desktop GL allows it. Since glium's write-only mapping exposes no
slice, `TripleVertexBuffer::map` now takes a `GliumStagedVertexBuffer` path on
GLES: write into a `Vec`, then `glBufferSubData` the touched range back on drop.

The staging `Vec` belongs to the `TripleVertexBuffer`, not to the mapping, and
that is load-bearing. `render_element` maps a layer once per *element*, not once
per frame, and each of those mappings writes only its own quads while
`next_quad` keeps climbing. A real mapping leaves the rest of the buffer alone;
staging that started empty each time uploaded zeroes over everything the earlier
elements had drawn. The symptom was a frame holding only the last element
rendered into each layer — the tab bar reduced to its close button, the key row
to a single key.

When something dies in the renderer, check the `emuglGLESv2_enc` tag before
believing the Rust backtrace — a GL error there explains a null-pointer panic
that otherwise looks like a glium bug.

## Rounded corners need a border colour

Not a GLES issue, but it has bitten twice — once in the key row, once in the
sidebar. `Element::border_corners` rounds a box by painting a `PolyStyle::Fill`
oval into each corner square that the background fill deliberately *skips*. The
poly takes its colour from `ElementColors::border`, so an element that rounds
its corners while leaving `border: Default::default()` gets four transparent
holes instead of four curves: a key renders as a cross, a sidebar button as a
notched slab.

Any element that calls `.border_corners(..)` must also set
`border: BorderColor::new(bg)` on both `colors` and `hover_colors`, matching
whatever background that state uses. `fancy_tab_bar.rs` has always done this;
`keyrow.rs` and `sidebar.rs` did not.

## Current state

Runs, draws, and takes input. Verified with `adb shell input text` driving a
shell in the pane and reading the output back off a screencap.
