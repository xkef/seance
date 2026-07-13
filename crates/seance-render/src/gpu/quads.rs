//! Floating quad emitter: filled rectangles and rounded rectangles.
//!
//! A single instanced pipeline draws every quad; the rounded corner is an
//! analytic signed-distance field in the fragment shader, so no mask texture
//! is needed and any corner radius is exact. Quads are keyed by the same
//! `i32` z used by [`super::layers`], so a caller places a split border, a
//! modal background, a visual-bell flash, or a selection-rect fallback by
//! picking a z — the schedule sorts them among the terminal layers with no
//! new pipeline.
//!
//! Emission is retained: [`QuadBatch`] holds whatever was emitted until the
//! caller clears it, so a static overlay is emitted once rather than every
//! frame.

use std::collections::BTreeMap;

/// A rectangle in physical (framebuffer) pixels, top-left origin, y-down —
/// the same space the fragment shaders see in `@builtin(position)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl PixelRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// One quad, laid out to match the vertex-instance attributes declared in
/// [`super::pipeline`] and the `VsIn` bindings in `shaders/quads.wgsl`.
/// `color` is straight-alpha RGBA in the surface's (sRGB) space; the shader
/// premultiplies before the `One / OneMinusSrcAlpha` blend.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct QuadInstance {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub corner_radius: f32,
}

/// One instanced draw: a contiguous run of [`QuadInstance`]s that share a z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QuadDraw {
    pub z: i32,
    pub start: u32,
    pub count: u32,
}

/// Accumulates emitted quads, bucketed by z so each z becomes one draw.
#[derive(Clone, Debug, Default)]
pub(crate) struct QuadBatch {
    by_layer: BTreeMap<i32, Vec<QuadInstance>>,
}

