//! Render-side frame abstractions: the [`FrameSource`] trait the renderer
//! pulls cells/cursor/placements/images from each frame, plus the
//! [`SnapshotFrameSource`] adapter that backs it with a
//! [`seance_protocol::frame::VtSnapshot`].
//!
//! The renderer never reaches into VT or transport state directly; it sees
//! only the visitors below. See `docs/architecture.md` for the rendering
//! pipeline.

#![warn(missing_docs)]

mod layer;
mod snapshot_source;
mod source;

pub use layer::PlacementLayer;
pub use snapshot_source::SnapshotFrameSource;
pub use source::{CellView, CellVisitor, FrameSource, ImageInfo, ImageVisitor, PlacementVisitor};
