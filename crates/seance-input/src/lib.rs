//! Keyboard and mouse input handling.
//!
//! Translates winit events into VT escape sequences via
//! libghostty-vt's key and mouse encoders. App-level shortcuts (Cmd+Q,
//! clipboard, font size) are matched upstream before reaching here.

mod keymap;

use libghostty_vt::{key, mouse};
use seance_vt::TerminalModes;
use winit::event::{ElementState, KeyEvent, Modifiers};
use winit::keyboard::{Key, PhysicalKey};
#[cfg(target_os = "macos")]
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

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

impl OptionAsAlt {
    fn to_libghostty(self) -> key::OptionAsAlt {
        match self {
            OptionAsAlt::None => key::OptionAsAlt::False,
            OptionAsAlt::Left => key::OptionAsAlt::Left,
            OptionAsAlt::Right => key::OptionAsAlt::Right,
            OptionAsAlt::Both => key::OptionAsAlt::True,
        }
    }
}

/// Translates winit events into VT bytes.
pub struct InputHandler {
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
}

impl Default for InputHandler {
    fn default() -> Self {
        let mut key_encoder = key::Encoder::new().expect("key encoder");
        // Enable DEC mode 1036 so ALT+<char> produces `ESC <char>` (matches
        // xterm/Ghostty defaults). Without this, the encoder drops the ALT
        // bit and just emits the un-prefixed character.
        key_encoder.set_alt_esc_prefix(true);
        Self {
            key_encoder,
            mouse_encoder: mouse::Encoder::new().expect("mouse encoder"),
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the macOS option-as-alt policy. The encoder uses this (together
    /// with the `ALT_SIDE` bit on each event's mods) to decide whether to
    /// emit `ESC`-prefix for Option+key or pass the composed text through.
    pub fn set_option_as_alt(&mut self, mode: OptionAsAlt) {
        self.key_encoder
            .set_macos_option_as_alt(mode.to_libghostty());
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

        key_event
            .set_key(gk)
            .set_action(keymap::map_action(event.state))
            .set_mods(keymap::map_mods(modifiers));
        if let Some(text) = &event.text {
            key_event.set_utf8(Some(text.as_str()));
        }

        let mut buf = Vec::new();
        let _ = self.key_encoder.encode_to_vec(&key_event, &mut buf);

        if log::log_enabled!(log::Level::Trace) {
            let logical = match &event.logical_key {
                Key::Character(s) => Some(s.as_str()),
                _ => None,
            };
            #[cfg(target_os = "macos")]
            let all_mods = event.text_with_all_modifiers();
            #[cfg(not(target_os = "macos"))]
            let all_mods: Option<&str> = None;
            log::trace!(
                "encode_key: code={:?} text={:?} logical={:?} all_mods={:?} mods={:?} -> {:02x?}",
                code,
                event.text.as_deref(),
                logical,
                all_mods,
                key_event.mods(),
                buf,
            );
        }

        buf
    }
}
