# wezterm for Android

The Android app is a thin Kotlin shell around the same `wezterm-gui` code the
desktop builds use. The terminal, renderer, mux and config are all Rust; the
Kotlin here exists only to do the two things Rust cannot: subclass an Activity,
and ask the system to keep the process alive.

```
android/app/src/main/
  AndroidManifest.xml                  permissions, the Activity, the Service
  java/.../WezTermActivity.kt          GameActivity subclass; loads the cdylib
  java/.../MuxService.kt               foreground service + wake lock
```

The native side lives in `wezterm-android` (the cdylib), `wezterm-gui/src/android.rs`
(the entry point and environment bootstrap) and `window/src/os/android/` (the
windowing backend).

## Building

You need a Rust toolchain with the Android targets installed, an NDK, and the
Android SDK. `ci/android-env.sh` at the repository root is the single
definition of the cross-compile environment, and is used both by hand and by
Gradle.

```bash
rustup target add aarch64-linux-android
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.0.12674087   # or wherever

cd android
./gradlew installDebug
```

Gradle invokes `cargo build -p wezterm-android` for each configured ABI and
stages the result into `jniLibs`. To build just the native library without
Gradle:

```bash
. ci/android-env.sh
cargo build --target aarch64-linux-android -p wezterm-android \
      --no-default-features --features vendored-fonts
```

`--no-default-features` matters: the default feature set enables `wayland`,
which turns on `window/wayland`.

### Which ABIs

`wezterm.abis` in `gradle.properties` controls this; it defaults to
`arm64-v8a` alone. The native library is large, so building several ABIs into
one APK is wasteful — the Gradle config turns on ABI splits automatically as
soon as more than one is listed.

### Debug builds are enormous

An unoptimised build of the whole GUI is several hundred MB per ABI. That is
fine for `adb install` during development, but `assembleRelease` is what
produces something shippable.

## Diagnosing startup

Everything before the config is loaded logs to logcat:

```bash
adb logcat -s wezterm_gui:V wezterm:V window:V config:V
```

The environment bootstrap logs the resolved paths at info level on the way up,
which is the fastest way to tell whether `HOME` and `PATH` came out right.

## Bundling a shell

The APK ships no shell of its own. `PATH` includes `/system/bin`, so whatever
toybox provides is available and the terminal works, but `bash` and a fuller
set of utilities have to be bundled.

Since API 29 an app may not execute a binary out of its writable data
directory, and SELinux constrains most of what is left. The one directory an
app may both read *and* execute from is the APK's native library directory. The
installer only extracts and marks executable those files matching `lib*.so`, so
binaries are shipped under that name:

```
android/app/src/main/prefix/
  arm64-v8a/
    libbash.so         -> becomes `bash`
    libbusybox.so      -> becomes `busybox`, plus one link per applet
```

At startup wezterm creates `$HOME/.local/bin/<name>` as a symlink to the
matching `lib<name>.so` and puts that directory first on `PATH`. `execve`
resolves the symlink and applies the W^X and SELinux checks to the target,
which lives in the executable directory, so this needs no root. Multi-call
binaries are asked for their applet list (`busybox --list`, `toybox --long`)
and get one link each; a dedicated binary always wins over an applet of the
same name.

`SHELL` is set to the best shell found, preferring `bash`, and falls back to
`/system/bin/sh`. Note that `/bin/sh`, which `portable-pty` would otherwise
reach for, does not exist on Android.

Building those binaries for Android is a separate exercise; the NDK toolchain
in `ci/android-env.sh` is the right starting point. Watch the APK size budget:
a static bash plus busybox is a few MB per ABI, which is why per-ABI splits are
on.

### Why not reuse an installed Termux prefix

Termux's `usr` tree lives in its private data directory, which SELinux makes
unreachable from another app irrespective of execute permissions.
`sharedUserId` would need Termux's signing key and is deprecated, and its
`run-command` intent executes in *Termux's* process, which cannot hand a pty
master fd back to ours. Root/Magisk would allow executing from an arbitrary
prefix, but that makes the app root-only; it is a fallback, not the design.
