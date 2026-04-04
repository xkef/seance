// Cell rendering shaders for seance.
// Ported from ghostty's Metal shaders (shaders.metal).

struct Uniforms {
    projection: mat4x4<f32>,
    cell_size: vec2<f32>,
    grid_size: vec2<u32>,
    grid_padding: vec4<f32>, // left, top, right, bottom
    bg_color: vec4<f32>,     // premultiplied linear RGBA
    min_contrast: f32,
    cursor_pos: vec2<u32>,
    cursor_color: vec4<f32>,
    cursor_wide: u32,        // bool as u32
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// ================================================================
// Full-screen vertex (used by bg_color and cell_bg passes)
// ================================================================

struct FullScreenOut {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> FullScreenOut {
    var out: FullScreenOut;
    // Single triangle covering the viewport:
    //   vid 0: (-1, -3)
    //   vid 1: (-1,  1)
    //   vid 2: ( 3,  1)
    let x = select(-1.0, 3.0, vid == 2u);
    let y = select(1.0, -3.0, vid == 0u);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// ================================================================
// Background color pass: fills entire surface with bg color
// ================================================================

@fragment
fn fs_bg_color(in: FullScreenOut) -> @location(0) vec4<f32> {
    return uniforms.bg_color;
}

// ================================================================
// Cell background pass: per-cell background colors
// ================================================================

@group(1) @binding(0) var<storage, read> bg_cells: array<u32>;

fn unpack_rgba(packed: u32) -> vec4<f32> {
    let r = f32(packed & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let b = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

@fragment
fn fs_cell_bg(in: FullScreenOut) -> @location(0) vec4<f32> {
    let pos = in.position.xy - uniforms.grid_padding.xy;
    let grid_pos = vec2<i32>(floor(pos / uniforms.cell_size));

    if grid_pos.x < 0 || grid_pos.y < 0 {
        return vec4<f32>(0.0);
    }
    if u32(grid_pos.x) >= uniforms.grid_size.x || u32(grid_pos.y) >= uniforms.grid_size.y {
        return vec4<f32>(0.0);
    }

    let idx = u32(grid_pos.y) * uniforms.grid_size.x + u32(grid_pos.x);
    let packed = bg_cells[idx];
    var color = unpack_rgba(packed);

    // Premultiply alpha
    color = vec4<f32>(color.rgb * color.a, color.a);
    return color;
}

// ================================================================
// Cell text pass: instanced glyph quads
// ================================================================

struct CellTextInstance {
    @location(0) glyph_pos: vec2<u32>,
    @location(1) glyph_size: vec2<u32>,
    @location(2) bearings: vec2<i32>,
    @location(3) grid_pos: vec2<u32>,
    @location(4) color: vec4<f32>,
    @location(5) atlas_and_flags: u32,
}

struct CellTextOut {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) atlas: u32,
}

@group(2) @binding(0) var atlas_grayscale: texture_2d<f32>;
@group(2) @binding(1) var atlas_color_tex: texture_2d<f32>;
@group(2) @binding(2) var atlas_sampler: sampler;

@vertex
fn vs_cell_text(
    @builtin(vertex_index) vid: u32,
    instance: CellTextInstance,
) -> CellTextOut {
    let corner = vec2<f32>(
        select(0.0, 1.0, vid == 1u || vid == 3u),
        select(0.0, 1.0, vid == 2u || vid == 3u),
    );

    let cell_pos = uniforms.cell_size * vec2<f32>(f32(instance.grid_pos.x), f32(instance.grid_pos.y));
    let size = vec2<f32>(f32(instance.glyph_size.x), f32(instance.glyph_size.y));
    var offset = vec2<f32>(f32(instance.bearings.x), f32(instance.bearings.y));
    offset.y = uniforms.cell_size.y - offset.y;

    let world_pos = cell_pos + size * corner + offset + uniforms.grid_padding.xy;

    var out: CellTextOut;
    out.position = uniforms.projection * vec4<f32>(world_pos, 0.0, 1.0);
    out.tex_coord = vec2<f32>(f32(instance.glyph_pos.x), f32(instance.glyph_pos.y))
                  + vec2<f32>(f32(instance.glyph_size.x), f32(instance.glyph_size.y)) * corner;
    out.color = instance.color;
    out.atlas = instance.atlas_and_flags & 0xFFu;

    return out;
}

@fragment
fn fs_cell_text(in: CellTextOut) -> @location(0) vec4<f32> {
    if in.atlas == 0u {
        // Grayscale atlas: text glyphs.
        // The atlas stores coverage alpha. The color comes from
        // the vertex (set by ghostty's cell rebuilder).
        let gs_size = vec2<f32>(textureDimensions(atlas_grayscale));
        let uv = in.tex_coord / gs_size;
        let a = textureSample(atlas_grayscale, atlas_sampler, uv).r;
        if a < 0.01 {
            discard;
        }
        let alpha = a * in.color.a;
        return vec4<f32>(in.color.rgb * alpha, alpha);
    } else {
        // Color atlas: emoji (already premultiplied BGRA).
        let c_size = vec2<f32>(textureDimensions(atlas_color_tex));
        let uv = in.tex_coord / c_size;
        return textureSample(atlas_color_tex, atlas_sampler, uv);
    }
}
