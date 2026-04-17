//! GPU-accelerated text rendering for séance.
//!
//! Owns the wgpu surface and pipelines, the glyph atlas, and the
//! text-shaping backend. Consumes a [`seance_vt::Terminal`] per frame
//! to produce a rendered grid. The text-shaping layer is an
//! implementation detail — this crate is the unit that would swap
//! cosmic-text for parley or a hand-rolled stack.

mod gpu;
mod renderer;
mod text;
mod theme;

pub use renderer::{CursorShape, RenderInputs, RendererConfig, TerminalRenderer};
