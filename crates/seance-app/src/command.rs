//! App-level commands produced by global keybinds.
//!
//! Distinct from `seance_input::VtInput`: these are actions the app
//! itself handles (clipboard, font size, window lifecycle), not bytes
//! to forward to the PTY.

#[derive(Debug, Clone, Copy)]
pub enum AppCommand {
    Quit,
    CloseWindow,
    Copy,
    Paste,
    SelectAll,
    FontSizeDelta(i8),
    FontSizeReset,
}
