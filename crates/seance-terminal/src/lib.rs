//! Terminal emulator with GPU rendering and text selection.
//!
//! - [`Terminal`] — VT emulator + PTY + selection (one per pane)
//! - [`TerminalRenderer`] — GPU renderer (font atlas + wgpu pipeline)

mod gpu;
mod renderer;
pub mod selection;
mod terminal;

pub use renderer::{CursorShape, Overlay, RendererConfig, TerminalRenderer};
pub use seance_input::TerminalModes;
pub use selection::GridPos;
pub use terminal::Terminal;
