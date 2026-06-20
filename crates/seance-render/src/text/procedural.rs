//! Procedural rasterization of Unicode box-drawing and block-element
//! codepoints (U+2500–U+259F) and the geometric Powerline separators
//! (U+E0B0–U+E0BF: triangles, semicircles, and corner wedges).
//!
//! Fonts ship inconsistent metrics for these glyphs, so monospace alignment
//! breaks at non-integer font sizes — long horizontal lines split into pieces,
//! corners drift away from the grid, powerline triangles overhang the cell.
//! Synthesizing the glyphs into a `cell_width × cell_height` alpha mask
//! sidesteps the problem entirely: lines meet pixel-perfectly because every
//! glyph is drawn relative to the same cell box.
//!
//! The output is fed through the same grayscale atlas as font glyphs, so the
//! GPU pipeline downstream of [`crate::text::cell_builder`] is unaware of the
//! origin. The interception happens in `CellBuilder`'s visitor: any cell whose
//! grapheme is a single codepoint in [`supports`] bypasses shaping.

use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke as SkStroke,
    Transform,
};

use super::backend::{CellMetrics, GlyphFormat, RasterizedGlyph};

/// Whether `c` has a procedural renderer. Cheap; safe to call in the visitor
/// hot loop.
pub(crate) fn supports(c: char) -> bool {
    lookup(c).is_some()
}

/// Rasterize `c` into a `cell_width × cell_height` grayscale alpha bitmap.
/// Returns `None` if `c` has no procedural renderer or the cell is degenerate.
pub(crate) fn rasterize(c: char, metrics: &CellMetrics) -> Option<RasterizedGlyph> {
    let kind = lookup(c)?;
    let width = (metrics.cell_width.round().max(1.0)) as u32;
    let height = (metrics.cell_height.round().max(1.0)) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    draw(kind, &mut pixmap, width, height);
    let data = extract_alpha(pixmap.data());
    // bearing_y = baseline: the cell shader computes
    //     offset.y = baseline - bearing_y
    // and a full-cell quad with bearing_y = baseline puts the bitmap's top
    // row at the top of the cell — which is what a cell-sized synthetic
    // glyph wants.
    Some(RasterizedGlyph {
        data,
        width,
        height,
        bearing_x: 0,
        bearing_y: metrics.baseline.round() as i32,
        format: GlyphFormat::Alpha,
    })
}

// ── encoding ────────────────────────────────────────────────────────────────

/// Stroke weight for one side of a box-drawing cell. `Double` is rendered as
/// two parallel light strokes separated by a one-stroke gap.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stroke {
    None,
    Light,
    Heavy,
    Double,
}

/// Order: north, east, south, west.
type Strokes = [Stroke; 4];

const N: usize = 0;
const E: usize = 1;
const S: usize = 2;
const W: usize = 3;

/// Which corner of the cell a quarter-arc sweeps through.
#[derive(Clone, Copy)]
enum ArcCorner {
    /// `╭` — stubs go down + right, arc curves through the top-left quadrant.
    TopLeft,
    /// `╮` — stubs go down + left, arc curves through the top-right quadrant.
    TopRight,
    /// `╯` — stubs go up + left, arc curves through the bottom-right quadrant.
    BottomRight,
    /// `╰` — stubs go up + right, arc curves through the bottom-left quadrant.
    BottomLeft,
}

#[derive(Clone, Copy)]
enum Diagonal {
    /// `╱` — bottom-left to top-right.
    Forward,
    /// `╲` — top-left to bottom-right.
    Back,
    /// `╳` — both diagonals.
    Cross,
}

/// 2×2 quadrant mask. Bit 0 = top-left, 1 = top-right, 2 = bottom-left,
/// 3 = bottom-right. Encodes every U+2596..=U+259F quadrant character.
#[derive(Clone, Copy)]
struct QuadrantMask(u8);

/// Sub-cell rectangle filling a fraction of the cell from one edge. Used for
/// half-blocks, eighth-blocks, and the lower N/8 / left N/8 ladders.
#[derive(Clone, Copy)]
enum BlockEdge {
    Lower,
    Upper,
    Left,
    Right,
}

/// One of the four Powerline separator triangles. The Filled variants fill
/// the half of the cell whose vertical edge sits on the indicated side, with
/// the apex on the opposite mid-edge. The Thin variants draw only the
/// hypotenuse as a stroke.
#[derive(Clone, Copy)]
enum PowerlineTriangle {
    /// U+E0B0 — solid rightward triangle. Vertical edge at x = 0, apex at
    /// (cell_width, cell_height / 2).
    RightFilled,
    /// U+E0B1 — outline rightward triangle (hypotenuse only).
    RightThin,
    /// U+E0B2 — solid leftward triangle (mirror of `RightFilled`).
    LeftFilled,
    /// U+E0B3 — outline leftward triangle (mirror of `RightThin`).
    LeftThin,
}

/// A Powerline rounded separator (U+E0B4–U+E0B7): a half-ellipse spanning the
/// full cell, flat diameter flush against one vertical edge and the curved
/// side bulging out to the opposite mid-edge. Filled variants fill the disc;
/// thin variants stroke the arc only.
#[derive(Clone, Copy)]
enum Semicircle {
    /// U+E0B4 — diameter on the left edge, bulge to (cell_width, h/2).
    RightFilled,
    /// U+E0B5 — outline of `RightFilled`.
    RightThin,
    /// U+E0B6 — diameter on the right edge, bulge to (0, h/2).
    LeftFilled,
    /// U+E0B7 — outline of `LeftFilled`.
    LeftThin,
}

/// A filled Powerline corner wedge (U+E0B8, U+E0BA, U+E0BC, U+E0BE): the
/// half-cell right triangle adjacent to the named corner, split off by a cell
/// diagonal. The matching thin wedges (U+E0B9/BB/BD/BF) are that diagonal on
/// its own and reuse [`Diagonal`].
#[derive(Clone, Copy)]
enum CornerTriangle {
    /// U+E0B8 — fills below the main diagonal (top-left → bottom-right).
    LowerLeft,
    /// U+E0BA — fills below the anti-diagonal (top-right → bottom-left).
    LowerRight,
    /// U+E0BC — fills above the anti-diagonal.
    UpperLeft,
    /// U+E0BE — fills above the main diagonal.
    UpperRight,
}

