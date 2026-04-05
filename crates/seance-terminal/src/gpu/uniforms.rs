//! GPU uniform buffer types matching the WGSL shader layout.

use ghostty_renderer::FrameData;

use crate::renderer::Overlay;

/// Cursor shape for the overlay cursor (copy mode, etc.).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// No overlay cursor.
    #[default]
    Hidden = 0,
    /// Solid block (like vim normal mode).
    Block = 1,
    /// Underline (like vim replace mode).
    Underline = 2,
    /// Vertical bar (like vim insert mode).
    Bar = 3,
}

/// Uniform buffer sent to all shader passes.
///
/// Layout must match the `Uniforms` struct in `cell.wgsl` exactly.
/// WGSL alignment rules: vec2 aligns to 8, vec4/mat4x4 align to 16.
const _: () = assert!(size_of::<Uniforms>() == 256);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    // --- block 0: projection (64 bytes) ---
    pub projection: [[f32; 4]; 4],     // offset 0

    // --- block 1: grid layout (32 bytes) ---
    pub cell_size: [f32; 2],           // offset 64
    pub grid_size: [u32; 2],           // offset 72
    pub grid_padding: [f32; 4],        // offset 80  (align 16)

    // --- block 2: colors + contrast (32 bytes) ---
    pub bg_color: [f32; 4],            // offset 96  (align 16)
    pub min_contrast: f32,             // offset 112
    pub cursor_visible: u32,           // offset 116  (0 = hidden, 1 = visible)
    pub cursor_pos: [u32; 2],          // offset 120  (align 8)

    // --- block 3: VT cursor color (16 bytes) ---
    pub cursor_color: [f32; 4],        // offset 128  (align 16)

    // --- block 4: VT cursor + overlay cursor (16 bytes) ---
    pub cursor_wide: u32,              // offset 144
    pub overlay_shape: u32,            // offset 148  (CursorShape as u32)
    pub overlay_pos: [u32; 2],         // offset 152  (align 8)

    // --- block 5: overlay cursor color (16 bytes) ---
    pub overlay_color: [f32; 4],       // offset 160  (align 16)

    // --- block 6: selection range (16 bytes) ---
    pub selection_start: [u32; 2],     // offset 176  (col, row)
    pub selection_end: [u32; 2],       // offset 184  (col, row)

    // --- block 7: selection color (16 bytes) ---
    pub selection_color: [f32; 4],     // offset 192  (align 16)

    // --- block 8: selection active flag + padding (48 bytes to 256) ---
    pub selection_active: u32,         // offset 208
    pub _pad: [u32; 11],              // offset 212  (pad to 256, align 16)
}

impl Uniforms {
    /// Build uniforms from a ghostty `FrameData` snapshot, the current
    /// surface size, and the overlay state.
    pub fn from_frame_data(
        fd: &FrameData,
        surface_width: f32,
        surface_height: f32,
        overlay: &Overlay,
    ) -> Self {
        let (sel_start, sel_end, sel_active) = match &overlay.selection {
            Some((start, end)) => ([start.col as u32, start.row as u32],
                                   [end.col as u32, end.row as u32], 1u32),
            None => ([0u32; 2], [0u32; 2], 0u32),
        };

        Self {
            projection: Self::ortho(surface_width, surface_height),
            cell_size: [fd.cell_width, fd.cell_height],
            grid_size: [fd.grid_cols as u32, fd.grid_rows as u32],
            grid_padding: fd.grid_padding,
            bg_color: u8x4_to_f32(fd.bg_color),
            min_contrast: fd.min_contrast,
            cursor_visible: if overlay.vt_cursor_visible { 1 } else { 0 },
            cursor_pos: [fd.cursor_pos[0] as u32, fd.cursor_pos[1] as u32],
            cursor_color: u8x4_to_f32(fd.cursor_color),
            cursor_wide: if fd.cursor_wide { 1 } else { 0 },
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

    /// Orthographic projection: (0,0) top-left, (w,h) bottom-right, Z 0..1.
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
