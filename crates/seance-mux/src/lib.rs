mod client;
mod domain;
mod error;
mod events;
mod history;
mod local;
mod pane_view;
mod protocol_domain;

pub use client::{MuxClient, PaneHandle};
pub use domain::{Domain, PaneSpawnOptions};
pub use error::{PaneError, SpawnError};
pub use events::{ClientRefresh, DomainEvent, MuxEvent};
pub use history::{PaneFrameHistory, ReplayBatch};
pub use local::LocalDomain;
pub use pane_view::{PaneFrame, PaneView};
pub use protocol_domain::ProtocolDomain;
pub use seance_frame::SnapshotFrameSource;
pub use seance_protocol::{
    CellAttrs, CellColor, CursorInfo, CursorShape, DomainId, GridPos, ImageId, ImageKey,
    InProcessTransport, LineContent, LineRange, PaneRef, PlacementSnapshot, Resize, Selection,
    SelectionGranularity, TerminalModes, ThemeColors, TransportFrame,
};

#[cfg(test)]
mod tests;
