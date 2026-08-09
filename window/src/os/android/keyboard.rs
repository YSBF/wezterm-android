//! Translation from Android key events into wezterm key events.
//!
//! Android delivers two largely independent streams:
//!
//! * `KeyEvent`s, which carry a keycode plus a meta-state bitmask. These come
//!   from physical keyboards and from the handful of soft-keyboard keys that
//!   are modelled as keys (enter, backspace, the dpad). This is the stream a
//!   terminal actually cares about, because it is the only one that can carry
//!   Ctrl and Alt.
//! * `TextEvent`s, which carry the whole IME buffer including a composing
//!   region. This is how ordinary soft-keyboard typing arrives, and it has no
//!   notion of modifiers at all.
//!
//! The keycode stream is mapped here. The text stream is handled in
//! `window.rs`, where it is diffed against the previous state to recover the
//! committed text.

use android_activity::input::{KeyAction, KeyCharacterMap, KeyMapChar, Keycode, MetaState};
use wezterm_input_types::{KeyCode, Modifiers, PhysKeyCode};

pub fn modifiers_from_meta_state(meta: MetaState) -> Modifiers {
    let mut mods = Modifiers::NONE;
    if meta.shift_on() {
        mods |= Modifiers::SHIFT;
    }
    if meta.ctrl_on() {
        mods |= Modifiers::CTRL;
    }
    if meta.alt_on() {
        mods |= Modifiers::ALT;
    }
    if meta.meta_on() {
        mods |= Modifiers::SUPER;
    }
    if meta.shift_left_on() {
        mods |= Modifiers::LEFT_SHIFT;
    }
    if meta.shift_right_on() {
        mods |= Modifiers::RIGHT_SHIFT;
    }
    if meta.ctrl_left_on() {
        mods |= Modifiers::LEFT_CTRL;
    }
    if meta.ctrl_right_on() {
        mods |= Modifiers::RIGHT_CTRL;
    }
    if meta.alt_left_on() {
        mods |= Modifiers::LEFT_ALT;
    }
    if meta.alt_right_on() {
        mods |= Modifiers::RIGHT_ALT;
    }
    mods
}

