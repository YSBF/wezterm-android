//! The Android shared library entry point.
//!
//! `android-activity` spawns a dedicated native thread and calls the exported,
//! unmangled `android_main` symbol on it. Everything past that lives in
//! `wezterm-gui`, which is a library precisely so that this can be a five-line
//! shim rather than a fork of the desktop entry point.
//!
//! Two rules the Activity lifecycle imposes on that thread, both honoured
//! inside `wezterm_gui::android`:
//!
//! * `AndroidApp::poll_events` must be called regularly or Android shows an
//!   ANR dialog, because some Activity callbacks on the Java main thread block
//!   until it is.
//! * `std::process::exit` must never be called, because the process may host
//!   other Android components -- notably the foreground Service that keeps the
//!   mux alive while the app is backgrounded.

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    wezterm_gui::android::android_main(app);
}
