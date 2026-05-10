use crate::renderer::RenderInputs;
use crate::text::FrameInfo;
use seance_config::{CursorStyle, Theme};
use seance_protocol::CursorShape as ProtocolCursorShape;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Hidden = 0,
    Block = 1,
    Underline = 2,
    Bar = 3,
}

impl From<CursorStyle> for CursorShape {
    fn from(s: CursorStyle) -> Self {
        match s {
            CursorStyle::Block => Self::Block,
            CursorStyle::Bar => Self::Bar,
            CursorStyle::Underline => Self::Underline,
        }
    }
}

impl From<ProtocolCursorShape> for CursorShape {
    fn from(s: ProtocolCursorShape) -> Self {
        match s {
            ProtocolCursorShape::Block => Self::Block,
            ProtocolCursorShape::Bar => Self::Bar,
            ProtocolCursorShape::Underline => Self::Underline,
        }
    }
}

/// Layout must match the `Uniforms` struct in `cell.wgsl` exactly.
const _: () = assert!(size_of::<Uniforms>() == 256);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub projection: [[f32; 4]; 4],
    pub cell_size: [f32; 2],
    pub grid_size: [u32; 2],
    pub grid_padding: [f32; 4],
    pub bg_color: [f32; 4],
    pub min_contrast: f32,
    pub cursor_visible: u32,
    pub cursor_pos: [u32; 2],
    pub cursor_color: [f32; 4],
    pub cursor_wide: u32,
    pub overlay_shape: u32,
    pub overlay_pos: [u32; 2],
    pub overlay_color: [f32; 4],
    pub selection_start: [u32; 2],
    pub selection_end: [u32; 2],
    pub selection_color: [f32; 4],
    pub selection_active: u32,
    pub baseline: f32,
    pub hovered_link_active: u32,
    /// Padding to satisfy WGSL `vec2<u32>` 8-byte alignment for the
    /// hovered_link range below.
    pub _pad_link: u32,
    pub hovered_link_start: [u32; 2],
    pub hovered_link_end: [u32; 2],
    pub hovered_link_color: [f32; 4],
}

impl Uniforms {
    pub fn from_frame_info(
        fi: &FrameInfo,
        surface_width: f32,
        surface_height: f32,
        inputs: &RenderInputs,
        theme: &Theme,
    ) -> Self {
        let (sel_start, sel_end, sel_active) = match &inputs.selection {
            Some((start, end)) => (
                [start.col as u32, start.row as u32],
                [end.col as u32, end.row as u32],
                1u32,
            ),
            None => ([0u32; 2], [0u32; 2], 0u32),
        };

        let (link_start, link_end, link_active) = match &inputs.hovered_link {
            Some(range) if range.start.row <= range.end.row => (
                [u32::from(range.start.col), u32::from(range.start.row)],
                [u32::from(range.end.col), u32::from(range.end.row)],
                1u32,
            ),
            _ => ([0u32; 2], [0u32; 2], 0u32),
        };

        Self {
            projection: Self::ortho(surface_width, surface_height),
            cell_size: [fi.cell_width, fi.cell_height],
            grid_size: [fi.grid_cols as u32, fi.grid_rows as u32],
            grid_padding: fi.grid_padding,
            bg_color: u8x4_to_f32(fi.bg_color),
            min_contrast: fi.min_contrast,
            cursor_visible: if inputs.vt_cursor_visible && fi.cursor_visible {
                1
            } else {
                0
            },
            cursor_pos: [fi.cursor_pos[0] as u32, fi.cursor_pos[1] as u32],
            cursor_color: u8x4_to_f32(fi.cursor_color),
            cursor_wide: if fi.cursor_wide { 1 } else { 0 },
            overlay_shape: inputs.cursor_shape as u32,
            overlay_pos: [fi.cursor_pos[0] as u32, fi.cursor_pos[1] as u32],
            overlay_color: u8x4_to_f32(theme.cursor),
            selection_start: sel_start,
            selection_end: sel_end,
            selection_color: theme.selection_bg,
            selection_active: sel_active,
            baseline: fi.baseline,
            hovered_link_active: link_active,
            _pad_link: 0,
            hovered_link_start: link_start,
            hovered_link_end: link_end,
            hovered_link_color: [
                f32::from(theme.fg[0]) / 255.0,
                f32::from(theme.fg[1]) / 255.0,
                f32::from(theme.fg[2]) / 255.0,
                1.0,
            ],
        }
    }

    pub fn ortho(width: f32, height: f32) -> [[f32; 4]; 4] {
        [
            [2.0 / width, 0.0, 0.0, 0.0],
            [0.0, -2.0 / height, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 1.0],
        ]
    }
}

fn u8x4_to_f32(c: [u8; 4]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_style_maps_to_overlay_shape() {
        assert_eq!(CursorShape::from(CursorStyle::Block) as u32, 1);
        assert_eq!(CursorShape::from(CursorStyle::Underline) as u32, 2);
        assert_eq!(CursorShape::from(CursorStyle::Bar) as u32, 3);
    }

    #[test]
    fn vt_cursor_shape_maps_to_overlay_shape() {
        assert_eq!(CursorShape::from(ProtocolCursorShape::Block) as u32, 1);
        assert_eq!(CursorShape::from(ProtocolCursorShape::Underline) as u32, 2);
        assert_eq!(CursorShape::from(ProtocolCursorShape::Bar) as u32, 3);
    }

    #[test]
    fn hovered_link_range_maps_to_uniforms() {
        let fi = FrameInfo {
            cell_width: 8.0,
            cell_height: 16.0,
            baseline: 12.0,
            grid_cols: 10,
            grid_rows: 4,
            grid_padding: [0.0; 4],
            bg_color: [0, 0, 0, 255],
            min_contrast: 1.0,
            cursor_pos: [0, 0],
            cursor_visible: true,
            cursor_color: [255, 255, 255, 255],
            cursor_wide: false,
        };
        let inputs = RenderInputs {
            hovered_link: Some(crate::renderer::HoveredLinkRange {
                start: seance_protocol::GridPos { col: 2, row: 1 },
                end: seance_protocol::GridPos { col: 4, row: 2 },
            }),
            ..RenderInputs::default()
        };
        let uniforms = Uniforms::from_frame_info(&fi, 80.0, 64.0, &inputs, &Theme::blank());

        assert_eq!(uniforms.hovered_link_active, 1);
        assert_eq!(uniforms.hovered_link_start, [2, 1]);
        assert_eq!(uniforms.hovered_link_end, [4, 2]);
    }
}
