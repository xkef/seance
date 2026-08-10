//! VT emulator + PTY + selection.
//!
//! Wraps libghostty-vt (state machine) and portable-pty (shell
//! spawner). External callers send actor commands and render immutable
//! [`VtSnapshot`] values through [`SnapshotFrameSource`]. [`spawn_vt_session`]
//! starts the Unix IO actor that owns VT + PTY.

mod clipboard;
mod core;
mod iterm;
mod kitty_graphics;
mod session;
mod snapshot_extraction;
mod terminal;

#[doc(hidden)]
pub mod test_support;

pub use clipboard::{ClipboardRequest, encode_osc52_reply};
pub use core::{DEFAULT_MAX_SCROLLBACK, VtCoreError};
pub use seance_frame::{
    CellView, CellVisitor, FrameSource, ImageInfo, ImageVisitor, PlacementLayer, PlacementVisitor,
    SnapshotFrameSource,
};
pub use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, CursorShape, DirtySnapshot, GridPos, PlacementSnapshot,
    Selection, SelectionGranularity, SnapshotCell, SnapshotImage, TerminalModes, VtSnapshot,
};
pub use session::{
    Resize, SnapshotSlot, SpawnError, ThemeColors, VtCommand, VtEvent, VtSessionError,
    VtSessionHandle, VtSessionOptions, spawn_vt_session,
};
