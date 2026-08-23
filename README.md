# wezterm-android

An Android port of [WezTerm](https://github.com/wezterm/wezterm).

This is a fork of [wezterm/wezterm](https://github.com/wezterm/wezterm), written
by [@wez](https://github.com/wez) and licensed MIT. Upstream's copyright and the
MIT licence in [`LICENSE.md`](LICENSE.md) apply to this repository unchanged, and
upstream's own README is preserved below the rule at the bottom of this file.

This fork is not affiliated with or endorsed by the WezTerm project. Please do
not take Android problems to upstream's issue tracker.

## What this adds

The app is a thin Kotlin `GameActivity` shell around the same `wezterm-gui` Rust
code the desktop builds use — terminal, renderer, mux and config are all shared.
On top of that:

- **An OpenGL ES render path.** The renderer was written against desktop GL.
  Getting it to draw on GLES meant asking EGL for an ES 3.2 context rather than
  settling for 3.0, making the dual-source blending that drives subpixel
  antialiasing optional at shader-compile time, and routing vertex writes
  through a CPU staging buffer, because GLES rejects the read+write mapping
  glium asks for.
- **A touch UI.** Gestures go through a region registry; scrolling moves by
  cells rather than pixels.
- **A row of extra keys.** Ctrl, Esc, Alt, Shift and the arrows, with modifiers
  that arm and lock. The arrows stay pinned while the rest of the row pans.
  Configured by `android_extra_keys_row`.
- **An SSH host sidebar.** Stored host profiles with a keychain, entered through
  native Android dialogs, so a session survives a failed connection.
- **Android platform plumbing.** Soft-keyboard and IME preedit handling, system
  bar insets, one GUI window per process, and surviving Activity recreation.

## State

Work in progress, not a shipping app. It builds, runs, renders and takes input,
verified on an x86_64 emulator (API 36) and on an arm64 device.

The APK bundles no shell. `PATH` includes `/system/bin`, so whatever toybox
provides is there and the terminal works, but `bash` and a fuller set of
utilities still have to be built and bundled — `android/README.md` explains the
W^X and SELinux constraints that shape how.

## Building

See [`android/README.md`](android/README.md) for the toolchain, the ABI
settings, and how to diagnose startup. The short version:

```bash
rustup target add aarch64-linux-android
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.0.12674087   # or wherever

cd android
./gradlew installDebug
```

Design notes and the running plan are in [`docs/android-port-plan.md`](docs/android-port-plan.md)
and [`docs/android-sidebar-and-key-row-plan.md`](docs/android-sidebar-and-key-row-plan.md).

---

*Everything below is upstream's README, unmodified.*

# Wez's Terminal

<img height="128" alt="WezTerm Icon" src="https://raw.githubusercontent.com/wezterm/wezterm/main/assets/icon/wezterm-icon.svg" align="left"> *A GPU-accelerated cross-platform terminal emulator and multiplexer written by <a href="https://github.com/wez">@wez</a> and implemented in <a href="https://www.rust-lang.org/">Rust</a>*

User facing docs and guide at: https://wezterm.org/

![Screenshot](docs/screenshots/two.png)

*Screenshot of wezterm on macOS, running vim*

## Installation

https://wezterm.org/installation

## Getting help

This is a spare time project, so please bear with me.  There are a couple of channels for support:

* You can use the [GitHub issue tracker](https://github.com/wezterm/wezterm/issues) to see if someone else has a similar issue, or to file a new one.
* Start or join a thread in our [GitHub Discussions](https://github.com/wezterm/wezterm/discussions); if you have general
  questions or want to chat with other wezterm users, you're welcome here!
* There is a [Matrix room via Element.io](https://matrix.to/#/#wezterm:matrix.org)
  for (potentially!) real time discussions.

The GitHub Discussions and Element/Gitter rooms are better suited for questions
than bug reports, but don't be afraid to use whichever you are most comfortable
using and we'll work it out.

## Supporting the Project

If you use and like WezTerm, please consider sponsoring it: your support helps
to cover the fees required to maintain the project and to validate the time
spent working on it!

[Read more about sponsoring](https://wezterm.org/sponsor.html).

* [![Sponsor WezTerm](https://img.shields.io/github/sponsors/wez?label=Sponsor%20WezTerm&logo=github&style=for-the-badge)](https://github.com/sponsors/wez)
* [Patreon](https://patreon.com/WezFurlong)
* [Ko-Fi](https://ko-fi.com/wezfurlong)
* [Liberapay](https://liberapay.com/wez)