#[derive(Clone, Copy)]
enum RenderKind {
    Box(Strokes),
    Powerline(PowerlineTriangle),
    Semicircle(Semicircle),
    CornerTriangle(CornerTriangle),
    Arc(ArcCorner),
    Diagonal(Diagonal),
    Quadrant(QuadrantMask),
    /// Filled rectangle anchored to `edge`, covering `numerator / 8` of the
    /// cell. `8/8` is a full block.
    BlockFraction {
        edge: BlockEdge,
        numerator: u8,
    },
    /// Constant alpha covering the whole cell. Used for shade glyphs.
    Shade(u8),
}

// ── registry ────────────────────────────────────────────────────────────────

fn lookup(c: char) -> Option<RenderKind> {
    let cp = c as u32;
    match cp {
        0x2500..=0x259F => lookup_in_range(cp),
        0xE0B0..=0xE0BF => lookup_powerline(cp),
        _ => None,
    }
}

fn lookup_powerline(cp: u32) -> Option<RenderKind> {
    Some(match cp {
        // Arrow separators.
        0xE0B0 => RenderKind::Powerline(PowerlineTriangle::RightFilled),
        0xE0B1 => RenderKind::Powerline(PowerlineTriangle::RightThin),
        0xE0B2 => RenderKind::Powerline(PowerlineTriangle::LeftFilled),
        0xE0B3 => RenderKind::Powerline(PowerlineTriangle::LeftThin),

        // Rounded separators.
        0xE0B4 => RenderKind::Semicircle(Semicircle::RightFilled),
        0xE0B5 => RenderKind::Semicircle(Semicircle::RightThin),
        0xE0B6 => RenderKind::Semicircle(Semicircle::LeftFilled),
        0xE0B7 => RenderKind::Semicircle(Semicircle::LeftThin),

        // Corner wedges. The thin variants are exactly a cell diagonal, so
        // they reuse the box-drawing diagonal renderer.
        0xE0B8 => RenderKind::CornerTriangle(CornerTriangle::LowerLeft),
        0xE0B9 => RenderKind::Diagonal(Diagonal::Back),
        0xE0BA => RenderKind::CornerTriangle(CornerTriangle::LowerRight),
        0xE0BB => RenderKind::Diagonal(Diagonal::Forward),
        0xE0BC => RenderKind::CornerTriangle(CornerTriangle::UpperLeft),
        0xE0BD => RenderKind::Diagonal(Diagonal::Forward),
        0xE0BE => RenderKind::CornerTriangle(CornerTriangle::UpperRight),
        0xE0BF => RenderKind::Diagonal(Diagonal::Back),

        _ => return None,
    })
}

