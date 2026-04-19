//! VT emulator + PTY + selection.
//!
//! Wraps libghostty-vt (state machine) and portable-pty (shell
//! spawner). Exposes a [`FrameSource`] trait that hides the VT API
//! from the renderer; the only implementation is
//! [`LibGhosttyFrameSource`].

mod frame;
mod frame_source;
mod kitty_placeholder;
mod modes;
pub mod selection;
mod terminal;

#[doc(hidden)]
pub mod test_support;

pub use frame::{
    CellAttrs, CellColor, CellView, CellVisitor, CursorInfo, CursorShape, FrameSource, ImageInfo,
    ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
pub use frame_source::LibGhosttyFrameSource;
pub use modes::TerminalModes;
pub use selection::GridPos;
pub use terminal::Terminal;
