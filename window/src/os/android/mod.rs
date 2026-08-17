//! The Android backend.
//!
//! A fourth sibling to `x11`, `wayland`, `macos` and `windows`. The renderer,
//! tab bar, overlays and glyph cache above it are unchanged; what differs is
//! that Android owns the process and the surface, and can take either away.
//!
//! * `app` holds the `AndroidApp` handed to `android_main`.
//! * `connection` runs the event loop and translates lifecycle transitions.
//! * `window` is the single activity-filling window and its EGL state.
//! * `keyboard` maps Android keycodes onto wezterm key events.
//! * `ime` tracks whether the soft keyboard is up.
//! * `clipboard` and `dialog` are the parts that have to go through JNI.
//!
//! Gesture recognition lives in `crate::touch` rather than here, and the region
//! registry it routes on in `crate::gesture`. Neither touches an Android API, so
//! keeping them in the crate root makes them testable on a desktop host, which
//! is where their regression tests run.

pub mod app;
mod clipboard;
mod connection;
pub(crate) mod dialog;
mod ime;
mod keyboard;
mod window;

pub use app::{android_app, clear_android_app, set_android_app, try_android_app, AppGeneration};
pub use connection::Connection;
pub use ime::soft_keyboard_visible;
pub use window::Window;
