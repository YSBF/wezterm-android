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

## What is not here yet

The APK contains no shell. `PATH` includes `/system/bin`, so whatever toybox
provides is available, but a real prefix — bash, coreutils — has to be bundled
into the native library directory as `lib*.so` files, which is the one place an
app may both read and execute from since API 29. See `docs/android-port-plan.md`.
