use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, NamedKey};

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
    ClosePaneNone,
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
}

impl InputHandler {
    pub fn new() -> Self {
        Self { mode: Mode::Normal }
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

    pub fn encode_mouse_wheel(&self, lines: i32, term_modes: TerminalModes) -> Option<Vec<u8>> {
        if term_modes.mouse_event == 0 {
            return None;
        }
        let mut out = Vec::new();
        let count = lines.unsigned_abs();
        for _ in 0..count {
            if term_modes.mouse_format_sgr {
                let button = if lines > 0 { 64 } else { 65 };
                out.extend_from_slice(format!("\x1b[<{};1;1M", button).as_bytes());
            } else {
                let button: u8 = if lines > 0 { 64 + 32 } else { 65 + 32 };
                out.extend_from_slice(&[b'\x1b', b'[', b'M', button, b'!', b'!']);
            }
        }
        Some(out)
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

        let bytes = key_to_bytes(event, modifiers, term_modes);
        if bytes.is_empty() {
            Action::Ignore
        } else {
            Action::WritePty(bytes)
        }
    }

    fn handle_prefix(&mut self, event: &KeyEvent) -> Action {
        self.mode = Mode::Normal;
        match &event.logical_key {
            Key::Character(c) => match c.as_str() {
                "|" | "\\" => Action::SplitHorizontal,
                "-" | "_" => Action::SplitVertical,
                "n" => Action::FocusNext,
                "p" => Action::FocusPrev,
                "x" => Action::ClosePaneNone,
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

fn key_to_bytes(
    event: &KeyEvent,
    modifiers: &winit::event::Modifiers,
    term_modes: TerminalModes,
) -> Vec<u8> {
    let ctrl = modifiers.state().control_key();

    // Application cursor mode (DECCKM) changes the CSI introducer
    // from `[` to `O` for arrow/home/end keys.
    let csi = if term_modes.cursor_keys { b'O' } else { b'[' };

    match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab => b"\t".to_vec(),
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::Space => b" ".to_vec(),
            NamedKey::ArrowUp => vec![b'\x1b', csi, b'A'],
            NamedKey::ArrowDown => vec![b'\x1b', csi, b'B'],
            NamedKey::ArrowRight => vec![b'\x1b', csi, b'C'],
            NamedKey::ArrowLeft => vec![b'\x1b', csi, b'D'],
            NamedKey::Home => vec![b'\x1b', csi, b'H'],
            NamedKey::End => vec![b'\x1b', csi, b'F'],
            NamedKey::Delete => b"\x1b[3~".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
            NamedKey::F1 => b"\x1bOP".to_vec(),
            NamedKey::F2 => b"\x1bOQ".to_vec(),
            NamedKey::F3 => b"\x1bOR".to_vec(),
            NamedKey::F4 => b"\x1bOS".to_vec(),
            NamedKey::F5 => b"\x1b[15~".to_vec(),
            NamedKey::F6 => b"\x1b[17~".to_vec(),
            NamedKey::F7 => b"\x1b[18~".to_vec(),
            NamedKey::F8 => b"\x1b[19~".to_vec(),
            NamedKey::F9 => b"\x1b[20~".to_vec(),
            NamedKey::F10 => b"\x1b[21~".to_vec(),
            NamedKey::F11 => b"\x1b[23~".to_vec(),
            NamedKey::F12 => b"\x1b[24~".to_vec(),
            _ => Vec::new(),
        },
        Key::Character(c) => {
            if ctrl {
                if let Some(ch) = c.chars().next() {
                    if ch.is_ascii_lowercase() {
                        return vec![ch as u8 - b'a' + 1];
                    }
                }
            }
            c.as_bytes().to_vec()
        }
        _ => Vec::new(),
    }
}
