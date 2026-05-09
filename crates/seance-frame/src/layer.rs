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
}
