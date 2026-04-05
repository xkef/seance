// Cell rendering shaders for séance.
// Ported from Ghostty's Metal shaders.
//
// Colors are handled in linear space. The surface uses an sRGB format,
// so the GPU applies gamma encoding on write. All sRGB inputs (uniform
// colors, vertex colors, cell bg colors) are linearized in the shader
// before blending. The color emoji atlas uses Bgra8UnormSrgb, so the
// hardware linearizes texels on sample. The grayscale atlas is R8Unorm
// (coverage values, not color — no conversion needed).

struct Uniforms {
    projection: mat4x4<f32>,
    cell_size: vec2<f32>,
    grid_size: vec2<u32>,
    grid_padding: vec4<f32>,     // left, top, right, bottom
    bg_color: vec4<f32>,         // sRGB RGBA (linearized in shader)
    min_contrast: f32,
    cursor_visible: u32,         // 0 = hidden (e.g. in copy mode), 1 = visible
    cursor_pos: vec2<u32>,
    cursor_color: vec4<f32>,     // sRGB RGBA
    cursor_wide: u32,
    overlay_shape: u32,          // 0=hidden, 1=block, 2=underline, 3=bar
    overlay_pos: vec2<u32>,
    overlay_color: vec4<f32>,    // sRGB RGBA
    selection_start: vec2<u32>,  // (col, row) inclusive
    selection_end: vec2<u32>,    // (col, row) inclusive
    selection_color: vec4<f32>,  // sRGB RGBA
    selection_active: u32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// ================================================================
// sRGB ↔ linear conversion
// ================================================================

fn srgb_to_linear_component(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn linearize(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        srgb_to_linear_component(color.r),
        srgb_to_linear_component(color.g),
        srgb_to_linear_component(color.b),
        color.a,
    );
}

fn linear_to_srgb_component(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// ================================================================
// Min-contrast (WCAG relative luminance)
// ================================================================

fn luminance(linear_rgb: vec3<f32>) -> f32 {
    return dot(linear_rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn contrast_ratio(l1: f32, l2: f32) -> f32 {
    let lighter = max(l1, l2);
    let darker = min(l1, l2);
    return (lighter + 0.05) / (darker + 0.05);
}

fn apply_min_contrast(fg: vec3<f32>, bg: vec3<f32>, min_ratio: f32) -> vec3<f32> {
    if min_ratio <= 1.0 {
        return fg;
    }

    let fg_lum = luminance(fg);
    let bg_lum = luminance(bg);

    if contrast_ratio(fg_lum, bg_lum) >= min_ratio {
        return fg;
    }

    let target_lighter = min_ratio * (bg_lum + 0.05) - 0.05;
    let target_darker = (bg_lum + 0.05) / min_ratio - 0.05;

    var target_lum: f32;
    if target_lighter <= 1.0 {
        target_lum = target_lighter;
    } else if target_darker >= 0.0 {
        target_lum = target_darker;
    } else {
        target_lum = select(0.0, 1.0, abs(target_lighter - fg_lum) < abs(target_darker - fg_lum));
    }

    if fg_lum > 0.001 {
        let scale = target_lum / fg_lum;
        let adjusted = clamp(fg * scale, vec3<f32>(0.0), vec3<f32>(1.0));
        let adj_lum = luminance(adjusted);
        if abs(adj_lum - target_lum) > 0.01 {
            if target_lum > adj_lum {
                let t = (target_lum - adj_lum) / (1.0 - adj_lum + 0.001);
                return mix(adjusted, vec3<f32>(1.0), clamp(t, 0.0, 1.0));
            } else {
                let t = (adj_lum - target_lum) / (adj_lum + 0.001);
                return mix(adjusted, vec3<f32>(0.0), clamp(t, 0.0, 1.0));
            }
        }
        return adjusted;
    }
    return vec3<f32>(target_lum);
}

// ================================================================
// Selection and overlay helpers
// ================================================================

// Check if a grid cell is inside the selection range.
// Selection is a linear range from (start_col, start_row) to (end_col, end_row)
// in reading order (start <= end guaranteed by the host).
fn is_in_selection(col: u32, row: u32) -> bool {
    if uniforms.selection_active == 0u {
        return false;
    }
    let s = uniforms.selection_start;
    let e = uniforms.selection_end;

    // Before first row or after last row
    if row < s.y || row > e.y {
        return false;
    }
    // Single row: check column bounds
    if s.y == e.y {
        return col >= s.x && col <= e.x;
    }
    // First row: from start_col to end of line
    if row == s.y {
        return col >= s.x;
    }
    // Last row: from start of line to end_col
    if row == e.y {
        return col <= e.x;
    }
    // Middle rows: entire line is selected
    return true;
}

// ================================================================
// Full-screen vertex (used by bg_color and cell_bg passes)
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
// Background color pass: fills entire surface with bg color
// ================================================================

@fragment
fn fs_bg_color(in: FullScreenOut) -> @location(0) vec4<f32> {
    return linearize(uniforms.bg_color);
}

// ================================================================
// Cell background pass: per-cell background colors + selection + overlay cursor
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
    var color = linearize(unpack_rgba(packed));

    // Selection highlight: blend selection color over cell bg
    if is_in_selection(col, row) {
        let sel = linearize(uniforms.selection_color);
        // Alpha-blend selection over cell bg
        color = vec4<f32>(
            mix(color.rgb, sel.rgb, sel.a),
            max(color.a, sel.a),
        );
    }

    // Overlay cursor: draw cursor shape at overlay_pos
    if uniforms.overlay_shape != 0u
       && col == uniforms.overlay_pos.x
       && row == uniforms.overlay_pos.y {
        let local = pos - uniforms.cell_size * vec2<f32>(f32(col), f32(row));
        let cur_color = linearize(uniforms.overlay_color);
        var draw = false;

        if uniforms.overlay_shape == 1u {
            // Block: fill entire cell
            draw = true;
        } else if uniforms.overlay_shape == 2u {
            // Underline: bottom 2px (or ~12% of cell height)
            let thickness = max(2.0, uniforms.cell_size.y * 0.12);
            draw = local.y >= (uniforms.cell_size.y - thickness);
        } else if uniforms.overlay_shape == 3u {
            // Bar: left 2px (or ~10% of cell width)
            let thickness = max(2.0, uniforms.cell_size.x * 0.1);
            draw = local.x < thickness;
        }

        if draw {
            color = vec4<f32>(
                mix(color.rgb, cur_color.rgb, cur_color.a),
                max(color.a, cur_color.a),
            );
        }
    }

    // Premultiply alpha (in linear space)
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
    @location(3) @interpolate(flat) bg_color_linear: vec3<f32>,
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

    // Determine text color. If the VT cursor is visible and this cell
    // is under it, use the cursor color (matching Ghostty's behavior).
    // When cursor_visible == 0 (e.g. copy mode), skip this swap entirely
    // so the overlay cursor doesn't fight with the VT cursor.
    let is_cursor_glyph = (instance.atlas_and_flags & 0xFF00u) != 0u;
    let at_cursor = uniforms.cursor_visible != 0u
                 && instance.grid_pos.x == uniforms.cursor_pos.x
                 && instance.grid_pos.y == uniforms.cursor_pos.y;
    let at_cursor_wide = uniforms.cursor_visible != 0u
                      && uniforms.cursor_wide != 0u
                      && instance.grid_pos.x == uniforms.cursor_pos.x + 1u
                      && instance.grid_pos.y == uniforms.cursor_pos.y;
    var color = instance.color;
    if (at_cursor || at_cursor_wide) && !is_cursor_glyph {
        color = uniforms.cursor_color;
    }

    // Look up the cell's background color for min-contrast.
    let bg_idx = instance.grid_pos.y * uniforms.grid_size.x + instance.grid_pos.x;
    let bg_packed = bg_cells[bg_idx];
    let bg_srgb = unpack_rgba(bg_packed);
    let bg_linear = linearize(bg_srgb).rgb;
    let effective_bg = select(bg_linear, linearize(uniforms.bg_color).rgb, bg_srgb.a < 0.01);

    var out: CellTextOut;
    out.position = uniforms.projection * vec4<f32>(world_pos, 0.0, 1.0);
    out.tex_coord = vec2<f32>(f32(instance.glyph_pos.x), f32(instance.glyph_pos.y))
                  + vec2<f32>(f32(instance.glyph_size.x), f32(instance.glyph_size.y)) * corner;
    out.color = linearize(color);
    out.atlas = instance.atlas_and_flags & 0xFFu;
    out.bg_color_linear = effective_bg;

    return out;
}

@fragment
fn fs_cell_text(in: CellTextOut) -> @location(0) vec4<f32> {
    if in.atlas == 0u {
        // Grayscale atlas: coverage alpha. Color from vertex (already linear).
        let gs_size = vec2<f32>(textureDimensions(atlas_grayscale));
        let uv = in.tex_coord / gs_size;
        let a = textureSample(atlas_grayscale, atlas_sampler, uv).r;

        // Apply min-contrast adjustment against the cell's background.
        let fg = apply_min_contrast(in.color.rgb, in.bg_color_linear, uniforms.min_contrast);

        let alpha = a * in.color.a;
        return vec4<f32>(fg * alpha, alpha);
    } else {
        // Color atlas: emoji. Texture is Bgra8UnormSrgb so hardware
        // auto-linearizes on sample. Already premultiplied.
        let c_size = vec2<f32>(textureDimensions(atlas_color_tex));
        let uv = in.tex_coord / c_size;
        return textureSample(atlas_color_tex, atlas_sampler, uv);
    }
}