/// The position of the key on a notional US layout. Android keycodes are
/// already layout-independent in the same way, so this is a direct table.
pub fn phys_key_code(code: Keycode) -> Option<PhysKeyCode> {
    use Keycode as K;
    Some(match code {
        K::A => PhysKeyCode::A,
        K::B => PhysKeyCode::B,
        K::C => PhysKeyCode::C,
        K::D => PhysKeyCode::D,
        K::E => PhysKeyCode::E,
        K::F => PhysKeyCode::F,
        K::G => PhysKeyCode::G,
        K::H => PhysKeyCode::H,
        K::I => PhysKeyCode::I,
        K::J => PhysKeyCode::J,
        K::K => PhysKeyCode::K,
        K::L => PhysKeyCode::L,
        K::M => PhysKeyCode::M,
        K::N => PhysKeyCode::N,
        K::O => PhysKeyCode::O,
        K::P => PhysKeyCode::P,
        K::Q => PhysKeyCode::Q,
        K::R => PhysKeyCode::R,
        K::S => PhysKeyCode::S,
        K::T => PhysKeyCode::T,
        K::U => PhysKeyCode::U,
        K::V => PhysKeyCode::V,
        K::W => PhysKeyCode::W,
        K::X => PhysKeyCode::X,
        K::Y => PhysKeyCode::Y,
        K::Z => PhysKeyCode::Z,

        K::Keycode0 => PhysKeyCode::K0,
        K::Keycode1 => PhysKeyCode::K1,
        K::Keycode2 => PhysKeyCode::K2,
        K::Keycode3 => PhysKeyCode::K3,
        K::Keycode4 => PhysKeyCode::K4,
        K::Keycode5 => PhysKeyCode::K5,
        K::Keycode6 => PhysKeyCode::K6,
        K::Keycode7 => PhysKeyCode::K7,
        K::Keycode8 => PhysKeyCode::K8,
        K::Keycode9 => PhysKeyCode::K9,

        K::Numpad0 => PhysKeyCode::Keypad0,
        K::Numpad1 => PhysKeyCode::Keypad1,
        K::Numpad2 => PhysKeyCode::Keypad2,
        K::Numpad3 => PhysKeyCode::Keypad3,
        K::Numpad4 => PhysKeyCode::Keypad4,
        K::Numpad5 => PhysKeyCode::Keypad5,
        K::Numpad6 => PhysKeyCode::Keypad6,
        K::Numpad7 => PhysKeyCode::Keypad7,
        K::Numpad8 => PhysKeyCode::Keypad8,
        K::Numpad9 => PhysKeyCode::Keypad9,
        K::NumpadAdd => PhysKeyCode::KeypadAdd,
        K::NumpadSubtract => PhysKeyCode::KeypadSubtract,
        K::NumpadMultiply => PhysKeyCode::KeypadMultiply,
        K::NumpadDivide => PhysKeyCode::KeypadDivide,
        K::NumpadDot => PhysKeyCode::KeypadDecimal,
        K::NumpadEnter => PhysKeyCode::KeypadEnter,
        K::NumpadEquals => PhysKeyCode::KeypadEquals,
        K::NumLock => PhysKeyCode::NumLock,

        K::F1 => PhysKeyCode::F1,
        K::F2 => PhysKeyCode::F2,
        K::F3 => PhysKeyCode::F3,
        K::F4 => PhysKeyCode::F4,
        K::F5 => PhysKeyCode::F5,
        K::F6 => PhysKeyCode::F6,
        K::F7 => PhysKeyCode::F7,
        K::F8 => PhysKeyCode::F8,
        K::F9 => PhysKeyCode::F9,
        K::F10 => PhysKeyCode::F10,
        K::F11 => PhysKeyCode::F11,
        K::F12 => PhysKeyCode::F12,

        K::Comma => PhysKeyCode::Comma,
        K::Period => PhysKeyCode::Period,
        K::Minus => PhysKeyCode::Minus,
        K::Equals => PhysKeyCode::Equal,
        K::LeftBracket => PhysKeyCode::LeftBracket,
        K::RightBracket => PhysKeyCode::RightBracket,
        K::Backslash => PhysKeyCode::Backslash,
        K::Semicolon => PhysKeyCode::Semicolon,
        K::Apostrophe => PhysKeyCode::Quote,
        K::Slash => PhysKeyCode::Slash,
        K::Grave => PhysKeyCode::Grave,
        K::Space => PhysKeyCode::Space,
        K::Tab => PhysKeyCode::Tab,
        K::Enter => PhysKeyCode::Return,
        K::Del => PhysKeyCode::Backspace,
        K::ForwardDel => PhysKeyCode::Delete,
        K::Escape => PhysKeyCode::Escape,
        K::Insert => PhysKeyCode::Insert,
        K::MoveHome => PhysKeyCode::Home,
        K::MoveEnd => PhysKeyCode::End,
        K::PageUp => PhysKeyCode::PageUp,
        K::PageDown => PhysKeyCode::PageDown,
        K::CapsLock => PhysKeyCode::CapsLock,

        K::DpadUp => PhysKeyCode::UpArrow,
        K::DpadDown => PhysKeyCode::DownArrow,
        K::DpadLeft => PhysKeyCode::LeftArrow,
        K::DpadRight => PhysKeyCode::RightArrow,

        K::ShiftLeft => PhysKeyCode::LeftShift,
        K::ShiftRight => PhysKeyCode::RightShift,
        K::CtrlLeft => PhysKeyCode::LeftControl,
        K::CtrlRight => PhysKeyCode::RightControl,
        K::AltLeft => PhysKeyCode::LeftAlt,
        K::AltRight => PhysKeyCode::RightAlt,
        K::MetaLeft => PhysKeyCode::LeftWindows,
        K::MetaRight => PhysKeyCode::RightWindows,
        K::Function => PhysKeyCode::Function,

        K::VolumeUp => PhysKeyCode::VolumeUp,
        K::VolumeDown => PhysKeyCode::VolumeDown,
        K::VolumeMute => PhysKeyCode::VolumeMute,

        _ => return None,
    })
}

