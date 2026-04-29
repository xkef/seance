//! Text shaping, rasterization, and atlas packing.
//!
//! The [`TextBackend`][backend::TextBackend] trait is the lone seam
//! where shaping and rasterization can be swapped. [`GlyphAtlas`] is
//! a shared data structure, not a swap point. [`CellBuilder`] drives
//! the backend and atlas together to produce per-frame GPU records.

mod atlas;
pub mod backend;
mod cell_builder;
pub mod cosmic;
mod shape_cache;

pub(crate) use atlas::GlyphAtlas;
pub(crate) use cell_builder::{BuildFrameConfig, CellBuilder, CellText, FrameInfo};
