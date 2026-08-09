#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use self::windows::*;

// Android is target_family = "unix", but shares none of the X11/Wayland stack,
// so every gate below has to exclude it explicitly.
#[cfg(all(feature = "wayland", not(target_os = "android")))]
pub mod wayland;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub mod x11;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub mod x_and_wayland;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub mod xdg_desktop_portal;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub mod xkeysyms;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
pub use x_and_wayland::*;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use self::android::*;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

pub mod parameters;
