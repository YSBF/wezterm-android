//! Ownership of the `AndroidApp` handed to `android_main`.
//!
//! Every other platform can ask the windowing system for a connection at any
//! time. Android inverts that: the OS constructs the activity and hands the
//! app object to a single entry point, so the entry point has to stash it
//! somewhere the rest of the crate can reach before `Connection::create_new`
//! runs.
//!
//! It is deliberately not write-once. `android_main` runs per Activity, not
//! per process, and the process outlives an Activity that is destroyed and
//! recreated -- a change to the system font scale is enough, since that is not
//! one of the configuration changes the manifest handles in place. The second
//! `android_main` is handed a *different* `AndroidApp`, and the one from the
//! first is attached to an Activity that no longer exists: events, clipboard,
//! IME and surfaces would all be aimed at the dead one.
//!
//! Each registration therefore replaces the last and is tagged with a
//! generation, so that a departing `android_main` clears the app only if a
//! successor has not already claimed the slot. Without the tag the teardown of
//! the outgoing Activity would race the startup of the incoming one and
//! sometimes leave the process with no app at all.

use android_activity::AndroidApp;
use std::sync::Mutex;

static ANDROID_APP: Mutex<Option<(u64, AndroidApp)>> = Mutex::new(None);

/// Identifies one `android_main` invocation's claim on the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppGeneration(u64);

/// Record the `AndroidApp` for this Activity, replacing any predecessor. Must
/// be called from `android_main` before anything constructs a `Connection`.
///
/// The returned generation is the handle for [`clear_android_app`].
pub fn set_android_app(app: AndroidApp) -> AppGeneration {
    let mut slot = ANDROID_APP.lock().unwrap();
    let generation = slot.as_ref().map_or(0, |(g, _)| g + 1);
    if generation > 0 {
        log::info!("android_main re-entered; replacing the AndroidApp (generation {generation})");
    }
    *slot = Some((generation, app));
    AppGeneration(generation)
}

/// Drop the recorded `AndroidApp`, but only if it is still the one registered
/// by the matching [`set_android_app`]. Called as `android_main` returns.
pub fn clear_android_app(generation: AppGeneration) {
    let mut slot = ANDROID_APP.lock().unwrap();
    if slot.as_ref().map(|(g, _)| *g) == Some(generation.0) {
        *slot = None;
    }
}

/// The currently registered `AndroidApp`, if any.
///
/// This hands back a clone rather than a reference: the slot can be replaced,
/// so nothing here can be `'static`. `AndroidApp` is a handle, so cloning it
/// is cheap and the clone refers to the same Activity.
pub fn try_android_app() -> Option<AndroidApp> {
    ANDROID_APP
        .lock()
        .unwrap()
        .as_ref()
        .map(|(_, app)| app.clone())
}

/// The currently registered `AndroidApp`.
pub fn android_app() -> anyhow::Result<AndroidApp> {
    try_android_app().ok_or_else(|| {
        anyhow::anyhow!(
            "no AndroidApp has been registered; \
             window::os::android::set_android_app must be called from android_main"
        )
    })
}
