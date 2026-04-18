//! VT mode flags queried from the emulator.
//!
//! Source of truth is the live VT state machine; consumed by the input
//! encoder (cursor-key mode, mouse tracking, bracketed paste).

#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_tracking: bool,
    pub mouse_format_sgr: bool,
    pub bracketed_paste: bool,
}
