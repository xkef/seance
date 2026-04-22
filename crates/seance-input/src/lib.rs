//! Keyboard and mouse input handling.
//!
//! Translates winit events into VT escape sequences via
//! libghostty-vt's key and mouse encoders. App-level shortcuts (Cmd+Q,
//! clipboard, font size) are matched upstream before reaching here.

mod keymap;

use libghostty_vt::{key, mouse};
use seance_vt::TerminalModes;
use winit::event::{ElementState, KeyEvent, Modifiers};
use winit::keyboard::{ModifiersKeyState, PhysicalKey};

/// Result of encoding a VT-bound key event.
#[derive(Debug)]
pub enum VtInput {
    /// Raw bytes to write to the PTY.
    Write(Vec<u8>),
    /// The event produced nothing to forward.
    Ignore,
}

/// macOS "option-as-alt" policy.
///
/// On macOS, Option serves double duty: it's both the VT Alt modifier
/// (readline `Alt+f`/`Alt+b`, vim `<M-…>`) and the OS text composer
/// (`Opt+o` → `ø`). This enum picks which role Option plays per side.
/// Ignored on non-macOS — Alt is always Alt there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OptionAsAlt {
    /// Both Option keys compose macOS special characters. macOS-friendly
    /// default — preserves `ø`/`¬`/`–` input.
    #[default]
    None,
    /// Only left-Option sends ESC-prefix; right-Option still composes.
    Left,
    /// Only right-Option sends ESC-prefix; left-Option still composes.
    Right,
    /// Both Option keys send ESC-prefix. Breaks macOS text composition.
    Both,
}

/// Translates winit events into VT bytes.
pub struct InputHandler {
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
    option_as_alt: OptionAsAlt,
}

impl Default for InputHandler {
    fn default() -> Self {
        Self {
            key_encoder: key::Encoder::new().expect("key encoder"),
            mouse_encoder: mouse::Encoder::new().expect("mouse encoder"),
            option_as_alt: OptionAsAlt::default(),
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the macOS option-as-alt policy. Takes effect on the next key
    /// event — no queued bytes to reinterpret.
    pub fn set_option_as_alt(&mut self, mode: OptionAsAlt) {
        self.option_as_alt = mode;
    }

    /// Encode a key event as VT bytes (cursor keys, function keys, etc.).
    pub fn handle_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &Modifiers,
        modes: TerminalModes,
    ) -> VtInput {
        if event.state != ElementState::Pressed {
            return VtInput::Ignore;
        }
        let bytes = self.encode_key(event, modifiers, modes);
        if bytes.is_empty() {
            VtInput::Ignore
        } else {
            VtInput::Write(bytes)
        }
    }

    /// Encode a mouse wheel event as VT mouse sequences. Returns
    /// `None` when mouse tracking is off (caller should scroll the
    /// viewport locally instead).
    pub fn encode_mouse_wheel(&mut self, lines: i32, modes: TerminalModes) -> Option<Vec<u8>> {
        if !modes.mouse_tracking {
            return None;
        }
        self.mouse_encoder
            .set_tracking_mode(mouse::TrackingMode::Normal);
        self.mouse_encoder.set_format(if modes.mouse_format_sgr {
            mouse::Format::Sgr
        } else {
            mouse::Format::X10
        });

        let button = if lines > 0 {
            mouse::Button::Four
        } else {
            mouse::Button::Five
        };
        let mut out = Vec::new();
        for _ in 0..lines.unsigned_abs() {
            let mut event = mouse::Event::new().ok()?;
            event
                .set_action(mouse::Action::Press)
                .set_button(Some(button))
                .set_position(mouse::Position { x: 0.0, y: 0.0 });
            self.mouse_encoder.encode_to_vec(&event, &mut out).ok()?;
        }
        if out.is_empty() { None } else { Some(out) }
    }

    fn encode_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &Modifiers,
        modes: TerminalModes,
    ) -> Vec<u8> {
        self.key_encoder
            .set_cursor_key_application(modes.cursor_keys);

        let PhysicalKey::Code(code) = event.physical_key else {
            return Vec::new();
        };
        let Some(gk) = keymap::map_keycode(code) else {
            return Vec::new();
        };
        let Ok(mut key_event) = key::Event::new() else {
            return Vec::new();
        };

        let alt_as_alt = self.alt_is_alt(modifiers);
        let mut mods = keymap::map_mods(modifiers);
        if !alt_as_alt {
            // Option is acting as the OS text composer — the composed glyph
            // already lives in `event.text`, and keeping ALT on would cause
            // the encoder to emit an unwanted `ESC` prefix in front of it.
            mods.remove(key::Mods::ALT);
        }

        key_event
            .set_key(gk)
            .set_action(keymap::map_action(event.state))
            .set_mods(mods);

        // When ALT is "real" (ESC-prefix), we intentionally drop `event.text`
        // so the encoder picks the key's layout-independent ASCII and emits
        // `ESC <char>`. Otherwise the composed glyph (or plain typed char)
        // flows through unchanged. `alt_as_alt` already implies Alt is held,
        // so no separate guard is needed.
        if !alt_as_alt && let Some(text) = &event.text {
            key_event.set_utf8(Some(text.as_str()));
        }

        let mut buf = Vec::new();
        let _ = self.key_encoder.encode_to_vec(&key_event, &mut buf);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "encode_key: code={:?} text={:?} alt={} lalt={:?} ralt={:?} policy={:?} alt_as_alt={} mods={:?} -> {:02x?}",
                code,
                event.text.as_deref(),
                modifiers.state().alt_key(),
                modifiers.lalt_state(),
                modifiers.ralt_state(),
                self.option_as_alt,
                alt_as_alt,
                mods,
                buf,
            );
        }

        buf
    }

    /// Should the currently-held Alt be treated as a VT Alt (ESC-prefix)
    /// or passed through to OS text composition?
    ///
    /// Non-macOS: always Alt. macOS: per `option_as_alt`, resolved against
    /// the pressed side (`lalt_state` / `ralt_state`).
    fn alt_is_alt(&self, modifiers: &Modifiers) -> bool {
        let alt = modifiers.state().alt_key();
        let lalt = matches!(modifiers.lalt_state(), ModifiersKeyState::Pressed);
        let ralt = matches!(modifiers.ralt_state(), ModifiersKeyState::Pressed);
        alt_is_alt_for(
            self.option_as_alt,
            cfg!(target_os = "macos"),
            alt,
            lalt,
            ralt,
        )
    }
}