impl QuadBatch {
    /// Emit a filled (optionally rounded) rectangle at layer `z`. The radius
    /// is clamped to `[0, min(width, height) / 2]`; degenerate rectangles
    /// (non-positive extent) are dropped.
    pub(crate) fn emit_rect(
        &mut self,
        z: i32,
        rect: PixelRect,
        color: [f32; 4],
        corner_radius: f32,
    ) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let max_radius = 0.5 * rect.width.min(rect.height);
        let corner_radius = corner_radius.clamp(0.0, max_radius);
        self.by_layer.entry(z).or_default().push(QuadInstance {
            rect: [rect.x, rect.y, rect.width, rect.height],
            color,
            corner_radius,
        });
    }

    pub(crate) fn clear(&mut self) {
        self.by_layer.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_layer.is_empty()
    }

    /// Flatten into one instance buffer plus the per-z draw ranges, in
    /// ascending z. The instance order matches the draw ranges, so a draw's
    /// `start..start + count` slices its own quads out of the returned vec.
    pub(crate) fn flatten(&self) -> (Vec<QuadInstance>, Vec<QuadDraw>) {
        let mut instances = Vec::new();
        let mut draws = Vec::new();
        for (&z, layer) in &self.by_layer {
            if layer.is_empty() {
                continue;
            }
            let start = instances.len() as u32;
            instances.extend_from_slice(layer);
            draws.push(QuadDraw {
                z,
                start,
                count: layer.len() as u32,
            });
        }
        (instances, draws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CPU mirror of `sd_rounded_box` in `shaders/quads.wgsl`. Kept in lockstep
    /// so the coverage assertions below describe exactly what the GPU draws.
    fn sd_rounded_box(px: f32, py: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
        let qx = px.abs() - half_w + r;
        let qy = py.abs() - half_h + r;
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        qx.max(qy).min(0.0) + outside - r
    }

    #[test]
    fn emit_rect_clamps_radius_to_half_min_extent() {
        let mut batch = QuadBatch::default();
        batch.emit_rect(0, PixelRect::new(0.0, 0.0, 10.0, 4.0), [1.0; 4], 100.0);
        let (instances, _) = batch.flatten();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].corner_radius, 2.0);
    }

    #[test]
    fn emit_rect_drops_degenerate_rects() {
        let mut batch = QuadBatch::default();
        batch.emit_rect(0, PixelRect::new(0.0, 0.0, 0.0, 10.0), [1.0; 4], 0.0);
        batch.emit_rect(0, PixelRect::new(0.0, 0.0, 10.0, -1.0), [1.0; 4], 0.0);
        assert!(batch.is_empty());
    }

    #[test]
    fn flatten_orders_by_layer_with_contiguous_ranges() {
        let mut batch = QuadBatch::default();
        // Emit out of z order; flatten must sort ascending and pack ranges.
        batch.emit_rect(10, PixelRect::new(0.0, 0.0, 4.0, 4.0), [1.0; 4], 0.0);
        batch.emit_rect(-5, PixelRect::new(0.0, 0.0, 4.0, 4.0), [1.0; 4], 0.0);
        batch.emit_rect(10, PixelRect::new(4.0, 0.0, 4.0, 4.0), [1.0; 4], 0.0);

        let (instances, draws) = batch.flatten();
        assert_eq!(instances.len(), 3);
        assert_eq!(
            draws,
            vec![
                QuadDraw {
                    z: -5,
                    start: 0,
                    count: 1
                },
                QuadDraw {
                    z: 10,
                    start: 1,
                    count: 2
                },
            ]
        );
    }

    #[test]
    fn clear_resets_the_batch() {
        let mut batch = QuadBatch::default();
        batch.emit_rect(0, PixelRect::new(0.0, 0.0, 4.0, 4.0), [1.0; 4], 0.0);
        assert!(!batch.is_empty());
        batch.clear();
        assert!(batch.is_empty());
        assert!(batch.flatten().0.is_empty());
    }

    #[test]
    fn quad_instance_is_tightly_packed() {
        // Vertex stride in `pipeline.rs` (36) and the attribute offsets
        // (rect@0, color@16, corner_radius@32) assume this exact layout.
        assert_eq!(size_of::<QuadInstance>(), 36);
        assert_eq!(std::mem::align_of::<QuadInstance>(), 4);
    }

    #[test]
    fn sdf_is_negative_inside_positive_outside() {
        // A 20×20 square centred at the origin, half-extent 10, radius 4.
        let inside = sd_rounded_box(0.0, 0.0, 10.0, 10.0, 4.0);
        assert!(inside < 0.0, "centre must be inside: {inside}");

        // A point just past the flat right edge is outside.
        let past_edge = sd_rounded_box(11.0, 0.0, 10.0, 10.0, 4.0);
        assert!(past_edge > 0.0, "past edge must be outside: {past_edge}");

        // The exact corner (10,10) sits outside a rounded box: with radius 4
        // the arc centre is at (6,6), so the corner is r*(√2−1) ≈ 1.657 out.
        let corner = sd_rounded_box(10.0, 10.0, 10.0, 10.0, 4.0);
        assert!(corner > 0.0, "square corner must be clipped: {corner}");
        assert!((corner - (4.0 * (2.0_f32.sqrt() - 1.0))).abs() < 1e-4);
    }

    #[test]
    fn zero_radius_matches_a_plain_box_edge() {
        // With r = 0 the SDF degenerates to a hard-edged box: distance 0 on
        // the edge, negative inside.
        let on_edge = sd_rounded_box(10.0, 0.0, 10.0, 10.0, 0.0);
        assert!((on_edge - 0.0).abs() < 1e-6, "edge distance should be 0");
        let inside = sd_rounded_box(9.0, 0.0, 10.0, 10.0, 0.0);
        assert!((inside - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn wgsl_shader_parses_and_validates() {
        let src = include_str!("shaders/quads.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("quads.wgsl should parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        );
        validator
            .validate(&module)
            .expect("quads.wgsl should validate");
    }
}
