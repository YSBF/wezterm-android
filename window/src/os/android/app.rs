//! Ownership of the `AndroidApp` handed to us by `android_main`.
//!
//! Every other platform can ask the windowing system for a connection at any
//! time. Android inverts that: the OS constructs the activity and hands the
//! app object to a single entry point, so the entry point has to stash it
//! somewhere the rest of the crate can reach before `Connection::create_new`
//! runs.

use android_activity::AndroidApp;
use std::sync::OnceLock;

static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

/// Record the `AndroidApp` for this process. Must be called from
/// `android_main` before anything constructs a `Connection`.
pub fn set_android_app(app: AndroidApp) {
    if ANDROID_APP.set(app).is_err() {
        log::warn!("set_android_app called more than once; ignoring");
    }
}

/// The `AndroidApp` recorded by `set_android_app`, if any.
pub fn try_android_app() -> Option<&'static AndroidApp> {
    ANDROID_APP.get()
}

/// The `AndroidApp` recorded by `set_android_app`.
pub fn android_app() -> anyhow::Result<&'static AndroidApp> {
    ANDROID_APP.get().ok_or_else(|| {
        anyhow::anyhow!(
            "no AndroidApp has been registered; \
             window::os::android::set_android_app must be called from android_main"
        )
    })
}
