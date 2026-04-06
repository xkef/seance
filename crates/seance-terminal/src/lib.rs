//! High-level terminal emulator with GPU rendering, search, and selection.
//!
//! - [`Terminal`] — VT emulator + PTY + search + selection (one per pane)
//! - [`TerminalRenderer`] — GPU renderer (font atlas + wgpu pipeline)

mod gpu;
pub mod search;
pub mod selection;
mod renderer;
mod terminal;

pub use ghostty_renderer::{Blending, Color, Colorspace, OptColor};
pub use renderer::{CursorShape, Overlay, RendererConfig, TerminalRenderer};
pub use search::SearchMatch;
pub use selection::GridPos;
pub use libghostty_vt::terminal::ScrollViewport;
pub use terminal::{CursorState, Terminal, TerminalModes};
