//! The platform-neutral face of native dialogs.
//!
//! Only Android implements one; see `os/android/dialog.rs` for what a dialog is
//! for and why the contract is two calls wide. This exists so that the GUI can
//! be written and compiled once: everywhere else the request fails immediately
//! with a message saying so, rather than the caller having to be written twice.

use promise::Future;

/// How a dialog ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome {
    /// The user submitted it. The string is whatever the platform layer was
    /// asked to hand back; its schema belongs to the caller.
    Submitted(String),
    /// The user dismissed it. Cancelling has no effect beyond failing the
    /// operation in progress.
    Cancelled,
}

/// Ask the platform to present a dialog described by `spec`.
#[cfg(target_os = "android")]
pub fn request_dialog(spec: String) -> Future<DialogOutcome> {
    crate::os::android::dialog::request_dialog(spec)
}

#[cfg(not(target_os = "android"))]
pub fn request_dialog(_spec: String) -> Future<DialogOutcome> {
    Future::err(anyhow::anyhow!(
        "native dialogs are only implemented on Android"
    ))
}

/// True when [`request_dialog`] can do anything.
pub fn dialogs_available() -> bool {
    cfg!(target_os = "android")
}

/// Hand text to the platform's share mechanism.
///
/// Android only: it is how anything leaves an app that can reach no directory the
/// user can see. Elsewhere this reports that there is nothing to share with, and
/// the caller falls back to the clipboard.
#[cfg(target_os = "android")]
pub fn share_text(subject: &str, text: &str) -> anyhow::Result<()> {
    crate::os::android::dialog::share_text(subject, text);
    Ok(())
}

#[cfg(not(target_os = "android"))]
pub fn share_text(_subject: &str, _text: &str) -> anyhow::Result<()> {
    anyhow::bail!("sharing is only implemented on Android")
}
