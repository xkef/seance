//! VT mode flags queried from the terminal emulator.
//!
//! Owned by the VT layer (source of truth: the live VT state machine);
//! consumed by the input encoder (cursor-key mode, mouse tracking mode,
//! bracketed paste) and by the app layer.

#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
    pub synchronized_output: bool,
    pub bracketed_paste: bool,
}
