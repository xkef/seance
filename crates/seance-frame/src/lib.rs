//! Render-facing frame traits.
//!
//! [`FrameSource`] is the contract the renderer reads each frame; visitor
//! traits ([`CellVisitor`], [`PlacementVisitor`], [`ImageVisitor`]) keep
//! the traversal allocation-free so the hot path never heap-allocates per
//! cell. [`SnapshotFrameSource`] is the adapter that bridges an owned
//! [`seance_protocol::frame::VtSnapshot`] into the trait — the seam
//! between protocol data (owned, serializable) and the live render path
//! (borrowed, throwaway).

mod layer;
mod snapshot_source;
mod source;

pub use layer::{PlacementLayer, SubRole, Z_MAIN, Z_WINDOW_BG};
pub use snapshot_source::SnapshotFrameSource;
pub use source::{CellView, CellVisitor, FrameSource, ImageInfo, ImageVisitor, PlacementVisitor};
