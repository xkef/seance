//! Bundled test fonts.
//!
//! Phase A ships with no font binaries: the `dump_frame()` layer reads
//! VT state directly and does not rasterize glyphs. The enum exists so
//! Phase B's font raster / shaper layers can add variants
//! (`NotoSansMono`, `IosevkaTerm`, `FiraCode`, …) without churning
//! every call site.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TestFont {
    /// Placeholder; resolves to whatever the renderer picks by default.
    /// Replace once real font fixtures land in Phase B.
    #[default]
    Default,
}
