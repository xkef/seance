//! Client-side mux for seance.
//!
//! The frontend (renderer, input, window) interacts with terminals through
//! a [`Domain`] — an opaque seam that can be either in-process (paired with
//! `seance-mux-server::LocalDomain` over an `InProcessTransport`) or remote
//! (a future Unix/SSH/TLS transport under [M12]). This crate carries no VT
//! dependency on its own; everything goes through the wire protocol in
//! `seance-protocol`.
//!
//! - [`MuxClient`] holds the active `Domain`, materializes per-pane state
//!   into [`PaneView`]s, and is the entry point for selection, link
//!   detection, and frame retrieval.
//! - [`ProtocolDomain`] is the [`Domain`] impl that speaks the wire format
//!   over any [`seance_protocol::transport::Transport`].
//! - [`links`] detects URLs and paths and resolves them via configurable
//!   [`LinkRule`]s.
//!
//! [M12]: https://github.com/xkef/seance/issues/221

mod client;
mod domain;
mod error;
mod events;
mod history;
mod interaction;
pub mod links;
mod pane_view;
mod protocol_domain;

pub use client::{MuxClient, PaneHandle};
pub use domain::{Domain, PaneSpawnOptions};
pub use error::{PaneError, SpawnError};
pub use events::{ClientRefresh, DomainEvent, MuxEvent};
pub use history::{PaneFrameHistory, ReplayBatch};
pub use interaction::{HoverInput, PaneInteractionState};
pub use links::{
    DetectedLink, GridRange, LinkAction, LinkDetector, LinkHighlight, LinkModifiers, LinkRule,
    LinkSource, LinkTarget,
};
pub use pane_view::{PaneFrame, PaneView};
pub use protocol_domain::ProtocolDomain;
pub use seance_frame::SnapshotFrameSource;
pub use seance_protocol::clipboard::{ClipboardRequest, encode_osc52_reply};
pub use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, CursorShape, GridPos, PlacementSnapshot, Resize, Selection,
    SelectionGranularity, TerminalModes, ThemeColors,
};
pub use seance_protocol::identity::{DomainId, ImageId, ImageKey, PaneRef};
pub use seance_protocol::transport::{InProcessTransport, TransportFrame};

#[cfg(test)]
mod tests;
