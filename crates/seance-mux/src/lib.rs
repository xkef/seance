//! Multiplexer client and domain abstractions.
//!
//! A [`MuxClient`] talks to a [`Domain`] — either an in-process
//! [`LocalDomain`] that owns VT actors directly, or a [`ProtocolDomain`]
//! that translates [`MuxClient`] calls into wire messages from
//! `seance-protocol`. The client holds per-pane [`PaneView`]s
//! materialised from frame deltas, exposes [`PaneFrame`]s the renderer
//! can consume, and surfaces refreshes via [`ClientRefresh`].
//!
//! Link detection (`pub mod links`) layers on top: it walks
//! [`seance_protocol::frame::VtSnapshot`] cells and OSC 8 hyperlink runs
//! to find activatable URLs and paths under the cursor.
//!
//! See `docs/architecture.md` for the mux model and threading boundary.

#![warn(missing_docs)]

mod client;
mod domain;
mod error;
mod events;
mod history;
pub mod links;
mod local;
mod pane_view;
mod protocol_domain;

pub use client::{MuxClient, PaneHandle};
pub use domain::{Domain, PaneSpawnOptions};
pub use error::{PaneError, SpawnError};
pub use events::{ClientRefresh, DomainEvent, MuxEvent};
pub use history::{PaneFrameHistory, ReplayBatch};
pub use links::{
    DetectedLink, GridRange, LinkAction, LinkDetector, LinkHighlight, LinkModifiers, LinkRule,
    LinkSource, LinkTarget,
};
pub use local::LocalDomain;
pub use pane_view::{PaneFrame, PaneView};
pub use protocol_domain::ProtocolDomain;
pub use seance_frame::SnapshotFrameSource;
pub use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, CursorShape, GridPos, LineRange, PlacementSnapshot, Resize,
    Selection, SelectionGranularity, TerminalModes, ThemeColors,
};
pub use seance_protocol::identity::{DomainId, ImageId, ImageKey, PaneRef};
pub use seance_protocol::mux::LineContent;
pub use seance_protocol::transport::{InProcessTransport, TransportFrame};

#[cfg(test)]
mod tests;
