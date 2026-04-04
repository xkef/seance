//! GPU uniform buffer types matching the WGSL shader layout.

/// Uniform buffer sent to all shader passes.
///
/// Layout must match the `Uniforms` struct in `cell.wgsl` exactly.
/// WGSL alignment rules: vec2 aligns to 8, vec4/mat4x4 align to 16.
/// Padding fields keep the Rust repr(C) layout in sync.
const _: () = assert!(size_of::<Uniforms>() == 160);

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
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
    /// Build an orthographic projection matrix for the given screen size.
    ///
    /// Maps (0,0) at top-left to (width, height) at bottom-right,
    /// with Z from 0 to 1. This matches the coordinate system used
    /// by the cell shaders.
    pub fn ortho(width: f32, height: f32) -> [[f32; 4]; 4] {
        [
            [2.0 / width, 0.0, 0.0, 0.0],
            [0.0, -2.0 / height, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 1.0],
        ]
    }
}