fn lookup_in_range(cp: u32) -> Option<RenderKind> {
    use Stroke::*;
    // Horizontals / verticals / corners / tees / crosses laid out in the
    // U+2500..U+254B block. Each entry packs (N, E, S, W) directly.
    let strokes: Strokes = match cp {
        // ── horizontal / vertical lines ─────────────────────────────────────
        0x2500 => [None, Light, None, Light], // ─
        0x2501 => [None, Heavy, None, Heavy], // ━
        0x2502 => [Light, None, Light, None], // │
        0x2503 => [Heavy, None, Heavy, None], // ┃

        // ── top-left corners ────────────────────────────────────────────────
        0x250C => [None, Light, Light, None], // ┌
        0x250D => [None, Heavy, Light, None], // ┍
        0x250E => [None, Light, Heavy, None], // ┎
        0x250F => [None, Heavy, Heavy, None], // ┏

        // ── top-right corners ───────────────────────────────────────────────
        0x2510 => [None, None, Light, Light], // ┐
        0x2511 => [None, None, Light, Heavy], // ┑
        0x2512 => [None, None, Heavy, Light], // ┒
        0x2513 => [None, None, Heavy, Heavy], // ┓

        // ── bottom-left corners ─────────────────────────────────────────────
        0x2514 => [Light, Light, None, None], // └
        0x2515 => [Light, Heavy, None, None], // ┕
        0x2516 => [Heavy, Light, None, None], // ┖
        0x2517 => [Heavy, Heavy, None, None], // ┗

        // ── bottom-right corners ────────────────────────────────────────────
        0x2518 => [Light, None, None, Light], // ┘
        0x2519 => [Light, None, None, Heavy], // ┙
        0x251A => [Heavy, None, None, Light], // ┚
        0x251B => [Heavy, None, None, Heavy], // ┛

        // ── left-side tees (├ family) ───────────────────────────────────────
        0x251C => [Light, Light, Light, None], // ├
        0x251D => [Light, Heavy, Light, None], // ┝
        0x251E => [Heavy, Light, Light, None], // ┞
        0x251F => [Light, Light, Heavy, None], // ┟
        0x2520 => [Heavy, Light, Heavy, None], // ┠
        0x2521 => [Heavy, Heavy, Light, None], // ┡
        0x2522 => [Light, Heavy, Heavy, None], // ┢
        0x2523 => [Heavy, Heavy, Heavy, None], // ┣

        // ── right-side tees (┤ family) ──────────────────────────────────────
        0x2524 => [Light, None, Light, Light], // ┤
        0x2525 => [Light, None, Light, Heavy], // ┥
        0x2526 => [Heavy, None, Light, Light], // ┦
        0x2527 => [Light, None, Heavy, Light], // ┧
        0x2528 => [Heavy, None, Heavy, Light], // ┨
        0x2529 => [Heavy, None, Light, Heavy], // ┩
        0x252A => [Light, None, Heavy, Heavy], // ┪
        0x252B => [Heavy, None, Heavy, Heavy], // ┫

        // ── top tees (┬ family) ─────────────────────────────────────────────
        0x252C => [None, Light, Light, Light], // ┬
        0x252D => [None, Light, Light, Heavy], // ┭
        0x252E => [None, Heavy, Light, Light], // ┮
        0x252F => [None, Heavy, Light, Heavy], // ┯
        0x2530 => [None, Light, Heavy, Light], // ┰
        0x2531 => [None, Light, Heavy, Heavy], // ┱
        0x2532 => [None, Heavy, Heavy, Light], // ┲
        0x2533 => [None, Heavy, Heavy, Heavy], // ┳

        // ── bottom tees (┴ family) ──────────────────────────────────────────
        0x2534 => [Light, Light, None, Light], // ┴
        0x2535 => [Light, Light, None, Heavy], // ┵
        0x2536 => [Light, Heavy, None, Light], // ┶
        0x2537 => [Light, Heavy, None, Heavy], // ┷
        0x2538 => [Heavy, Light, None, Light], // ┸
        0x2539 => [Heavy, Light, None, Heavy], // ┹
        0x253A => [Heavy, Heavy, None, Light], // ┺
        0x253B => [Heavy, Heavy, None, Heavy], // ┻

        // ── crosses (┼ family) ──────────────────────────────────────────────
        0x253C => [Light, Light, Light, Light], // ┼
        0x253D => [Light, Light, Light, Heavy], // ┽
        0x253E => [Light, Heavy, Light, Light], // ┾
        0x253F => [Light, Heavy, Light, Heavy], // ┿
        0x2540 => [Heavy, Light, Light, Light], // ╀
        0x2541 => [Light, Light, Heavy, Light], // ╁
        0x2542 => [Heavy, Light, Heavy, Light], // ╂
        0x2543 => [Heavy, Light, Light, Heavy], // ╃
        0x2544 => [Heavy, Heavy, Light, Light], // ╄
        0x2545 => [Light, Light, Heavy, Heavy], // ╅
        0x2546 => [Light, Heavy, Heavy, Light], // ╆
        0x2547 => [Heavy, Heavy, Light, Heavy], // ╇
        0x2548 => [Light, Heavy, Heavy, Heavy], // ╈
        0x2549 => [Heavy, Light, Heavy, Heavy], // ╉
        0x254A => [Heavy, Heavy, Heavy, Light], // ╊
        0x254B => [Heavy, Heavy, Heavy, Heavy], // ╋

        // ── double-line family (U+2550..=U+256C) ────────────────────────────
        0x2550 => [None, Double, None, Double],     // ═
        0x2551 => [Double, None, Double, None],     // ║
        0x2552 => [None, Double, Light, None],      // ╒
        0x2553 => [None, Light, Double, None],      // ╓
        0x2554 => [None, Double, Double, None],     // ╔
        0x2555 => [None, None, Light, Double],      // ╕
        0x2556 => [None, None, Double, Light],      // ╖
        0x2557 => [None, None, Double, Double],     // ╗
        0x2558 => [Light, Double, None, None],      // ╘
        0x2559 => [Double, Light, None, None],      // ╙
        0x255A => [Double, Double, None, None],     // ╚
        0x255B => [Light, None, None, Double],      // ╛
        0x255C => [Double, None, None, Light],      // ╜
        0x255D => [Double, None, None, Double],     // ╝
        0x255E => [Light, Double, Light, None],     // ╞
        0x255F => [Double, Light, Double, None],    // ╟
        0x2560 => [Double, Double, Double, None],   // ╠
        0x2561 => [Light, None, Light, Double],     // ╡
        0x2562 => [Double, None, Double, Light],    // ╢
        0x2563 => [Double, None, Double, Double],   // ╣
        0x2564 => [None, Double, Light, Double],    // ╤
        0x2565 => [None, Light, Double, Light],     // ╥
        0x2566 => [None, Double, Double, Double],   // ╦
        0x2567 => [Light, Double, None, Double],    // ╧
        0x2568 => [Double, Light, None, Light],     // ╨
        0x2569 => [Double, Double, None, Double],   // ╩
        0x256A => [Light, Double, Light, Double],   // ╪
        0x256B => [Double, Light, Double, Light],   // ╫
        0x256C => [Double, Double, Double, Double], // ╬

        // ── half-line stubs ─────────────────────────────────────────────────
        0x2574 => [None, None, None, Light],  // ╴
        0x2575 => [Light, None, None, None],  // ╵
        0x2576 => [None, Light, None, None],  // ╶
        0x2577 => [None, None, Light, None],  // ╷
        0x2578 => [None, None, None, Heavy],  // ╸
        0x2579 => [Heavy, None, None, None],  // ╹
        0x257A => [None, Heavy, None, None],  // ╺
        0x257B => [None, None, Heavy, None],  // ╻
        0x257C => [None, Heavy, None, Light], // ╼
        0x257D => [Light, None, Heavy, None], // ╽
        0x257E => [None, Light, None, Heavy], // ╾
        0x257F => [Heavy, None, Light, None], // ╿

        _ => return next_kind(cp),
    };
    Some(RenderKind::Box(strokes))
}

