//! Bridge between characters and physical key positions under the current
//! keyboard layout (TIS/UCKeyTranslate, ported from Handy's input.rs): the
//! synthesized Cmd+V needs the key that types "v", and hotkey capture needs
//! the physical position of whatever character gpui reported, since global
//! hotkeys register by position. TIS APIs are main-thread-only; callers
//! resolve on the main thread.

use std::ffi::c_void;

/// The ANSI "v" key, correct wherever the layout does not remap it.
pub const ANSI_V: u16 = 9;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn UCKeyTranslate(
        key_layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(cf: *const c_void);
}

const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;
/// `(cmdKey >> 8) & 0xFF`: layouts can remap what Cmd shortcuts produce, so
/// the paste scan must ask with the Command bit set.
const CMD_MODIFIER_STATE: u32 = 1;
const NO_MODIFIER_STATE: u32 = 0;

/// Physical positions whose typed character depends on the layout:
/// global-hotkey `Code` names paired with macOS virtual keycodes (Carbon's
/// kVK_ANSI_* values, matching global-hotkey's registration table). Named
/// keys (space, enter, F-keys) type the same everywhere and stay out of it.
const POSITIONAL: &[(&str, u16)] = &[
    ("KeyA", 0x00),
    ("KeyS", 0x01),
    ("KeyD", 0x02),
    ("KeyF", 0x03),
    ("KeyH", 0x04),
    ("KeyG", 0x05),
    ("KeyZ", 0x06),
    ("KeyX", 0x07),
    ("KeyC", 0x08),
    ("KeyV", 0x09),
    ("KeyB", 0x0b),
    ("KeyQ", 0x0c),
    ("KeyW", 0x0d),
    ("KeyE", 0x0e),
    ("KeyR", 0x0f),
    ("KeyY", 0x10),
    ("KeyT", 0x11),
    ("Digit1", 0x12),
    ("Digit2", 0x13),
    ("Digit3", 0x14),
    ("Digit4", 0x15),
    ("Digit6", 0x16),
    ("Digit5", 0x17),
    ("Equal", 0x18),
    ("Digit9", 0x19),
    ("Digit7", 0x1a),
    ("Minus", 0x1b),
    ("Digit8", 0x1c),
    ("Digit0", 0x1d),
    ("BracketRight", 0x1e),
    ("KeyO", 0x1f),
    ("KeyU", 0x20),
    ("BracketLeft", 0x21),
    ("KeyI", 0x22),
    ("KeyP", 0x23),
    ("KeyL", 0x25),
    ("KeyJ", 0x26),
    ("Quote", 0x27),
    ("KeyK", 0x28),
    ("Semicolon", 0x29),
    ("Backslash", 0x2a),
    ("Comma", 0x2b),
    ("Slash", 0x2c),
    ("KeyN", 0x2d),
    ("KeyM", 0x2e),
    ("Period", 0x2f),
    ("Backquote", 0x32),
];

/// The global-hotkey `Code` name for the physical key that types `c` under
/// the current layout, e.g. 'z' on QWERTZ resolves to "KeyY". `None` when no
/// unmodified key types the character or the position has no ANSI name.
pub fn positional_token_for_char(c: char) -> Option<&'static str> {
    let keycode = keycode_for_char(c)?;
    POSITIONAL
        .iter()
        .find(|(_, code)| *code == keycode)
        .map(|(name, _)| *name)
}

/// The character the current layout types for a stored `Code` name, so
/// keycaps show what is printed on the user's keyboard.
pub fn char_for_positional_token(token: &str) -> Option<char> {
    let keycode = POSITIONAL
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(token))
        .map(|(_, code)| *code)?;
    char_for_keycode(keycode)
}

fn keycode_for_char(wanted: char) -> Option<u16> {
    let wanted = wanted.to_lowercase().next()?;
    with_layout(|layout| {
        (0u16..128).find(|&keycode| translate(layout, keycode, NO_MODIFIER_STATE) == Some(wanted))
    })
}

fn char_for_keycode(keycode: u16) -> Option<char> {
    with_layout(|layout| translate(layout, keycode, NO_MODIFIER_STATE))
}

/// Scan the current layout for the keycode that types "v" with Cmd held;
/// falls back to the ANSI position. Must run on the main thread.
pub fn v_keycode() -> u16 {
    with_layout(|layout| {
        (0u16..128).find(|&keycode| {
            matches!(
                translate(layout, keycode, CMD_MODIFIER_STATE),
                Some('v') | Some('V')
            )
        })
    })
    .unwrap_or(ANSI_V)
}

fn with_layout<T>(f: impl FnOnce(*const c_void) -> Option<T>) -> Option<T> {
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let layout_data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let result = if layout_data.is_null() {
            None
        } else {
            f(CFDataGetBytePtr(layout_data) as *const c_void)
        };
        CFRelease(source);
        result
    }
}

fn translate(layout: *const c_void, keycode: u16, modifier_state: u32) -> Option<char> {
    let keyboard_type = unsafe { LMGetKbdType() } as u32;
    let mut dead_key_state = 0u32;
    let mut chars = [0u16; 4];
    let mut length = 0usize;
    let status = unsafe {
        UCKeyTranslate(
            layout,
            keycode,
            K_UC_KEY_ACTION_DISPLAY,
            modifier_state,
            keyboard_type,
            K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
            &mut dead_key_state,
            chars.len(),
            &mut length,
            chars.as_mut_ptr(),
        )
    };
    if status != 0 || length != 1 {
        return None;
    }
    char::from_u32(u32::from(chars[0]))
}
