//! GPU uniform buffer types matching the WGSL shader layout.

use ghostty_renderer::FrameData;

/// Uniform buffer sent to all shader passes.
///
/// Layout must match the `Uniforms` struct in `cell.wgsl` exactly.
/// WGSL alignment rules: vec2 aligns to 8, vec4/mat4x4 align to 16.
/// Padding fields keep the Rust repr(C) layout in sync.
const _: () = assert!(size_of::<Uniforms>() == 160);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub projection: [[f32; 4]; 4], // offset 0,   64 bytes
    pub cell_size: [f32; 2],       // offset 64,  8 bytes
    pub grid_size: [u32; 2],       // offset 72,  8 bytes
    pub grid_padding: [f32; 4],    // offset 80,  16 bytes (align 16)
    pub bg_color: [f32; 4],        // offset 96,  16 bytes (align 16)
    pub min_contrast: f32,         // offset 112, 4 bytes
    pub _pad0: u32,                // offset 116, 4 bytes — aligns cursor_pos to 8
    pub cursor_pos: [u32; 2],      // offset 120, 8 bytes (WGSL vec2<u32> align 8)
    pub cursor_color: [f32; 4],    // offset 128, 16 bytes (align 16)
    pub cursor_wide: u32,          // offset 144, 4 bytes
    pub _pad1: [u32; 3],           // offset 148, 12 bytes — struct size 160 (align 16)
}

impl Uniforms {
    /// Build uniforms from a ghostty `FrameData` snapshot and the current
    /// surface size. This is the single source of truth for uniform
    /// construction — both full and bg-only render paths use it.
    pub fn from_frame_data(fd: &FrameData, surface_width: f32, surface_height: f32) -> Self {
        Self {
            projection: Self::ortho(surface_width, surface_height),
            cell_size: [fd.cell_width, fd.cell_height],
            grid_size: [fd.grid_cols as u32, fd.grid_rows as u32],
            grid_padding: fd.grid_padding,
            bg_color: u8x4_to_f32(fd.bg_color),
            min_contrast: fd.min_contrast,
            _pad0: 0,
            cursor_pos: [fd.cursor_pos[0] as u32, fd.cursor_pos[1] as u32],
            cursor_color: u8x4_to_f32(fd.cursor_color),
            cursor_wide: if fd.cursor_wide { 1 } else { 0 },
            _pad1: [0; 3],
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