/// Codepoints outside the simple stroke-matrix tables — arcs, diagonals,
/// block elements, quadrants, shades.
fn next_kind(cp: u32) -> Option<RenderKind> {
    use ArcCorner::*;
    use BlockEdge::*;
    use Diagonal::*;
    Some(match cp {
        // Light arcs.
        0x256D => RenderKind::Arc(TopLeft),     // ╭
        0x256E => RenderKind::Arc(TopRight),    // ╮
        0x256F => RenderKind::Arc(BottomRight), // ╯
        0x2570 => RenderKind::Arc(BottomLeft),  // ╰

        // Diagonals.
        0x2571 => RenderKind::Diagonal(Forward), // ╱
        0x2572 => RenderKind::Diagonal(Back),    // ╲
        0x2573 => RenderKind::Diagonal(Cross),   // ╳

        // ── block elements U+2580..=U+2590 ──────────────────────────────────
        // ▀ upper half block (= upper 4/8).
        0x2580 => RenderKind::BlockFraction {
            edge: Upper,
            numerator: 4,
        },
        // Lower N/8 blocks: ▁ ▂ ▃ ▄ ▅ ▆ ▇ █
        0x2581..=0x2588 => RenderKind::BlockFraction {
            edge: Lower,
            numerator: (cp - 0x2580) as u8,
        },
        // Left N/8 blocks: ▉ ▊ ▋ ▌ ▍ ▎ ▏  (▉ = 7/8, descending to 1/8)
        0x2589..=0x258F => RenderKind::BlockFraction {
            edge: Left,
            numerator: (0x2590 - cp) as u8,
        },
        // ▐ right half block (= right 4/8).
        0x2590 => RenderKind::BlockFraction {
            edge: Right,
            numerator: 4,
        },

        // Shades.
        0x2591 => RenderKind::Shade(64),
        0x2592 => RenderKind::Shade(128),
        0x2593 => RenderKind::Shade(192),

        // ▔ upper 1/8 block.
        0x2594 => RenderKind::BlockFraction {
            edge: Upper,
            numerator: 1,
        },
        // ▕ right 1/8 block.
        0x2595 => RenderKind::BlockFraction {
            edge: Right,
            numerator: 1,
        },

        // ── quadrant blocks U+2596..=U+259F ─────────────────────────────────
        // mask bits: 1 = TL, 2 = TR, 4 = BL, 8 = BR.
        0x2596 => RenderKind::Quadrant(QuadrantMask(0b0100)), // ▖ lower-left
        0x2597 => RenderKind::Quadrant(QuadrantMask(0b1000)), // ▗ lower-right
        0x2598 => RenderKind::Quadrant(QuadrantMask(0b0001)), // ▘ upper-left
        0x2599 => RenderKind::Quadrant(QuadrantMask(0b1101)), // ▙ TL+BL+BR
        0x259A => RenderKind::Quadrant(QuadrantMask(0b1001)), // ▚ TL+BR
        0x259B => RenderKind::Quadrant(QuadrantMask(0b0111)), // ▛ TL+TR+BL
        0x259C => RenderKind::Quadrant(QuadrantMask(0b1011)), // ▜ TL+TR+BR
        0x259D => RenderKind::Quadrant(QuadrantMask(0b0010)), // ▝ upper-right
        0x259E => RenderKind::Quadrant(QuadrantMask(0b0110)), // ▞ TR+BL
        0x259F => RenderKind::Quadrant(QuadrantMask(0b1110)), // ▟ TR+BL+BR

        _ => return None,
    })
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// Light-stroke width in pixels; minimum 1 so a thin glyph never disappears at
/// small cell sizes.
fn light_width(cell_height: u32) -> f32 {
    ((cell_height as f32) / 12.0).round().max(1.0)
}

/// Heavy is roughly 2× light, but always at least one pixel wider so the
/// contrast is visible at every cell size.
fn heavy_width(cell_height: u32) -> f32 {
    let light = light_width(cell_height);
    (light * 2.0).round().max(light + 1.0)
}

fn white_paint() -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.blend_mode = BlendMode::Source;
    paint.anti_alias = true;
    paint
}

fn draw(kind: RenderKind, pixmap: &mut Pixmap, w: u32, h: u32) {
    match kind {
        RenderKind::Box(strokes) => draw_box(strokes, pixmap, w, h),
        RenderKind::Powerline(triangle) => draw_powerline(triangle, pixmap, w, h),
        RenderKind::Semicircle(kind) => draw_semicircle(kind, pixmap, w, h),
        RenderKind::CornerTriangle(kind) => draw_corner_triangle(kind, pixmap, w, h),
        RenderKind::Arc(corner) => draw_arc(corner, pixmap, w, h),
        RenderKind::Diagonal(kind) => draw_diagonal(kind, pixmap, w, h),
        RenderKind::Quadrant(mask) => draw_quadrants(mask, pixmap, w, h),
        RenderKind::BlockFraction { edge, numerator } => {
            draw_block_fraction(edge, numerator, pixmap, w, h);
        }
        RenderKind::Shade(alpha) => draw_shade(alpha, pixmap, w, h),
    }
}

fn draw_box(strokes: Strokes, pixmap: &mut Pixmap, w: u32, h: u32) {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let light = light_width(h);
    let heavy = heavy_width(h);

    for (dir, &stroke) in strokes.iter().enumerate() {
        if stroke == Stroke::None {
            continue;
        }
        let (sx, sy, ex, ey) = endpoints(dir, cx, cy, w as f32, h as f32);
        match stroke {
            Stroke::Light => stroke_line(pixmap, sx, sy, ex, ey, light),
            Stroke::Heavy => stroke_line(pixmap, sx, sy, ex, ey, heavy),
            Stroke::Double => stroke_double_line(pixmap, dir, sx, sy, ex, ey, light),
            Stroke::None => unreachable!(),
        }
    }
}

/// Endpoint pair for a stroke pointing in `dir`: starts at the cell center and
/// terminates at the cell edge. `dir` matches the [`Strokes`] index order:
/// 0 = north, 1 = east, 2 = south, 3 = west.
fn endpoints(dir: usize, cx: f32, cy: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    match dir {
        N => (cx, cy, cx, 0.0),
        E => (cx, cy, w, cy),
        S => (cx, cy, cx, h),
        W => (cx, cy, 0.0, cy),
        _ => unreachable!(),
    }
}

fn stroke_line(pixmap: &mut Pixmap, sx: f32, sy: f32, ex: f32, ey: f32, width: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(sx, sy);
    pb.line_to(ex, ey);
    let Some(path) = pb.finish() else {
        return;
    };
    let stroke = SkStroke {
        width,
        line_cap: LineCap::Butt,
        ..SkStroke::default()
    };
    pixmap.stroke_path(&path, &white_paint(), &stroke, Transform::identity(), None);
}

