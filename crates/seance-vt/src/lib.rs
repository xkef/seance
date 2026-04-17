//! VT emulator + PTY + selection.
//!
//! Wraps libghostty-vt as the VT state machine and portable-pty as the
//! shell spawner. Owns selection state over the VT grid and exposes
//! terminal mode flags consumed by the input encoder and the app.
//!
//! Also defines the `FrameSource` trait — the wall between VT state
//! and the renderer. The sole implementation lives in
//! [`LibGhosttyFrameSource`].

mod frame;
mod frame_source;
mod modes;
pub mod selection;
mod terminal;

pub use frame::{CellColor, CellView, CellVisitor, CursorInfo, FrameSource};
pub use frame_source::LibGhosttyFrameSource;
pub use libghostty_vt::Terminal as VtTerminal;
pub use modes::TerminalModes;
pub use selection::GridPos;
pub use terminal::Terminal;
