//! Dynamic, `i32`-keyed layer schedule.
//!
//! Replaces a fixed draw sequence with a sorted set of layers, each created
//! on demand via [`LayerSchedule::layer_for_z`] and walked in ascending `z`.
//! Within a layer, draws run in a fixed [`SubRole`] order
//! (`Below → Content → Above`). Well-known positions are `const`, never enum
//! variants, so adding an overlay (status bar, modal, split border) is a
//! `layer_for_z(N)` call with no type edits — the renderer enumerates no
//! product feature.
//!
//! The schedule carries [`DrawOp`] tags, not GPU state; [`super::state`] walks
//! it and binds the matching pipeline. seance's draw ops are heterogeneous
//! (solid fill, SSBO `cell_bg`, instanced text, per-image quads), so a layer
//! holds a small set of op kinds, not one uniform quad buffer.

use seance_frame::PlacementLayer;

/// Stacked window backgrounds sit below the terminal grid. Reserved position
/// in the layer vocabulary; no draw targets it until window-background
/// stacking lands.
#[allow(dead_code)]
pub(crate) const Z_WINDOW_BG: i32 = -100;
/// The terminal cell content — the reference plane. Kitty bands are sub-roles
/// of this layer, not separate layers.
pub(crate) const Z_MAIN: i32 = 0;

/// Fixed within-layer order. The terminal cell content is the reference plane,
/// so a Kitty image at z=-1 still draws after `cell_bg` (Below) and before
/// text (Content) by living in the right sub-role of [`Z_MAIN`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SubRole {
    Below,
    Content,
    Above,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DrawOp {
    BgColorFill,
    CellBg,
    CellText,
    Images(PlacementLayer),
}

struct Layer {
    z: i32,
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

    fn role_mut(&mut self, role: SubRole) -> &mut Vec<DrawOp> {
        match role {
            SubRole::Below => &mut self.below,
            SubRole::Content => &mut self.content,
            SubRole::Above => &mut self.above,
        }
    }
}

#[derive(Default)]
pub(crate) struct LayerSchedule {
    /// Kept sorted by `z` ascending.
    layers: Vec<Layer>,
}

impl LayerSchedule {
    /// The layer at `z`, created (in sorted position) if absent.
    fn layer_for_z(&mut self, z: i32) -> &mut Layer {
        match self.layers.binary_search_by_key(&z, |l| l.z) {
            Ok(i) => &mut self.layers[i],
            Err(i) => {
                self.layers.insert(i, Layer::new(z));
                &mut self.layers[i]
            }
        }
    }

    pub(crate) fn push(&mut self, z: i32, role: SubRole, op: DrawOp) {
        self.layer_for_z(z).role_mut(role).push(op);
    }

    /// Draw ops in render order: ascending `z`, then `Below → Content →
    /// Above`, then insertion order within a sub-role.
    pub(crate) fn walk(&self) -> impl Iterator<Item = DrawOp> + '_ {
        self.layers.iter().flat_map(|l| {
            l.below
                .iter()
                .chain(l.content.iter())
                .chain(l.above.iter())
                .copied()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_orders_by_z_then_subrole() {
        let mut s = LayerSchedule::default();
        // Insert out of order; the schedule sorts on read.
        s.push(
            Z_MAIN,
            SubRole::Above,
            DrawOp::Images(PlacementLayer::AboveText),
        );
        s.push(Z_MAIN, SubRole::Below, DrawOp::BgColorFill);
        s.push(Z_MAIN, SubRole::Content, DrawOp::CellText);
        s.push(Z_MAIN, SubRole::Below, DrawOp::CellBg);

        let order: Vec<_> = s.walk().collect();
        assert_eq!(
            order,
            vec![
                DrawOp::BgColorFill,
                DrawOp::CellBg,
                DrawOp::CellText,
                DrawOp::Images(PlacementLayer::AboveText),
            ]
        );
    }

    #[test]
    fn floating_op_at_arbitrary_z_slots_in_sorted() {
        // M4 exit criterion: a floating draw at z=N is a `layer_for_z(N)`
        // call — no new enum variant, no match edit. A negative band sorts
        // below Z_MAIN; a positive z sorts above it.
        let mut s = LayerSchedule::default();
        s.push(Z_MAIN, SubRole::Content, DrawOp::CellText);
        s.push(100, SubRole::Content, DrawOp::CellBg); // a modal-height overlay
        s.push(Z_WINDOW_BG, SubRole::Below, DrawOp::BgColorFill);

        let order: Vec<_> = s.walk().collect();
        assert_eq!(
            order,
            vec![DrawOp::BgColorFill, DrawOp::CellText, DrawOp::CellBg]
        );
    }

    #[test]
    fn layer_for_z_is_idempotent() {
        let mut s = LayerSchedule::default();
        s.push(5, SubRole::Below, DrawOp::BgColorFill);
        s.push(5, SubRole::Below, DrawOp::CellBg);
        assert_eq!(s.layers.len(), 1);
        assert_eq!(s.walk().count(), 2);
    }
}
