// Don't create a new standard console window when launched from the windows GUI.
#![cfg_attr(not(test), windows_subsystem = "windows")]

//! The desktop entry point. Everything lives in the library so that the
//! Android cdylib, which is loaded by an Activity rather than executed, can
//! share it.

fn main() {
    wezterm_gui::wezterm_gui_main()
}