/// Pure decision: given the option-as-alt policy, whether we're on macOS,
/// and the three Alt-related modifier bits, should Alt act as a VT Alt
/// (ESC-prefix) rather than a macOS text composer?
fn alt_is_alt_for(
    mode: OptionAsAlt,
    is_macos: bool,
    alt_held: bool,
    lalt_pressed: bool,
    ralt_pressed: bool,
) -> bool {
    if !alt_held {
        return false;
    }
    if !is_macos {
        return true;
    }
    match mode {
        OptionAsAlt::None => false,
        OptionAsAlt::Both => true,
        OptionAsAlt::Left => lalt_pressed,
        OptionAsAlt::Right => ralt_pressed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_is_alt_non_macos_always_true_when_held() {
        for mode in [
            OptionAsAlt::None,
            OptionAsAlt::Left,
            OptionAsAlt::Right,
            OptionAsAlt::Both,
        ] {
            assert!(
                alt_is_alt_for(mode, false, true, false, false),
                "mode={mode:?}"
            );
        }
    }

    #[test]
    fn alt_is_alt_false_when_alt_not_held() {
        assert!(!alt_is_alt_for(
            OptionAsAlt::Both,
            true,
            false,
            false,
            false
        ));
        assert!(!alt_is_alt_for(
            OptionAsAlt::Both,
            false,
            false,
            false,
            false
        ));
    }

    #[test]
    fn alt_is_alt_macos_none_always_composes() {
        assert!(!alt_is_alt_for(OptionAsAlt::None, true, true, true, false));
        assert!(!alt_is_alt_for(OptionAsAlt::None, true, true, false, true));
    }

    #[test]
    fn alt_is_alt_macos_both_always_alt() {
        assert!(alt_is_alt_for(OptionAsAlt::Both, true, true, true, false));
        assert!(alt_is_alt_for(OptionAsAlt::Both, true, true, false, true));
    }

    #[test]
    fn alt_is_alt_macos_left_follows_lalt() {
        assert!(alt_is_alt_for(OptionAsAlt::Left, true, true, true, false));
        assert!(!alt_is_alt_for(OptionAsAlt::Left, true, true, false, true));
    }

    #[test]
    fn alt_is_alt_macos_right_follows_ralt() {
        assert!(alt_is_alt_for(OptionAsAlt::Right, true, true, false, true));
        assert!(!alt_is_alt_for(OptionAsAlt::Right, true, true, true, false));
    }
}