/// Draw a double-line stub: two parallel light strokes offset from the axis by
/// `gap`. The strokes run from the cell edge inward to the cell center along
/// `dir`.
fn stroke_double_line(
    pixmap: &mut Pixmap,
    dir: usize,
    sx: f32,
    sy: f32,
    ex: f32,
    ey: f32,
    light: f32,
) {
    // Offset perpendicular to the stroke direction by `light` pixels so the
    // two rails of the double line don't bleed into each other at small cell
    // sizes.
    let offset = light;
    let (ox, oy) = match dir {
        N | S => (offset, 0.0),
        E | W => (0.0, offset),
        _ => (0.0, 0.0),
    };
    stroke_line(pixmap, sx - ox, sy - oy, ex - ox, ey - oy, light);
    stroke_line(pixmap, sx + ox, sy + oy, ex + ox, ey + oy, light);
}

fn draw_arc(corner: ArcCorner, pixmap: &mut Pixmap, w: u32, h: u32) {
    let wf = w as f32;
    let hf = h as f32;
    let cx = wf / 2.0;
    let cy = hf / 2.0;
    let light = light_width(h);
    // Radius small enough to leave room for the straight stubs.
    let radius = (cx.min(cy) * 0.6).max(light);

    // Each arc connects a vertical stub at the cell's mid-x to a horizontal
    // stub at the cell's mid-y. The arc center sits on the diagonal opposite
    // the curved corner.
    let (start, end, center, stub_v, stub_h) = match corner {
        ArcCorner::TopLeft => {
            // ╭: stubs go right and down. Curve through the upper-left
            // quadrant; arc center at (cx + radius, cy + radius).
            (
                (cx, cy + radius),
                (cx + radius, cy),
                (cx + radius, cy + radius),
                (cx, cy + radius, cx, hf), // vertical stub: arc end → bottom
                (cx + radius, cy, wf, cy), // horizontal stub: arc end → right
            )
        }
        ArcCorner::TopRight => {
            // ╮: stubs go left and down. Curve through upper-right;
            // arc center at (cx - radius, cy + radius).
            (
                (cx, cy + radius),
                (cx - radius, cy),
                (cx - radius, cy + radius),
                (cx, cy + radius, cx, hf),
                (cx - radius, cy, 0.0, cy),
            )
        }
        ArcCorner::BottomRight => {
            // ╯: stubs go left and up. Curve through lower-right;
            // arc center at (cx - radius, cy - radius).
            (
                (cx, cy - radius),
                (cx - radius, cy),
                (cx - radius, cy - radius),
                (cx, cy - radius, cx, 0.0),
                (cx - radius, cy, 0.0, cy),
            )
        }
        ArcCorner::BottomLeft => {
            // ╰: stubs go right and up. Curve through lower-left;
            // arc center at (cx + radius, cy - radius).
            (
                (cx, cy - radius),
                (cx + radius, cy),
                (cx + radius, cy - radius),
                (cx, cy - radius, cx, 0.0),
                (cx + radius, cy, wf, cy),
            )
        }
    };

    // Quarter-circle via cubic Bezier; k is the standard control-point
    // factor that approximates a 90° arc to within ~0.02%.
    const K: f32 = 0.552_284_8;
    let (sx, sy) = start;
    let (ex, ey) = end;
    let (cxc, cyc) = center;
    let c1 = (sx + (cxc - sx) * (1.0 - K), sy + (cyc - sy) * (1.0 - K));
    let c2 = (ex + (cxc - ex) * (1.0 - K), ey + (cyc - ey) * (1.0 - K));

    let mut pb = PathBuilder::new();
    pb.move_to(sx, sy);
    pb.cubic_to(c1.0, c1.1, c2.0, c2.1, ex, ey);
    if let Some(path) = pb.finish() {
        let stroke = SkStroke {
            width: light,
            line_cap: LineCap::Butt,
            ..SkStroke::default()
        };
        pixmap.stroke_path(&path, &white_paint(), &stroke, Transform::identity(), None);
    }

    stroke_line(pixmap, stub_v.0, stub_v.1, stub_v.2, stub_v.3, light);
    stroke_line(pixmap, stub_h.0, stub_h.1, stub_h.2, stub_h.3, light);
}

