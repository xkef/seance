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
