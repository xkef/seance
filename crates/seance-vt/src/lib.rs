//! VT emulator + PTY + selection.
//!
//! Wraps libghostty-vt (state machine) and portable-pty (shell
//! spawner). External callers send actor commands and render immutable
//! [`VtSnapshot`] values through [`SnapshotFrameSource`]. [`spawn_vt_session`]
//! starts the Unix IO actor that owns VT + PTY.

mod core;
mod frame;
mod kitty_graphics;
mod modes;
pub mod selection;
mod session;
mod snapshot;
mod snapshot_extraction;
mod terminal;

#[doc(hidden)]
pub mod test_support;

pub use core::VtCoreError;
pub use frame::{
    CellAttrs, CellColor, CellView, CellVisitor, CursorInfo, CursorShape, DirtySnapshot,
    FrameSource, ImageInfo, ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
pub use modes::TerminalModes;
pub use selection::{GridPos, Selection, SelectionGranularity};
pub use session::{
    Resize, SnapshotSlot, SpawnError, ThemeColors, VtCommand, VtEvent, VtSessionError,
    VtSessionHandle, VtSessionOptions, spawn_vt_session,
};
pub use snapshot::{SnapshotCell, SnapshotFrameSource, SnapshotImage, VtSnapshot};
