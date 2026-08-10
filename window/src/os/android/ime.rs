//! Whether the soft keyboard is on screen.
//!
//! GameTextInput can raise and dismiss the IME but cannot be asked whether it
//! is currently up, and the answer changes without us asking: the back gesture
//! dismisses it, losing focus dismisses it, and the system may raise it on its
//! own. A toggle button that tracks only its own presses therefore drifts out
//! of step with reality and then appears to do nothing for one tap.
//!
//! The Activity does know, because it already receives the IME inset in order
//! to keep the terminal above the keyboard, so it reports each change here.

use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

static VISIBLE: AtomicBool = AtomicBool::new(false);

/// True while the soft keyboard is on screen, as last reported by the Activity.
pub fn soft_keyboard_visible() -> bool {
    VISIBLE.load(Ordering::Relaxed)
}

/// Called from `WezTermActivity`'s window insets listener, on the Java main
/// thread, every time the IME inset changes.
#[no_mangle]
pub extern "system" fn Java_org_wezfurlong_wezterm_WezTermActivity_nativeSoftKeyboardVisible(
    _env: *mut c_void,
    _this: *mut c_void,
    visible: u8,
) {
    VISIBLE.store(visible != 0, Ordering::Relaxed);
}
