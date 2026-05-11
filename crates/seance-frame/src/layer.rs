/// Z-ordered partition the renderer asks the [`crate::FrameSource`] to
/// emit placements for. The `z` value of a
/// [`seance_protocol::frame::PlacementSnapshot`] determines which layer
/// it falls into:
///
/// - [`Self::BelowBg`] — `z < i32::MIN / 2`: drawn before the cell
///   background pass.
/// - [`Self::BelowText`] — `i32::MIN / 2 ..= -1`: between background and
///   glyphs.
/// - [`Self::AboveText`] — `z >= 0`: drawn over glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementLayer {
    /// Below the cell background pass.
    BelowBg,
    /// Between background and text.
    BelowText,
    /// Above text.
    AboveText,
}

impl PlacementLayer {
    /// Whether a placement with `z` belongs in this layer.
    pub fn contains_z(self, z: i32) -> bool {
        match self {
            PlacementLayer::BelowBg => z < i32::MIN / 2,
            PlacementLayer::BelowText => (i32::MIN / 2..0).contains(&z),
            PlacementLayer::AboveText => z >= 0,
        }
    }
}
