# Bundled executables

Drop prebuilt binaries here, one directory per ABI, named `lib<command>.so`:

    arm64-v8a/libbash.so       -> bash
    arm64-v8a/libbusybox.so    -> busybox and its applets

The `lib*.so` naming is not decoration. Since API 29 an app may only execute
binaries out of its native library directory, and the installer only extracts
and marks executable the files matching that pattern. wezterm links them to
their real names at startup; see `wezterm-gui/src/android/prefix.rs`.

Nothing here is required — with the directory empty, `PATH` falls through to
`/system/bin` and the terminal runs whatever toybox provides.
