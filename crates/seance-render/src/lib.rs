//! GPU-accelerated text rendering for séance.
//!
//! Owns the wgpu surface and pipelines, the glyph atlas, and the
//! text-shaping backend. Consumes a [`seance_frame::FrameSource`] per frame,
//! typically a [`seance_frame::SnapshotFrameSource`]. The text-shaping layer is
//! an implementation detail — this crate is the unit that would swap
//! cosmic-text for parley or a hand-rolled stack.

#![warn(missing_docs)]

mod gpu;
mod image;
mod renderer;
mod text;

pub use renderer::{CursorShape, HoveredLinkRange, RenderInputs, RendererConfig, TerminalRenderer};
