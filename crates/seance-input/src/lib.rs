mod keymap;

use libghostty_vt::{key, mouse};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, NamedKey, PhysicalKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prefix,
    Copy,
}

#[derive(Debug)]
pub enum Action {
    WritePty(Vec<u8>),
    SplitHorizontal,
    SplitVertical,
    FocusNext,
    FocusPrev,
    ClosePane,
    Zoom,
    EnterCopyMode,
    Detach,
    Ignore,
}

/// Terminal modes that affect input encoding.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
}

pub struct InputHandler {
    mode: Mode,
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
}

impl Default for InputHandler {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            key_encoder: key::Encoder::new().expect("key encoder"),
            mouse_encoder: mouse::Encoder::new().expect("mouse encoder"),
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn handle_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> Action {
        if event.state != ElementState::Pressed {
            return Action::Ignore;
        }

        match self.mode {
            Mode::Normal => self.handle_normal(event, modifiers, term_modes),
            Mode::Prefix => self.handle_prefix(event),
            Mode::Copy => Action::Ignore,
        }
    }

    pub fn encode_mouse_wheel(&mut self, lines: i32, term_modes: TerminalModes) -> Option<Vec<u8>> {
        if term_modes.mouse_event == 0 {
            return None;
        }

        self.mouse_encoder.set_tracking_mode(mouse::TrackingMode::Normal);
        if term_modes.mouse_format_sgr {
            self.mouse_encoder.set_format(mouse::Format::Sgr);
        } else {
            self.mouse_encoder.set_format(mouse::Format::X10);
        }

        let mut out = Vec::new();
        let count = lines.unsigned_abs();
        let button = if lines > 0 {
            mouse::Button::Four
        } else {
            mouse::Button::Five
        };

        for _ in 0..count {
            let mut event = mouse::Event::new().ok()?;
            event
                .set_action(mouse::Action::Press)
                .set_button(Some(button))
                .set_position(mouse::Position { x: 0.0, y: 0.0 });
            self.mouse_encoder.encode_to_vec(&event, &mut out).ok()?;
        }

        if out.is_empty() { None } else { Some(out) }
    }

    fn handle_normal(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> Action {
        let ctrl = modifiers.state().control_key();

        if ctrl {
            if let Key::Named(NamedKey::Space) = &event.logical_key {
                self.mode = Mode::Prefix;
                return Action::Ignore;
            }
        }

        let bytes = self.encode_key(event, modifiers, term_modes);
        if bytes.is_empty() {
            Action::Ignore
        } else {
            Action::WritePty(bytes)
        }
    }

    fn encode_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> Vec<u8> {
        self.key_encoder
            .set_cursor_key_application(term_modes.cursor_keys);

        let physical_key = match event.physical_key {
            PhysicalKey::Code(code) => keymap::map_keycode(code),
            _ => None,
        };
        let Some(gk) = physical_key else {
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
        buf
    }

    fn handle_prefix(&mut self, event: &KeyEvent) -> Action {
        self.mode = Mode::Normal;
        match &event.logical_key {
            Key::Character(c) => match c.as_str() {
                "|" | "\\" => Action::SplitHorizontal,
                "-" | "_" => Action::SplitVertical,
                "n" => Action::FocusNext,
                "p" => Action::FocusPrev,
                "x" => Action::ClosePane,
                "z" => Action::Zoom,
                "[" => {
                    self.mode = Mode::Copy;
                    Action::EnterCopyMode
                }
                "d" => Action::Detach,
                _ => Action::Ignore,
            },
            Key::Named(NamedKey::Space) => {
                Action::WritePty(b" ".to_vec())
            }
            _ => Action::Ignore,
        }
    }
}
