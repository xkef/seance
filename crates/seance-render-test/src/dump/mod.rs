//! Layered dump helpers.
//!
//! Phase A exposes only [`dump_frame`], the ASCII grid + annotation
//! format used by L4 snapshot tests. Shaper / atlas / cell-builder
//! dumps arrive in later phases as sibling modules.

mod frame;

pub use frame::dump_frame;