fn draw_powerline(triangle: PowerlineTriangle, pixmap: &mut Pixmap, w: u32, h: u32) {
    let wf = w as f32;
    let hf = h as f32;
    let mid_y = hf / 2.0;
    let mut pb = PathBuilder::new();
    match triangle {
        PowerlineTriangle::RightFilled | PowerlineTriangle::RightThin => {
            pb.move_to(0.0, 0.0);
            pb.line_to(wf, mid_y);
            pb.line_to(0.0, hf);
        }
        PowerlineTriangle::LeftFilled | PowerlineTriangle::LeftThin => {
            pb.move_to(wf, 0.0);
            pb.line_to(0.0, mid_y);
            pb.line_to(wf, hf);
        }
    }
    let filled = matches!(
        triangle,
        PowerlineTriangle::RightFilled | PowerlineTriangle::LeftFilled
    );
    if filled {
        pb.close();
        let Some(path) = pb.finish() else { return };
        pixmap.fill_path(
            &path,
            &white_paint(),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        let Some(path) = pb.finish() else { return };
        // Round join keeps the apex from spiking out via miter at the
        // acute angle the two segments form.
        let stroke = SkStroke {
            width: light_width(h),
            line_cap: LineCap::Butt,
            line_join: LineJoin::Round,
            ..SkStroke::default()
        };
        pixmap.stroke_path(&path, &white_paint(), &stroke, Transform::identity(), None);
    }
}

fn draw_semicircle(kind: Semicircle, pixmap: &mut Pixmap, w: u32, h: u32) {
    let wf = w as f32;
    let hf = h as f32;
    let mid_y = hf / 2.0;
    // Half-ellipse: rx spans the whole cell width, ry is half the height.
    // The diameter sits on `flat_x`; the curve bulges out to `apex_x`.
    let (apex_x, flat_x) = match kind {
        Semicircle::RightFilled | Semicircle::RightThin => (wf, 0.0),
        Semicircle::LeftFilled | Semicircle::LeftThin => (0.0, wf),
    };
    // Cubic control-point factor for a quarter ellipse (see `draw_arc`).
    const K: f32 = 0.552_284_8;
    let dx = (apex_x - flat_x) * K;
    let dy = mid_y * K;

    let mut pb = PathBuilder::new();
    pb.move_to(flat_x, 0.0);
    // Top quarter: diameter top → apex.
    pb.cubic_to(flat_x + dx, 0.0, apex_x, mid_y - dy, apex_x, mid_y);
    // Bottom quarter: apex → diameter bottom.
    pb.cubic_to(apex_x, mid_y + dy, flat_x + dx, hf, flat_x, hf);

    let filled = matches!(kind, Semicircle::RightFilled | Semicircle::LeftFilled);
    if filled {
        pb.close();
        let Some(path) = pb.finish() else { return };
        pixmap.fill_path(
            &path,
            &white_paint(),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        let Some(path) = pb.finish() else { return };
        let stroke = SkStroke {
            width: light_width(h),
            line_cap: LineCap::Butt,
            line_join: LineJoin::Round,
            ..SkStroke::default()
        };
        pixmap.stroke_path(&path, &white_paint(), &stroke, Transform::identity(), None);
    }
}

fn draw_corner_triangle(kind: CornerTriangle, pixmap: &mut Pixmap, w: u32, h: u32) {
    let wf = w as f32;
    let hf = h as f32;
    let pts = match kind {
        CornerTriangle::LowerLeft => [(0.0, 0.0), (0.0, hf), (wf, hf)],
        CornerTriangle::LowerRight => [(wf, 0.0), (0.0, hf), (wf, hf)],
        CornerTriangle::UpperLeft => [(0.0, 0.0), (wf, 0.0), (0.0, hf)],
        CornerTriangle::UpperRight => [(0.0, 0.0), (wf, 0.0), (wf, hf)],
    };
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    pb.line_to(pts[1].0, pts[1].1);
    pb.line_to(pts[2].0, pts[2].1);
    pb.close();
    let Some(path) = pb.finish() else { return };
    pixmap.fill_path(
        &path,
        &white_paint(),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn draw_diagonal(kind: Diagonal, pixmap: &mut Pixmap, w: u32, h: u32) {
    let wf = w as f32;
    let hf = h as f32;
    let light = light_width(h);
    match kind {
        Diagonal::Forward => stroke_line(pixmap, 0.0, hf, wf, 0.0, light),
        Diagonal::Back => stroke_line(pixmap, 0.0, 0.0, wf, hf, light),
        Diagonal::Cross => {
            stroke_line(pixmap, 0.0, hf, wf, 0.0, light);
            stroke_line(pixmap, 0.0, 0.0, wf, hf, light);
        }
    }
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32) {
    let Some(rect) = Rect::from_xywh(x, y, w.max(0.0), h.max(0.0)) else {
        return;
    };
    pixmap.fill_rect(rect, &white_paint(), Transform::identity(), None);
}

fn draw_quadrants(mask: QuadrantMask, pixmap: &mut Pixmap, w: u32, h: u32) {
    let half_w = (w as f32) / 2.0;
    let half_h = (h as f32) / 2.0;
    if mask.0 & 0b0001 != 0 {
        fill_rect(pixmap, 0.0, 0.0, half_w, half_h);
    }
    if mask.0 & 0b0010 != 0 {
        fill_rect(pixmap, half_w, 0.0, w as f32 - half_w, half_h);
    }
    if mask.0 & 0b0100 != 0 {
        fill_rect(pixmap, 0.0, half_h, half_w, h as f32 - half_h);
    }
    if mask.0 & 0b1000 != 0 {
        fill_rect(pixmap, half_w, half_h, w as f32 - half_w, h as f32 - half_h);
    }
}

fn draw_block_fraction(edge: BlockEdge, numerator: u8, pixmap: &mut Pixmap, w: u32, h: u32) {
    let frac = f32::from(numerator) / 8.0;
    let wf = w as f32;
    let hf = h as f32;
    match edge {
        BlockEdge::Lower => {
            let band = hf * frac;
            fill_rect(pixmap, 0.0, hf - band, wf, band);
        }
        BlockEdge::Upper => {
            let band = hf * frac;
            fill_rect(pixmap, 0.0, 0.0, wf, band);
        }
        BlockEdge::Left => {
            let band = wf * frac;
            fill_rect(pixmap, 0.0, 0.0, band, hf);
        }
        BlockEdge::Right => {
            let band = wf * frac;
            fill_rect(pixmap, wf - band, 0.0, band, hf);
        }
    }
}

fn draw_shade(alpha: u8, pixmap: &mut Pixmap, w: u32, h: u32) {
    if alpha == 0 {
        return;
    }
    let mut paint = white_paint();
    paint.set_color_rgba8(255, 255, 255, alpha);
    let Some(rect) = Rect::from_xywh(0.0, 0.0, w as f32, h as f32) else {
        return;
    };
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    // Silence unused-fill-rule import lints in some configurations.
    let _ = FillRule::Winding;
}

/// tiny-skia stores premultiplied BGRA; the alpha channel is byte 3 of every
/// pixel. Pull it out into a tight `Vec<u8>` matching the grayscale atlas
/// layout.
fn extract_alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).map(|p| p[3]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(cell_w: u32, cell_h: u32) -> CellMetrics {
        CellMetrics {
            cell_width: cell_w as f32,
            cell_height: cell_h as f32,
            baseline: cell_h as f32 * 0.8,
        }
    }

    #[test]
    fn supports_returns_true_for_all_documented_codepoints() {
        // Spot-check each subrange of the registry.
        for cp in [
            0x2500u32, 0x2501, 0x2502, 0x2503, 0x250C, 0x2518, 0x251C, 0x2534, 0x253C, 0x254B,
            0x2550, 0x2551, 0x2554, 0x2557, 0x255A, 0x255D, 0x256C, 0x256D, 0x2570, 0x2571, 0x2573,
            0x2574, 0x257F, 0x2580, 0x2584, 0x2588, 0x258F, 0x2590, 0x2591, 0x2593, 0x2594, 0x2595,
            0x2596, 0x259F,
        ] {
            let c = char::from_u32(cp).unwrap();
            assert!(supports(c), "expected procedural support for U+{cp:04X}");
        }
    }

    #[test]
    fn supports_rejects_codepoints_outside_block() {
        for c in ['a', '0', '€', '中', ' ', '\n'] {
            assert!(!supports(c), "did not expect procedural support for {c:?}");
        }
    }

    #[test]
    fn rasterize_produces_cell_sized_alpha_bitmap() {
        let m = metrics(10, 20);
        let g = rasterize('─', &m).unwrap();
        assert_eq!(g.width, 10);
        assert_eq!(g.height, 20);
        assert_eq!(g.data.len(), 10 * 20);
        assert_eq!(g.format, GlyphFormat::Alpha);
        assert_eq!(g.bearing_x, 0);
        assert_eq!(g.bearing_y, 16);
    }

    #[test]
    fn horizontal_line_writes_across_a_middle_row() {
        // U+2500 ─ should put ink in some pixel of column 0 and column w-1
        // on the same row band, regardless of cell size.
        let m = metrics(12, 24);
        let g = rasterize('─', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        let mid = h / 2;
        // Inspect the rows around the middle for the stroke (it can be 1–2
        // pixels tall after anti-aliasing).
        let mut left = 0u8;
        let mut right = 0u8;
        for dy in [-1i32, 0, 1] {
            let row = (mid as i32 + dy) as usize;
            left = left.max(g.data[row * w]);
            right = right.max(g.data[row * w + (w - 1)]);
        }
        assert!(left > 0, "expected ink on left edge of ─");
        assert!(right > 0, "expected ink on right edge of ─");
    }

    #[test]
    fn vertical_line_writes_top_and_bottom_pixels() {
        let m = metrics(10, 20);
        let g = rasterize('│', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        let mid = w / 2;
        // Anti-aliasing can leave the middle column at 0 if the stroke
        // straddles the column boundary; sweep a 1-pixel-wide band instead.
        let band: Vec<usize> = [mid.saturating_sub(1), mid, mid + 1]
            .into_iter()
            .filter(|c| *c < w)
            .collect();
        let max_at = |row: usize| band.iter().map(|c| g.data[row * w + c]).max().unwrap_or(0);
        assert!(max_at(0) > 0, "expected ink in top of │");
        assert!(max_at(h - 1) > 0, "expected ink in bottom of │");
    }

    #[test]
    fn block_full_fills_entire_cell() {
        let m = metrics(8, 16);
        let g = rasterize('█', &m).unwrap();
        // Full block: every pixel saturated.
        assert!(g.data.iter().all(|&p| p == 255));
    }

    #[test]
    fn lower_half_block_fills_bottom_half_only() {
        let m = metrics(8, 16);
        let g = rasterize('▄', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        // Top row should be empty, bottom row should be saturated.
        assert_eq!(g.data[0], 0);
        assert_eq!(g.data[(h - 1) * w], 255);
    }

    #[test]
    fn upper_quadrant_fills_only_upper_left() {
        let m = metrics(8, 16);
        let g = rasterize('▘', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        assert_eq!(g.data[0], 255, "upper-left should be filled");
        // Other quadrants should be empty at their centers.
        assert_eq!(g.data[(h - 1) * w], 0, "lower-left should be empty");
        assert_eq!(g.data[w - 1], 0, "upper-right should be empty");
        assert_eq!(g.data[(h - 1) * w + (w - 1)], 0, "lower-right empty");
    }

    #[test]
    fn shade_produces_constant_alpha() {
        let m = metrics(8, 16);
        let g = rasterize('▒', &m).unwrap();
        // Medium shade = 50%. Allow ±2 for premultiplication rounding.
        assert!(g.data.iter().all(|&p| (124..=132).contains(&p)));
    }

    #[test]
    fn unsupported_codepoint_returns_none() {
        let m = metrics(8, 16);
        assert!(rasterize('a', &m).is_none());
        // Dashed variants live in U+2504..U+250B; not in the registry yet.
        assert!(rasterize('\u{2504}', &m).is_none());
        // Powerline PUA codepoints adjacent to the supported set
        // (U+E0B0..=U+E0BF) — these neighbors are not registered.
        assert!(rasterize('\u{E0AF}', &m).is_none());
        assert!(rasterize('\u{E0C0}', &m).is_none());
    }

    #[test]
    fn supports_powerline_triangle_codepoints() {
        for cp in [0xE0B0u32, 0xE0B1, 0xE0B2, 0xE0B3] {
            let c = char::from_u32(cp).unwrap();
            assert!(supports(c), "expected procedural support for U+{cp:04X}");
        }
    }

    #[test]
    fn powerline_right_filled_paints_solid_left_edge() {
        // U+E0B0's defining feature: the left column of pixels carries
        // ink — that's the seam that has to meet the previous segment
        // flush. A font reporting bearing_x > 0 used to break this.
        // Skip the y = 0 and y = h-1 corner rows: those touch the
        // triangle at a single vertex, so anti-aliased coverage there
        // is platform-dependent and not the property we're testing.
        let m = metrics(12, 24);
        let g = rasterize('\u{E0B0}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        for row in 1..h - 1 {
            let alpha = g.data[row * w];
            assert!(
                alpha > 0,
                "expected ink at left edge row {row}, got {alpha}"
            );
        }
        // Apex sits at the right-mid edge.
        let mid = h / 2;
        assert!(g.data[mid * w + (w - 1)] > 0);
        // Deep right-side interior pixels (far from the hypotenuse) are
        // outside the triangle entirely — well-defined zero.
        assert_eq!(g.data[w - 2], 0, "(w-2, 0) is clearly outside");
        assert_eq!(
            g.data[(h - 1) * w + (w - 2)],
            0,
            "(w-2, h-1) is clearly outside"
        );
    }

    #[test]
    fn powerline_left_filled_mirrors_right_filled_geometry() {
        // At mid-height both variants fill the entire row (the apex of
        // one and the vertical edge of the other share that scanline),
        // so the mirror property has to be probed at a row where the
        // triangle is a sliver. At y = 1 with a 12×24 cell, RightFilled
        // occupies a left-edge sliver and LeftFilled occupies a right-
        // edge sliver — they're complements there.
        let m = metrics(12, 24);
        let right = rasterize('\u{E0B0}', &m).unwrap();
        let left = rasterize('\u{E0B2}', &m).unwrap();
        let w = right.width as usize;
        let h = right.height as usize;
        let mid = h / 2;

        // Apexes point opposite ways.
        assert!(
            right.data[mid * w + (w - 1)] > 0,
            "right-filled apex at right-mid edge"
        );
        assert!(left.data[mid * w] > 0, "left-filled apex at left-mid edge");

        // y = 1 sliver: left column is inside RightFilled, outside
        // LeftFilled.
        assert!(right.data[w] > 0, "right-filled left sliver at y=1");
        assert_eq!(left.data[w], 0, "left-filled has no left ink at y=1");

        // And mirrored: right column is inside LeftFilled, outside
        // RightFilled.
        assert!(
            left.data[w + (w - 1)] > 0,
            "left-filled right sliver at y=1"
        );
        assert_eq!(
            right.data[w + (w - 1)],
            0,
            "right-filled has no right ink at y=1"
        );
    }

    #[test]
    fn powerline_thin_variant_leaves_interior_empty() {
        // The thin variants draw only the hypotenuse. A pixel deep in
        // the lower-left interior (far from any stroke) must stay
        // unmarked; otherwise the triangle has been accidentally
        // filled.
        let m = metrics(16, 32);
        let g = rasterize('\u{E0B1}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        // Pixel (1, h-8) sits well inside the lower-left region —
        // distance from the hypotenuse y + x = h is ≈ 5 px, comfortably
        // outside the ~3 px stroke.
        assert_eq!(
            g.data[(h - 8) * w + 1],
            0,
            "thin variant should leave deep interior clear"
        );
    }

    #[test]
    fn powerline_rasterize_returns_full_cell_dimensions() {
        // bearing_x = 0 and size = cell are what make the sprite snap to
        // the cell boundary. Verify that the metrics flow through.
        let m = metrics(10, 20);
        let g = rasterize('\u{E0B0}', &m).unwrap();
        assert_eq!(g.width, 10);
        assert_eq!(g.height, 20);
        assert_eq!(g.bearing_x, 0);
        assert_eq!(g.bearing_y, 16);
        assert_eq!(g.format, GlyphFormat::Alpha);
    }

    #[test]
    fn supports_extended_powerline_separators() {
        for cp in 0xE0B4u32..=0xE0BF {
            let c = char::from_u32(cp).unwrap();
            assert!(supports(c), "expected procedural support for U+{cp:04X}");
        }
    }

    #[test]
    fn semicircle_right_filled_inks_flat_edge_and_apex() {
        // U+E0B4: the flat diameter sits on the left edge (the seam that has
        // to meet the previous segment), the curve bulges to the right-mid
        // edge. Skip the corner rows where the diameter degenerates to a
        // vertex and anti-aliased coverage is platform-dependent.
        let m = metrics(16, 32);
        let g = rasterize('\u{E0B4}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        let mid = h / 2;
        for row in 1..h - 1 {
            assert!(g.data[row * w] > 0, "expected flat-edge ink at row {row}");
        }
        assert!(g.data[mid * w + (w - 1)] > 0, "apex at right-mid edge");
        // Top-right corner lies outside the half-disc.
        assert_eq!(g.data[w - 1], 0, "top-right corner outside the disc");
    }

    #[test]
    fn semicircle_left_filled_mirrors_right() {
        let m = metrics(16, 32);
        let g = rasterize('\u{E0B6}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        let mid = h / 2;
        // Diameter on the right edge, apex on the left-mid edge.
        for row in 1..h - 1 {
            assert!(
                g.data[row * w + (w - 1)] > 0,
                "expected flat-edge ink at row {row}"
            );
        }
        assert!(g.data[mid * w] > 0, "apex at left-mid edge");
        assert_eq!(g.data[0], 0, "top-left corner outside the disc");
    }

    #[test]
    fn semicircle_thin_leaves_interior_empty() {
        // U+E0B5 strokes only the arc; a point well inside the disc and away
        // from both the arc and the (unstroked) flat edge stays clear.
        let m = metrics(16, 32);
        let g = rasterize('\u{E0B5}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        let mid = h / 2;
        assert_eq!(
            g.data[mid * w + w / 4],
            0,
            "thin arc must leave the interior clear"
        );
        // The arc itself reaches the right-mid edge.
        assert!(g.data[mid * w + (w - 1)] > 0, "thin arc present at apex");
    }

    #[test]
    fn corner_triangle_lower_left_fills_below_main_diagonal() {
        let m = metrics(16, 16);
        let g = rasterize('\u{E0B8}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        assert_eq!(g.data[(h - 1) * w], 255, "bottom-left corner filled");
        assert_eq!(g.data[w - 1], 0, "top-right corner empty");
    }

    #[test]
    fn corner_triangle_upper_right_mirrors_lower_left() {
        let m = metrics(16, 16);
        let g = rasterize('\u{E0BE}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        assert_eq!(g.data[w - 1], 255, "top-right corner filled");
        assert_eq!(g.data[(h - 1) * w], 0, "bottom-left corner empty");
    }

    #[test]
    fn corner_triangle_thin_is_diagonal_only() {
        // U+E0B9 is the lower-left wedge's hypotenuse — the main diagonal on
        // its own. The bulk of the wedge stays unfilled, but the diagonal
        // crosses the cell center.
        let m = metrics(16, 16);
        let g = rasterize('\u{E0B9}', &m).unwrap();
        let w = g.width as usize;
        let h = g.height as usize;
        assert_eq!(g.data[(h - 1) * w], 0, "thin wedge leaves the corner empty");
        let mid = h / 2;
        let band = [w / 2 - 1, w / 2, w / 2 + 1];
        assert!(
            band.iter().any(|&c| g.data[mid * w + c] > 0),
            "diagonal stroke crosses the cell center"
        );
    }
}
