//! Terminal view: pairs a VT emulator with a PTY and manages their I/O.
//!
//! This is the unit that the multiplexer operates on — each pane owns one
//! `TerminalView`. The view knows nothing about rendering or GPU state;
//! it just maintains terminal content and shuttles bytes between the PTY
//! and the VT parser.

use ghostty_renderer::{ScrollAction, Terminal};
use seance_input::TerminalModes;
use seance_pty::Pty;

pub struct TerminalView {
    terminal: Terminal,
    pty: Pty,
    /// New PTY data arrived since the last `take_dirty()` call.
    dirty: bool,
}

impl TerminalView {
    /// Spawn a new shell in a PTY with the given grid dimensions.
    pub fn spawn(cols: u16, rows: u16) -> Self {
        let terminal = Terminal::new(cols, rows).expect("failed to create terminal");
        let pty = Pty::spawn(seance_pty::Size { cols, rows }).expect("failed to spawn PTY");
        Self {
            terminal,
            pty,
            dirty: false,
        }
    }

    /// Borrow the underlying terminal (for `Renderer::set_terminal`).
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// Read all available PTY output into the terminal. Returns `true`
    /// if any bytes were read.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let mut got_data = false;
        loop {
            match self.pty.read(&mut buf) {
                Ok(n) if n > 0 => {
                    self.terminal.vt_write(&buf[..n]);
                    got_data = true;
                }
                _ => break,
            }
        }
        // Drain device-status responses back to the PTY.
        // drain_responses borrows &mut self.terminal; we must write
        // before calling any other mutating terminal method.
        let responses = self.terminal.drain_responses();
        if !responses.is_empty() {
            let _ = self.pty.write_all(responses);
        }
        self.terminal.clear_responses();
        if got_data {
            self.dirty = true;
        }
        got_data
    }

    /// Write raw bytes to the PTY (keyboard input).
    pub fn write(&self, data: &[u8]) {
        let _ = self.pty.write_all(data);
    }

    /// Resize both the terminal grid and the PTY slave.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.terminal.resize(cols, rows);
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    pub fn scroll(&mut self, action: ScrollAction) {
        self.terminal.scroll(action);
    }

    /// Returns `true` if new data has arrived since the last call,
    /// and clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Query terminal modes for input encoding (read-only).
    pub fn modes(&self) -> TerminalModes {
        TerminalModes {
            cursor_keys: self.terminal.mode_cursor_keys(),
            mouse_event: self.terminal.mode_mouse_event(),
            mouse_format_sgr: self.terminal.mode_mouse_format_sgr(),
        }
    }
}
