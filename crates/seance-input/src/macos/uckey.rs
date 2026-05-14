//! Dead-key composition via `UCKeyTranslate`.
//!
//! winit's `KeyEvent::text` is sourced from `NSEvent.characters`, which on
//! non-US layouts withholds dead-key glyphs (`Opt+n` reports `n` instead of
//! `~`) until the composition resolves on the *follow-up* keypress. The
//! terminal needs the glyph at press time, so we ask the Text Input Source
//! for the active layout and call `UCKeyTranslate` with
//! `kUCKeyTranslateNoDeadKeysMask` to force resolution per press.
//!
//! Only the FFI surface lives here. The composer-side gate (when the
//! configured `OptionAsAlt` policy is consulted) lives in `lib.rs` so it
//! stays unit-testable on non-macOS hosts.

use std::cell::Cell;
use std::ffi::c_void;
use std::os::raw::c_ulong;
use std::ptr;

use winit::event::Modifiers;
use winit::keyboard::{KeyCode, ModifiersKeyState};

type CFTypeRef = *const c_void;
type TISInputSourceRef = CFTypeRef;
type CFDataRef = CFTypeRef;
type CFStringRef = CFTypeRef;
type UniCharCount = c_ulong;

const KEY_ACTION_DOWN: u16 = 0;
const NO_DEAD_KEYS_MASK: u32 = 1;

// Carbon modifier byte (NSEventModifierFlags >> 8 & 0xff).
const CARBON_SHIFT: u32 = 0x02;
const CARBON_OPTION: u32 = 0x08;

#[allow(non_snake_case, non_upper_case_globals)]
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISGetInputSourceProperty(source: TISInputSourceRef, property: CFStringRef) -> CFTypeRef;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        key_layout_ptr: *const u8,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: UniCharCount,
        actual_string_length: *mut UniCharCount,
        unicode_string: *mut u16,
    ) -> i32;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
}

#[allow(non_snake_case)]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
}

/// Holds a non-owning pointer into the active layout's `UCKeyboardLayout`
/// bytes. The Text Input Source that owns those bytes is intentionally
/// leaked for the process lifetime — TIS returns the same pointer while
/// the layout is stable, and v1 doesn't subscribe to layout-change
/// notifications. Users restart on layout swap.
pub struct UcKey {
    layout: Cell<*const u8>,
}

impl UcKey {
    pub fn new() -> Self {
        Self {
            layout: Cell::new(ptr::null()),
        }
    }

    /// Returns the composed character for `code` under `mods`, or `None`
    /// when the key has no text mapping, the layout is unavailable, or
    /// `UCKeyTranslate` produced an empty string.
    pub fn translate(&self, code: KeyCode, mods: &Modifiers) -> Option<String> {
        let virtual_key = map_keycode(code)?;
        let layout = self.layout_ptr()?;
        let mod_byte = carbon_mod_byte(mods);

        let mut dead_state: u32 = 0;
        let mut buf = [0u16; 4];
        let mut actual: UniCharCount = 0;

        let status = unsafe {
            UCKeyTranslate(
                layout,
                virtual_key,
                KEY_ACTION_DOWN,
                mod_byte,
                u32::from(LMGetKbdType()),
                NO_DEAD_KEYS_MASK,
                &mut dead_state,
                buf.len() as UniCharCount,
                &mut actual,
                buf.as_mut_ptr(),
            )
        };
        if status != 0 || actual == 0 {
            return None;
        }
        let slice = &buf[..actual as usize];
        let s: String = char::decode_utf16(slice.iter().copied())
            .filter_map(Result::ok)
            .collect();
        if s.is_empty() { None } else { Some(s) }
    }

    fn layout_ptr(&self) -> Option<*const u8> {
        let cached = self.layout.get();
        if !cached.is_null() {
            return Some(cached);
        }
        let ptr = unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            if data.is_null() {
                return None;
            }
            CFDataGetBytePtr(data)
        };
        if ptr.is_null() {
            return None;
        }
        self.layout.set(ptr);
        Some(ptr)
    }
}

fn carbon_mod_byte(mods: &Modifiers) -> u32 {
    let state = mods.state();
    let mut byte = 0;
    if state.shift_key() {
        byte |= CARBON_SHIFT;
    }
    if state.alt_key() {
        byte |= CARBON_OPTION;
    }
    byte
}

/// True when right-Option is the side currently held. Mirrors
/// `keymap::map_mods`, which sets `ALT_SIDE` only when `ralt_state` is
/// `Pressed` because `lalt_state` may report `Unknown` on some platforms.
pub fn right_option_held(mods: &Modifiers) -> bool {
    matches!(mods.ralt_state(), ModifiersKeyState::Pressed)
}

fn map_keycode(code: KeyCode) -> Option<u16> {
    Some(match code {
        KeyCode::KeyA => 0x00,
        KeyCode::KeyB => 0x0B,
        KeyCode::KeyC => 0x08,
        KeyCode::KeyD => 0x02,
        KeyCode::KeyE => 0x0E,
        KeyCode::KeyF => 0x03,
        KeyCode::KeyG => 0x05,
        KeyCode::KeyH => 0x04,
        KeyCode::KeyI => 0x22,
        KeyCode::KeyJ => 0x26,
        KeyCode::KeyK => 0x28,
        KeyCode::KeyL => 0x25,
        KeyCode::KeyM => 0x2E,
        KeyCode::KeyN => 0x2D,
        KeyCode::KeyO => 0x1F,
        KeyCode::KeyP => 0x23,
        KeyCode::KeyQ => 0x0C,
        KeyCode::KeyR => 0x0F,
        KeyCode::KeyS => 0x01,
        KeyCode::KeyT => 0x11,
        KeyCode::KeyU => 0x20,
        KeyCode::KeyV => 0x09,
        KeyCode::KeyW => 0x0D,
        KeyCode::KeyX => 0x07,
        KeyCode::KeyY => 0x10,
        KeyCode::KeyZ => 0x06,
        KeyCode::Digit1 => 0x12,
        KeyCode::Digit2 => 0x13,
        KeyCode::Digit3 => 0x14,
        KeyCode::Digit4 => 0x15,
        KeyCode::Digit5 => 0x17,
        KeyCode::Digit6 => 0x16,
        KeyCode::Digit7 => 0x1A,
        KeyCode::Digit8 => 0x1C,
        KeyCode::Digit9 => 0x19,
        KeyCode::Digit0 => 0x1D,
        KeyCode::Minus => 0x1B,
        KeyCode::Equal => 0x18,
        KeyCode::BracketLeft => 0x21,
        KeyCode::BracketRight => 0x1E,
        KeyCode::Backslash => 0x2A,
        KeyCode::Semicolon => 0x29,
        KeyCode::Quote => 0x27,
        KeyCode::Backquote => 0x32,
        KeyCode::Comma => 0x2B,
        KeyCode::Period => 0x2F,
        KeyCode::Slash => 0x2C,
        // `kVK_ISO_Section` — the §/± slot above Tab on ISO layouts; on
        // Swiss German it carries the `<`/`>` glyphs and is the
        // physical key for `Opt+^` (grave dead key).
        KeyCode::IntlBackslash => 0x0A,
        KeyCode::IntlYen => 0x5D,
        KeyCode::IntlRo => 0x5E,
        KeyCode::Space => 0x31,
        _ => return None,
    })
}
