// Cell rendering shaders for séance.
// Ported from Ghostty's Metal shaders.
//
// Native blending: the surface uses Bgra8Unorm (non-sRGB), so alpha
// compositing happens in gamma/sRGB space. Colors pass through as
// sRGB without conversion.

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

// ================================================================
// Min-contrast (WCAG relative luminance in linearized sRGB)
// ================================================================

fn linearize_component(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn linearize_rgb(rgb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        linearize_component(rgb.r),
        linearize_component(rgb.g),
        linearize_component(rgb.b),
    );
}

fn luminance(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn contrast_ratio(color1: vec3<f32>, color2: vec3<f32>) -> f32 {
    let l1 = luminance(color1);
    let l2 = luminance(color2);
    return (max(l1, l2) + 0.05) / (min(l1, l2) + 0.05);
}

fn apply_min_contrast(fg: vec3<f32>, bg: vec3<f32>, min_ratio: f32) -> vec3<f32> {
    if min_ratio <= 1.0 {
        return fg;
    }

    let fg_linear = linearize_rgb(fg);
    let bg_linear = linearize_rgb(bg);
    if contrast_ratio(fg_linear, bg_linear) >= min_ratio {
        return fg;
    }

    let white_ratio = contrast_ratio(vec3<f32>(1.0, 1.0, 1.0), bg_linear);
    let black_ratio = contrast_ratio(vec3<f32>(0.0, 0.0, 0.0), bg_linear);
    if white_ratio > black_ratio {
        return vec3<f32>(1.0, 1.0, 1.0);
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// ================================================================
// Selection helper
// ================================================================

fn is_in_selection(col: u32, row: u32) -> bool {
    if uniforms.selection_active == 0u {
        return false;
    }
    let s = uniforms.selection_start;
    let e = uniforms.selection_end;

    if row < s.y || row > e.y {
        return false;
    }
    if s.y == e.y {
        return col >= s.x && col <= e.x;
    }
    if row == s.y {
        return col >= s.x;
    }
    if row == e.y {
        return col <= e.x;
    }
    return true;
}

// ================================================================
// Full-screen vertex (bg_color and cell_bg passes)
// ================================================================

struct FullScreenOut {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> FullScreenOut {
    var out: FullScreenOut;
    let x = select(-1.0, 3.0, vid == 2u);
    let y = select(1.0, -3.0, vid == 0u);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// ================================================================
// Background color pass
// ================================================================

@fragment
fn fs_bg_color(in: FullScreenOut) -> @location(0) vec4<f32> {
    let bg = uniforms.bg_color;
    return vec4<f32>(bg.rgb * bg.a, bg.a);
}

// ================================================================
// Cell background pass
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

    let col = u32(grid_pos.x);
    let row = u32(grid_pos.y);
    let idx = row * uniforms.grid_size.x + col;
    let packed = bg_cells[idx];
    var color = unpack_rgba(packed);

    if is_in_selection(col, row) {
        let sel = uniforms.selection_color;
        color = vec4<f32>(
            mix(color.rgb, sel.rgb, sel.a),
            max(color.a, sel.a),
        );
    }

    if uniforms.cursor_visible != 0u
       && uniforms.overlay_shape != 0u
       && col == uniforms.overlay_pos.x
       && row == uniforms.overlay_pos.y {
        let local = pos - uniforms.cell_size * vec2<f32>(f32(col), f32(row));
        let cur_color = uniforms.overlay_color;
        var draw = false;

        if uniforms.overlay_shape == 1u {
            draw = true;
        } else if uniforms.overlay_shape == 2u {
            let thickness = max(2.0, uniforms.cell_size.y * 0.12);
            draw = local.y >= (uniforms.cell_size.y - thickness);
        } else if uniforms.overlay_shape == 3u {
            let thickness = max(2.0, uniforms.cell_size.x * 0.12);
            let top = uniforms.baseline * 0.3;
            let bottom = uniforms.baseline + (uniforms.cell_size.y - uniforms.baseline) * 0.85;
            draw = local.x < thickness && local.y >= top && local.y < bottom;
        }

        if draw {
            color = vec4<f32>(
                mix(color.rgb, cur_color.rgb, cur_color.a),
                max(color.a, cur_color.a),
            );
        }
    }

    color = vec4<f32>(color.rgb * color.a, color.a);
    return color;
}

// ================================================================
// Cell text pass
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
    @location(3) @interpolate(flat) bg_color: vec3<f32>,
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
    offset.y = uniforms.baseline - offset.y;

    let world_pos = cell_pos + size * corner + offset + uniforms.grid_padding.xy;

    let is_cursor_glyph = (instance.atlas_and_flags & 0xFF00u) != 0u;
    let at_cursor = uniforms.cursor_visible != 0u
                 && instance.grid_pos.x == uniforms.cursor_pos.x
                 && instance.grid_pos.y == uniforms.cursor_pos.y;
    let at_cursor_wide = uniforms.cursor_visible != 0u
                      && uniforms.cursor_wide != 0u
                      && instance.grid_pos.x == uniforms.cursor_pos.x + 1u
                      && instance.grid_pos.y == uniforms.cursor_pos.y;
    let bg_idx = instance.grid_pos.y * uniforms.grid_size.x + instance.grid_pos.x;
    let bg_packed = bg_cells[bg_idx];
    let bg_srgb = unpack_rgba(bg_packed);
    let effective_bg = select(bg_srgb.rgb, uniforms.bg_color.rgb, bg_srgb.a < 0.01);

    var color = instance.color;
    if (at_cursor || at_cursor_wide) && !is_cursor_glyph {
        if uniforms.overlay_shape == 1u {
            // Block cursor fills the cell; invert glyph to bg for legibility.
            color = vec4<f32>(effective_bg, color.a);
        } else {
            color = uniforms.cursor_color;
        }
    }

    var out: CellTextOut;
    out.position = uniforms.projection * vec4<f32>(world_pos, 0.0, 1.0);
    out.tex_coord = vec2<f32>(f32(instance.glyph_pos.x), f32(instance.glyph_pos.y))
                  + vec2<f32>(f32(instance.glyph_size.x), f32(instance.glyph_size.y)) * corner;
    out.color = color;
    out.atlas = instance.atlas_and_flags & 0xFFu;
    out.bg_color = effective_bg;

    return out;
}

@fragment
fn fs_cell_text(in: CellTextOut) -> @location(0) vec4<f32> {
    if in.atlas == 0u {
        let gs_size = vec2<f32>(textureDimensions(atlas_grayscale));
        let uv = in.tex_coord / gs_size;
        let a = textureSample(atlas_grayscale, atlas_sampler, uv).r;

        let fg = apply_min_contrast(in.color.rgb, in.bg_color, uniforms.min_contrast);

        let alpha = a * in.color.a;
        return vec4<f32>(fg * alpha, alpha);
    } else {
        let c_size = vec2<f32>(textureDimensions(atlas_color_tex));
        let uv = in.tex_coord / c_size;
        return textureSample(atlas_color_tex, atlas_sampler, uv);
    }
}
