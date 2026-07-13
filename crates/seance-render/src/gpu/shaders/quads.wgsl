// Floating quad shader for séance.
//
// One instanced pipeline draws filled and rounded rectangles. The rounded
// corner is an analytic signed-distance field (iq's rounded box), so no mask
// texture is needed and the radius is exact at any size.
//
// Native blending: the surface is non-sRGB, so colors composite in gamma
// space and pass through without conversion. The fragment premultiplies for
// the `One / OneMinusSrcAlpha` blend configured in pipeline.rs.

// Only `projection` is read; the full uniform buffer is larger, but a WGSL
// uniform var may be a prefix of the bound buffer.
struct QuadUniforms {
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: QuadUniforms;

struct VsIn {
    @builtin(vertex_index) vid: u32,
    // rect = (x, y, width, height) in physical pixels.
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) corner_radius: f32,
}

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) corner_radius: f32,
}

@vertex
fn vs_quad(in: VsIn) -> VsOut {
    // Unit-quad corners for a 4-vertex triangle strip:
    // vid 0→(0,0), 1→(1,0), 2→(0,1), 3→(1,1).
    let unit = vec2<f32>(f32(in.vid & 1u), f32((in.vid >> 1u) & 1u));
    let pixel = in.rect.xy + unit * in.rect.zw;

    var out: VsOut;
    out.position = uniforms.projection * vec4<f32>(pixel, 0.0, 1.0);
    out.color = in.color;
    out.center = in.rect.xy + in.rect.zw * 0.5;
    out.half_size = in.rect.zw * 0.5;
    out.corner_radius = in.corner_radius;
    return out;
}

// Signed distance to a rounded box centered at the origin (Inigo Quilez).
// Negative inside, zero on the edge, positive outside. Mirrored on the CPU
// in quads.rs's tests.
fn sd_rounded_box(p: vec2<f32>, half_size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_quad(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.position.xy - in.center;
    let d = sd_rounded_box(p, in.half_size, in.corner_radius);

    // One-pixel screen-space antialiased edge.
    let aa = max(fwidth(d), 1e-4);
    let coverage = 1.0 - smoothstep(0.0, aa, d);

    let a = in.color.a * coverage;
    return vec4<f32>(in.color.rgb * a, a);
}
