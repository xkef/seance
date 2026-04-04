//! wgpu-based renderer for seance.
//!
//! Consumes Level 2 data from libghostty-renderer (cell buffers + glyph
//! atlas textures) and renders all terminal panes into a single wgpu
//! surface. This gives seance full control over the frame for
//! flicker-free split rendering and atomic presentation.

mod pipeline;
mod state;
mod uniforms;

pub use state::GpuState;
pub use uniforms::Uniforms;
