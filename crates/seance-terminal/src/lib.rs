//! Terminal emulator with GPU rendering and text selection.
//!
//! - [`Terminal`] — VT emulator + PTY + selection (one per pane)
//! - [`TerminalRenderer`] — GPU renderer (font atlas + wgpu pipeline)

mod gpu;
pub mod selection;
mod renderer;
mod terminal;

pub use renderer::{CursorShape, Overlay, RendererConfig, TerminalRenderer};
pub use selection::GridPos;
pub use terminal::{Terminal, TerminalModes};
