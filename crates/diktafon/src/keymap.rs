//! Resolve which physical key produces "v" under the current keyboard layout,
//! so the synthesized Cmd+V works on non-ANSI layouts (ported from Handy's
//! input.rs). TIS APIs are main-thread-only; callers resolve on the main
//! thread and hand the keycode to the paste path.

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
/// the scan must ask with the Command bit set.
const CMD_MODIFIER_STATE: u32 = 1;

/// Scan the current layout for the keycode that types "v" with Cmd held;
/// falls back to the ANSI position. Must run on the main thread.
pub fn v_keycode() -> u16 {
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return ANSI_V;
        }
        let layout_data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let keycode = if layout_data.is_null() {
            ANSI_V
        } else {
            let layout = CFDataGetBytePtr(layout_data) as *const c_void;
            scan_for_v(layout).unwrap_or(ANSI_V)
        };
        CFRelease(source);
        keycode
    }
}

unsafe fn scan_for_v(layout: *const c_void) -> Option<u16> {
    let keyboard_type = unsafe { LMGetKbdType() } as u32;
    for keycode in 0u16..128 {
        let mut dead_key_state = 0u32;
        let mut chars = [0u16; 4];
        let mut length = 0usize;
        let status = unsafe {
            UCKeyTranslate(
                layout,
                keycode,
                K_UC_KEY_ACTION_DISPLAY,
                CMD_MODIFIER_STATE,
                keyboard_type,
                K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
                &mut dead_key_state,
                chars.len(),
                &mut length,
                chars.as_mut_ptr(),
            )
        };
        if status == 0
            && length == 1
            && (chars[0] == u16::from(b'v') || chars[0] == u16::from(b'V'))
        {
            return Some(keycode);
        }
    }
    None
}
