// Kitty graphics image compositor.
//
// Reads only the projection matrix from the shared cell Uniforms block;
// the rest of the block is declared so the bind-group layout matches.

struct Uniforms {
    projection: mat4x4<f32>,
    cell_size: vec2<f32>,
    grid_size: vec2<u32>,
    grid_padding: vec4<f32>,
    bg_color: vec4<f32>,
    min_contrast: f32,
    cursor_visible: u32,
    cursor_pos: vec2<u32>,
    cursor_color: vec4<f32>,
    cursor_wide: u32,
    overlay_shape: u32,
    overlay_pos: vec2<u32>,
    overlay_color: vec4<f32>,
    selection_start: vec2<u32>,
    selection_end: vec2<u32>,
    selection_color: vec4<f32>,
    selection_active: u32,
    baseline: f32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var image_tex: texture_2d<f32>;
@group(1) @binding(1) var image_samp: sampler;

struct InstanceIn {
    @location(0) dest_rect: vec4<f32>,
    @location(1) source_uv: vec4<f32>,
}

struct Out {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_image(@builtin(vertex_index) vi: u32, inst: InstanceIn) -> Out {
    // Triangle-strip quad: vi=0→(0,0), 1→(1,0), 2→(0,1), 3→(1,1).
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let px = inst.dest_rect.xy + corner * inst.dest_rect.zw;
    let uv = inst.source_uv.xy + corner * inst.source_uv.zw;
    return Out(uniforms.projection * vec4<f32>(px, 0.0, 1.0), uv);
}

@fragment
fn fs_image(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image_tex, image_samp, in.uv);
    return vec4<f32>(c.rgb * c.a, c.a);
}
