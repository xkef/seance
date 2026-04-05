//! High-level terminal emulator with GPU rendering, search, and selection.
//!
//! `seance-terminal` provides two main types:
//!
//! - [`Terminal`] — a managed terminal pane with VT emulation, PTY I/O,
//!   text search, and selection. Each multiplexer pane owns one.
//!
//! - [`TerminalRenderer`] — a GPU-accelerated renderer that composites
//!   terminal content onto a window surface. Owns the font engine, glyph
//!   atlas, and wgpu pipeline.
//!
//! # Architecture
//!
//! ```text
//!  ┌─────────────────────────────────────────────┐
//!  │ seance-terminal (this crate)                 │
//!  │                                              │
//!  │  Terminal          TerminalRenderer           │
//!  │  ┌──────────┐     ┌──────────────────┐      │
//!  │  │ VT state  │     │ ghostty Renderer │      │
//!  │  │ PTY I/O   │     │ GPU pipeline     │      │
//!  │  │ Search    │     │ Atlas textures   │      │
//!  │  │ Selection │     └──────────────────┘      │
//!  │  └──────────┘                                │
//!  └─────────────────────────────────────────────┘
//!           │                    │
//!  ghostty-renderer    wgpu (internal)
//!  seance-pty
//! ```
//!
//! The GPU pipeline (`gpu/`) is an internal module — consumers interact
//! only with the safe, documented public API.
//!
//! # Example
//!
//! ```no_run
//! use seance_terminal::{Terminal, TerminalRenderer, RendererConfig};
//!
//! // Create a terminal pane
//! let mut term = Terminal::spawn(80, 24).unwrap();
//! term.write(b"echo hello\r");
//! term.poll();
//!
//! // Search
//! let n = term.search("hello");
//! println!("found {} matches", n);
//!
//! // Selection
//! term.start_selection(0, 0);
//! term.update_selection(10, 0);
//! if let Some(text) = term.selection_text() {
//!     println!("selected: {text}");
//! }
//! ```

mod gpu;
pub mod search;
pub mod selection;
mod renderer;
mod terminal;

// Re-export the public API.
pub use ghostty_renderer::{Blending, Color, Colorspace, OptColor, ScrollAction};
pub use renderer::{CursorShape, Overlay, RendererConfig, TerminalRenderer};
pub use search::SearchMatch;
pub use selection::GridPos;
pub use terminal::{CursorState, Terminal, TerminalModes};
