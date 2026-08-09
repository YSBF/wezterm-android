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
//! * `touch` recognises gestures, because a finger is not a mouse.
//! * `clipboard` is the one part that has to go through JNI.

pub mod app;
mod clipboard;
mod connection;
mod keyboard;
mod touch;
mod window;

pub use app::{android_app, set_android_app, try_android_app};
pub use connection::Connection;
pub use window::Window;
