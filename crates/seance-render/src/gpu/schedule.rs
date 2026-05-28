//! Dynamic z-keyed draw schedule.
//!
//! Replaces the renderer's fixed pass sequence with a `Vec<Layer>` kept
//! sorted by an open `i32` z, mirroring WezTerm's `layer_for_zindex`. Layers
//! are created on demand via [`LayerSchedule::layer_for_z`]; well-known
//! positions are `const` z values ([`seance_frame::Z_MAIN`], etc.), never
//! enum variants, so a new overlay is one `layer_for_z(N)` call with no type
//! edits. Within a layer the ops record in fixed
//! [`SubRole`](seance_frame::SubRole) order.
//!
//! seance's draw ops are heterogeneous (solid fill, SSBO cell-bg, instanced
//! glyphs, per-image Kitty quads), so a layer holds an ordered list of
//! [`DrawOp`] kinds per sub-role rather than a single uniform quad buffer;
//! the schedule decides *when* each op records, not *what* it draws.

use seance_frame::{PlacementLayer, SubRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrawOp {
    BgColorFill,
    CellBg,
    CellText,
    KittyBand(PlacementLayer),
}

pub(super) struct Layer {
    pub(super) z: i32,
    below: Vec<DrawOp>,
    content: Vec<DrawOp>,
    above: Vec<DrawOp>,
}

impl Layer {
    fn new(z: i32) -> Self {
        Self {
            z,
            below: Vec::new(),
            content: Vec::new(),
            above: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, role: SubRole, op: DrawOp) {
        match role {
            SubRole::Below => self.below.push(op),
            SubRole::Content => self.content.push(op),
            SubRole::Above => self.above.push(op),
        }
    }

    /// Ops in fixed `Below → Content → Above` order. Within a sub-role,
    /// ops keep their push order.
    pub(super) fn ops(&self) -> impl Iterator<Item = DrawOp> + '_ {
        self.below
            .iter()
            .chain(&self.content)
            .chain(&self.above)
            .copied()
    }
}

#[derive(Default)]
pub(super) struct LayerSchedule {
    layers: Vec<Layer>,
}

impl LayerSchedule {
    pub(super) fn clear(&mut self) {
        self.layers.clear();
    }

    /// Layer at `z`, created and inserted in sorted position on miss.
    pub(super) fn layer_for_z(&mut self, z: i32) -> &mut Layer {
        match self.layers.binary_search_by_key(&z, |l| l.z) {
            Ok(i) => &mut self.layers[i],
            Err(i) => {
                self.layers.insert(i, Layer::new(z));
                &mut self.layers[i]
            }
        }
    }

    pub(super) fn layers(&self) -> &[Layer] {
        &self.layers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_frame::{Z_MAIN, Z_WINDOW_BG};

    #[test]
    fn layers_kept_sorted_regardless_of_insert_order() {
        let mut s = LayerSchedule::default();
        s.layer_for_z(100).push(SubRole::Content, DrawOp::CellText);
        s.layer_for_z(Z_MAIN).push(SubRole::Below, DrawOp::CellBg);
        s.layer_for_z(Z_WINDOW_BG)
            .push(SubRole::Below, DrawOp::BgColorFill);

        let zs: Vec<i32> = s.layers().iter().map(|l| l.z).collect();
        assert_eq!(zs, vec![Z_WINDOW_BG, Z_MAIN, 100]);
    }

    #[test]
    fn repeated_layer_for_z_returns_same_layer() {
        let mut s = LayerSchedule::default();
        s.layer_for_z(Z_MAIN).push(SubRole::Below, DrawOp::CellBg);
        s.layer_for_z(Z_MAIN)
            .push(SubRole::Content, DrawOp::CellText);
        assert_eq!(s.layers().len(), 1);
    }

    #[test]
    fn ops_walk_below_then_content_then_above() {
        let mut s = LayerSchedule::default();
        let main = s.layer_for_z(Z_MAIN);
        // Push out of sub-role order to prove the walk re-orders by role
        // while preserving within-role push order.
        main.push(SubRole::Above, DrawOp::KittyBand(PlacementLayer::AboveText));
        main.push(SubRole::Content, DrawOp::CellText);
        main.push(SubRole::Below, DrawOp::BgColorFill);
        main.push(SubRole::Below, DrawOp::KittyBand(PlacementLayer::BelowBg));

        assert_eq!(
            main.ops().collect::<Vec<_>>(),
            vec![
                DrawOp::BgColorFill,
                DrawOp::KittyBand(PlacementLayer::BelowBg),
                DrawOp::CellText,
                DrawOp::KittyBand(PlacementLayer::AboveText),
            ]
        );
    }

    // Epic exit criterion #2: a floating op at a brand-new z is a single
    // `layer_for_z(N)` call — no enum/match/type edits anywhere.
    #[test]
    fn op_at_new_z_needs_only_a_layer_for_z_call() {
        let mut s = LayerSchedule::default();
        s.layer_for_z(Z_MAIN)
            .push(SubRole::Content, DrawOp::CellText);
        s.layer_for_z(50).push(SubRole::Content, DrawOp::CellBg);

        let zs: Vec<i32> = s.layers().iter().map(|l| l.z).collect();
        assert_eq!(zs, vec![Z_MAIN, 50]);
    }

    #[test]
    fn clear_drops_all_layers() {
        let mut s = LayerSchedule::default();
        s.layer_for_z(Z_MAIN).push(SubRole::Below, DrawOp::CellBg);
        s.clear();
        assert!(s.layers().is_empty());
    }
}
