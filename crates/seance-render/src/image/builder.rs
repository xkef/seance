//! Collects kitty placements into per-layer GPU instance lists.
//!
//! Each `FrameSource::visit_placements` call produces instances for one
//! z-layer; this module sorts them by z (so overlapping placements draw
//! back-to-front) and emits a flat list of draws with image-id stamps
//! for per-draw bind-group rebinds.

use bytemuck::{Pod, Zeroable};
use seance_vt::{PlacementLayer, PlacementSnapshot, PlacementVisitor};

use crate::text::FrameInfo;

/// GPU-facing per-placement quad instance. 32 bytes, `Pod`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub(crate) struct ImageInstance {
    pub dest_rect: [f32; 4],
    pub source_uv: [f32; 4],
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ImageDraw {
    pub image_id: u32,
    pub instance: u32,
}

#[derive(Default)]
pub(crate) struct ImageFrame {
    /// All placement instances from all three layers, concatenated.
    pub(crate) instances: Vec<ImageInstance>,

    /// Scratch buckets populated during `visit_placements`; merged into
    /// `instances` during `finalize`.
    below_bg_scratch: Vec<(i32, ImageInstance, u32)>,
    below_text_scratch: Vec<(i32, ImageInstance, u32)>,
    above_text_scratch: Vec<(i32, ImageInstance, u32)>,

    /// Resolved draw commands after finalize, partitioned by layer.
    below_bg_draws: Vec<ImageDraw>,
    below_text_draws: Vec<ImageDraw>,
    above_text_draws: Vec<ImageDraw>,
}

impl ImageFrame {
    pub(crate) fn clear(&mut self) {
        self.instances.clear();
        self.below_bg_scratch.clear();
        self.below_text_scratch.clear();
        self.above_text_scratch.clear();
        self.below_bg_draws.clear();
        self.below_text_draws.clear();
        self.above_text_draws.clear();
    }

    fn scratch_mut(&mut self, layer: PlacementLayer) -> &mut Vec<(i32, ImageInstance, u32)> {
        match layer {
            PlacementLayer::BelowBg => &mut self.below_bg_scratch,
            PlacementLayer::BelowText => &mut self.below_text_scratch,
            PlacementLayer::AboveText => &mut self.above_text_scratch,
        }
    }

    pub(crate) fn draws_for(&self, layer: PlacementLayer) -> &[ImageDraw] {
        match layer {
            PlacementLayer::BelowBg => &self.below_bg_draws,
            PlacementLayer::BelowText => &self.below_text_draws,
            PlacementLayer::AboveText => &self.above_text_draws,
        }
    }

    /// Sort each scratch bucket by z, then append to `instances` and
    /// record draw ranges. After this, scratch buckets are drained.
    pub(crate) fn finalize(&mut self) {
        drain_bucket(
            &mut self.below_bg_scratch,
            &mut self.instances,
            &mut self.below_bg_draws,
        );
        drain_bucket(
            &mut self.below_text_scratch,
            &mut self.instances,
            &mut self.below_text_draws,
        );
        drain_bucket(
            &mut self.above_text_scratch,
            &mut self.instances,
            &mut self.above_text_draws,
        );
    }
}

fn drain_bucket(
    scratch: &mut Vec<(i32, ImageInstance, u32)>,
    instances: &mut Vec<ImageInstance>,
    draws: &mut Vec<ImageDraw>,
) {
    scratch.sort_by_key(|(z, _, _)| *z);
    for (_z, instance, image_id) in scratch.drain(..) {
        let idx = instances.len() as u32;
        instances.push(instance);
        draws.push(ImageDraw {
            image_id,
            instance: idx,
        });
    }
}

/// Bridges `PlacementVisitor` into the scratch bucket for one layer.
pub(crate) struct PlacementCollector<'a> {
    pub(crate) frame: &'a mut ImageFrame,
    pub(crate) layer: PlacementLayer,
    pub(crate) fi: &'a FrameInfo,
}

impl PlacementVisitor for PlacementCollector<'_> {
    fn placement(&mut self, p: &PlacementSnapshot) {
        if p.pixel_width == 0 || p.pixel_height == 0 {
            return;
        }
        if p.image_width == 0 || p.image_height == 0 {
            return;
        }
        // Destination rect in viewport pixels. `viewport_col/row` may be
        // negative for partially scrolled placements; the rasterizer
        // clips naturally, so no CPU-side clamping is needed.
        let dest_x = self.fi.grid_padding[0] + p.viewport_col as f32 * self.fi.cell_width;
        let dest_y = self.fi.grid_padding[1] + p.viewport_row as f32 * self.fi.cell_height;
        let dest_rect = [dest_x, dest_y, p.pixel_width as f32, p.pixel_height as f32];

        let iw = p.image_width as f32;
        let ih = p.image_height as f32;
        let source_uv = [
            p.source_x as f32 / iw,
            p.source_y as f32 / ih,
            p.source_width as f32 / iw,
            p.source_height as f32 / ih,
        ];

        self.frame.scratch_mut(self.layer).push((
            p.z,
            ImageInstance {
                dest_rect,
                source_uv,
            },
            p.image_id,
        ));
    }
}