/// The keys that have a fixed meaning in a terminal regardless of layout.
/// These are resolved without consulting the key character map, both because
/// the answer never varies and because the character map lookup costs a JNI
/// round trip per keystroke.
fn fixed_key_code(code: Keycode) -> Option<KeyCode> {
    use Keycode as K;
    Some(match code {
        // wezterm models these as control characters rather than named keys.
        K::Enter | K::NumpadEnter => KeyCode::Char('\r'),
        K::Tab => KeyCode::Char('\t'),
        K::Del => KeyCode::Char('\u{8}'),
        K::ForwardDel => KeyCode::Char('\u{7f}'),
        K::Escape => KeyCode::Char('\u{1b}'),
        K::Space => KeyCode::Char(' '),

        K::DpadUp => KeyCode::UpArrow,
        K::DpadDown => KeyCode::DownArrow,
        K::DpadLeft => KeyCode::LeftArrow,
        K::DpadRight => KeyCode::RightArrow,

        K::MoveHome => KeyCode::Home,
        K::MoveEnd => KeyCode::End,
        K::PageUp => KeyCode::PageUp,
        K::PageDown => KeyCode::PageDown,
        K::Insert => KeyCode::Insert,

        K::F1 => KeyCode::Function(1),
        K::F2 => KeyCode::Function(2),
        K::F3 => KeyCode::Function(3),
        K::F4 => KeyCode::Function(4),
        K::F5 => KeyCode::Function(5),
        K::F6 => KeyCode::Function(6),
        K::F7 => KeyCode::Function(7),
        K::F8 => KeyCode::Function(8),
        K::F9 => KeyCode::Function(9),
        K::F10 => KeyCode::Function(10),
        K::F11 => KeyCode::Function(11),
        K::F12 => KeyCode::Function(12),

        K::ShiftLeft => KeyCode::LeftShift,
        K::ShiftRight => KeyCode::RightShift,
        K::CtrlLeft => KeyCode::LeftControl,
        K::CtrlRight => KeyCode::RightControl,
        K::AltLeft => KeyCode::LeftAlt,
        K::AltRight => KeyCode::RightAlt,
        K::MetaLeft => KeyCode::LeftWindows,
        K::MetaRight => KeyCode::RightWindows,
        K::CapsLock => KeyCode::CapsLock,
        K::NumLock => KeyCode::NumLock,
        K::ScrollLock => KeyCode::ScrollLock,
        K::Break => KeyCode::Pause,
        K::Sysrq => KeyCode::PrintScreen,

        K::Numpad0 => KeyCode::Numpad(0),
        K::Numpad1 => KeyCode::Numpad(1),
        K::Numpad2 => KeyCode::Numpad(2),
        K::Numpad3 => KeyCode::Numpad(3),
        K::Numpad4 => KeyCode::Numpad(4),
        K::Numpad5 => KeyCode::Numpad(5),
        K::Numpad6 => KeyCode::Numpad(6),
        K::Numpad7 => KeyCode::Numpad(7),
        K::Numpad8 => KeyCode::Numpad(8),
        K::Numpad9 => KeyCode::Numpad(9),
        K::NumpadAdd => KeyCode::Add,
        K::NumpadSubtract => KeyCode::Subtract,
        K::NumpadMultiply => KeyCode::Multiply,
        K::NumpadDivide => KeyCode::Divide,
        K::NumpadDot => KeyCode::Decimal,

        _ => return None,
    })
}

/// The outcome of translating a single Android key event.
pub enum Translated {
    /// A complete key press.
    Key(KeyCode),
    /// A dead key was pressed; hold it and combine with the next character.
    DeadKey(char),
    /// Nothing a terminal can consume.
    None,
}

