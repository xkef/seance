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

    pub fn handle_key(&mut self, event: &KeyEvent, modifiers: &winit::event::Modifiers) -> Action {
        if event.state != ElementState::Pressed {
            return Action::Ignore;
        }

        match self.mode {
            Mode::Normal => self.handle_normal(event, modifiers),
            Mode::Prefix => self.handle_prefix(event),
            Mode::Copy => Action::Ignore,
        }
    }

    fn handle_normal(&mut self, event: &KeyEvent, modifiers: &winit::event::Modifiers) -> Action {
        let ctrl = modifiers.state().control_key();

        if ctrl {
            if let Key::Named(NamedKey::Space) = &event.logical_key {
                self.mode = Mode::Prefix;
                return Action::Ignore;
            }
        }

        let bytes = key_to_bytes(event, modifiers);
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

fn key_to_bytes(event: &KeyEvent, modifiers: &winit::event::Modifiers) -> Vec<u8> {
    let ctrl = modifiers.state().control_key();

    match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab => b"\t".to_vec(),
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::ArrowUp => b"\x1b[A".to_vec(),
            NamedKey::ArrowDown => b"\x1b[B".to_vec(),
            NamedKey::ArrowRight => b"\x1b[C".to_vec(),
            NamedKey::ArrowLeft => b"\x1b[D".to_vec(),
            NamedKey::Home => b"\x1b[H".to_vec(),
            NamedKey::End => b"\x1b[F".to_vec(),
            NamedKey::Delete => b"\x1b[3~".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
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
