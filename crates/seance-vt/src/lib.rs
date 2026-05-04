//! VT emulator + PTY + selection.
//!
//! Wraps libghostty-vt (state machine) and portable-pty (shell
//! spawner). Exposes a [`FrameSource`] trait that hides the VT API
//! from the renderer; live libghostty state is adapted by
//! [`LibGhosttyFrameSource`], and owned snapshots are adapted by
//! [`SnapshotFrameSource`]. [`spawn_vt_session`] starts the Unix IO actor
//! that owns VT + PTY and publishes [`VtSnapshot`] values.

mod frame;
mod frame_source;
mod kitty_placeholder;
mod modes;
pub mod selection;
mod session;
mod snapshot;
mod terminal;

#[doc(hidden)]
pub mod test_support;

pub use frame::{
    CellAttrs, CellColor, CellView, CellVisitor, CursorInfo, CursorShape, DirtySnapshot,
    FrameSource, ImageInfo, ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
pub use frame_source::LibGhosttyFrameSource;
pub use modes::TerminalModes;
pub use selection::{GridPos, Selection, SelectionGranularity};
pub use session::{
    Resize, SnapshotSlot, SpawnError, ThemeColors, VtCommand, VtEvent, VtSessionError,
    VtSessionHandle, VtSessionOptions, spawn_vt_session,
};
pub use snapshot::{SnapshotCell, SnapshotFrameSource, SnapshotImage, VtSnapshot};
pub use terminal::{Terminal, install_png_decoder_for_this_thread};
