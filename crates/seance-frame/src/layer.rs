#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementLayer {
    BelowBg,
    BelowText,
    AboveText,
}

impl PlacementLayer {
    pub fn contains_z(self, z: i32) -> bool {
        match self {
            PlacementLayer::BelowBg => z < i32::MIN / 2,
            PlacementLayer::BelowText => (i32::MIN / 2..0).contains(&z),
            PlacementLayer::AboveText => z >= 0,
        }
    }

    /// Sub-role this Kitty band occupies within the `Z_MAIN` layer. The two
    /// below-content bands paint before the glyph plane; `AboveText` paints
    /// after it. The bg/text split between `BelowBg` and `BelowText` is a
    /// within-`Below` ordering, not a separate sub-role.
    pub fn sub_role(self) -> SubRole {
        match self {
            PlacementLayer::BelowBg | PlacementLayer::BelowText => SubRole::Below,
            PlacementLayer::AboveText => SubRole::Above,
        }
    }
}

/// Fixed draw order within a single z-layer. Every layer records its ops in
/// this order regardless of z, so the only thing a layer's z controls is
/// where the whole `Below → Content → Above` triple sits in the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubRole {
    Below,
    Content,
    Above,
}

/// The main terminal layer: cell backgrounds, glyphs, and the three Kitty
/// graphics bands all live here as sub-roles.
pub const Z_MAIN: i32 = 0;

/// Window / pane backgrounds stack below the terminal content. Kept well
/// clear of `Z_MAIN` so future product layers (split borders, dimming) can
/// slot between without renumbering.
pub const Z_WINDOW_BG: i32 = -1000;
