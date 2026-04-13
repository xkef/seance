use crate::font::cell_builder::FrameInfo;
use crate::renderer::Overlay;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Hidden = 0,
    Block = 1,
    Underline = 2,
    Bar = 3,
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
    pub _pad: [u32; 11],
}

impl Uniforms {
    pub fn from_frame_info(
        fi: &FrameInfo,
        surface_width: f32,
        surface_height: f32,
        overlay: &Overlay,
    ) -> Self {
        let (sel_start, sel_end, sel_active) = match &overlay.selection {
            Some((start, end)) => (
                [start.col as u32, start.row as u32],
                [end.col as u32, end.row as u32],
                1u32,
            ),
            None => ([0u32; 2], [0u32; 2], 0u32),
        };

        Self {
            projection: Self::ortho(surface_width, surface_height),
            cell_size: [fi.cell_width, fi.cell_height],
            grid_size: [fi.grid_cols as u32, fi.grid_rows as u32],
            grid_padding: fi.grid_padding,
            bg_color: u8x4_to_f32(fi.bg_color),
            min_contrast: fi.min_contrast,
            cursor_visible: if overlay.vt_cursor_visible { 1 } else { 0 },
            cursor_pos: [fi.cursor_pos[0] as u32, fi.cursor_pos[1] as u32],
            cursor_color: u8x4_to_f32(fi.cursor_color),
            cursor_wide: if fi.cursor_wide { 1 } else { 0 },
            overlay_shape: overlay.cursor_shape as u32,
            overlay_pos: [overlay.cursor_pos.col as u32, overlay.cursor_pos.row as u32],
            overlay_color: overlay.cursor_color,
            selection_start: sel_start,
            selection_end: sel_end,
            selection_color: overlay.selection_color,
            selection_active: sel_active,
            _pad: [0; 11],
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