/// Resolve `code` to the key it produces, consulting `key_map` for anything
/// that depends on the physical keyboard's layout.
///
/// `key_map` is the character map of the originating input device, which is
/// what makes non-US physical keyboards work; when it is unavailable we fall
/// back to the meaning the key would have on a US layout.
pub fn translate_key(
    code: Keycode,
    meta: MetaState,
    key_map: Option<&KeyCharacterMap>,
) -> Translated {
    if let Some(key) = fixed_key_code(code) {
        return Translated::Key(key);
    }

    if let Some(map) = key_map {
        // Ask Android what this key produces under the current meta state.
        // Ctrl is deliberately masked out of the lookup: on a US layout
        // Ctrl-A must resolve to 'a' so that wezterm's own key assignment
        // and control-character logic can act on it, but Android would
        // return no character at all for that combination.
        let lookup_meta = MetaState(meta.0 & !(ANDROID_META_CTRL_MASK));
        match map.get(code, lookup_meta) {
            Ok(KeyMapChar::Unicode(c)) => return Translated::Key(KeyCode::Char(c)),
            Ok(KeyMapChar::CombiningAccent(c)) => return Translated::DeadKey(c),
            Ok(KeyMapChar::None) => {}
            Err(err) => {
                log::trace!("KeyCharacterMap::get({code:?}) failed: {err:#}");
            }
        }
    }

    // No character map, or the map had nothing to say. Fall back to the US
    // layout meaning of the physical key so that at least the ASCII range
    // works on a device where the JNI lookup is unavailable.
    if let Some(phys) = phys_key_code(code) {
        if let Some(c) = us_layout_char(phys, meta.shift_on()) {
            return Translated::Key(KeyCode::Char(c));
        }
        return Translated::Key(KeyCode::Physical(phys));
    }

    Translated::None
}

/// AMETA_CTRL_ON | AMETA_CTRL_LEFT_ON | AMETA_CTRL_RIGHT_ON
const ANDROID_META_CTRL_MASK: u32 = 0x1000 | 0x2000 | 0x4000;

fn us_layout_char(phys: PhysKeyCode, shift: bool) -> Option<char> {
    use PhysKeyCode as P;
    let (lower, upper) = match phys {
        P::A => ('a', 'A'),
        P::B => ('b', 'B'),
        P::C => ('c', 'C'),
        P::D => ('d', 'D'),
        P::E => ('e', 'E'),
        P::F => ('f', 'F'),
        P::G => ('g', 'G'),
        P::H => ('h', 'H'),
        P::I => ('i', 'I'),
        P::J => ('j', 'J'),
        P::K => ('k', 'K'),
        P::L => ('l', 'L'),
        P::M => ('m', 'M'),
        P::N => ('n', 'N'),
        P::O => ('o', 'O'),
        P::P => ('p', 'P'),
        P::Q => ('q', 'Q'),
        P::R => ('r', 'R'),
        P::S => ('s', 'S'),
        P::T => ('t', 'T'),
        P::U => ('u', 'U'),
        P::V => ('v', 'V'),
        P::W => ('w', 'W'),
        P::X => ('x', 'X'),
        P::Y => ('y', 'Y'),
        P::Z => ('z', 'Z'),
        P::K0 => ('0', ')'),
        P::K1 => ('1', '!'),
        P::K2 => ('2', '@'),
        P::K3 => ('3', '#'),
        P::K4 => ('4', '$'),
        P::K5 => ('5', '%'),
        P::K6 => ('6', '^'),
        P::K7 => ('7', '&'),
        P::K8 => ('8', '*'),
        P::K9 => ('9', '('),
        P::Minus => ('-', '_'),
        P::Equal => ('=', '+'),
        P::LeftBracket => ('[', '{'),
        P::RightBracket => (']', '}'),
        P::Backslash => ('\\', '|'),
        P::Semicolon => (';', ':'),
        P::Quote => ('\'', '"'),
        P::Comma => (',', '<'),
        P::Period => ('.', '>'),
        P::Slash => ('/', '?'),
        P::Grave => ('`', '~'),
        P::Space => (' ', ' '),
        _ => return None,
    };
    Some(if shift { upper } else { lower })
}

pub fn key_is_down(action: KeyAction) -> Option<bool> {
    match action {
        KeyAction::Down | KeyAction::Multiple => Some(true),
        KeyAction::Up => Some(false),
        _ => None,
    }
}
