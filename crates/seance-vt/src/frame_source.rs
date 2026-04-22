//! libghostty-vt implementation of [`FrameSource`].
//!
//! The only place that touches libghostty-vt's
//! `RenderState` / `RowIterator` / `CellIterator` dance.

use libghostty_vt::RenderState;
use libghostty_vt::kitty::graphics as kg;
use libghostty_vt::render::{CellIteration, CellIterator, CursorVisualStyle, RowIterator};
use libghostty_vt::style::{self, PaletteIndex, RgbColor};

use crate::frame::{
    CellAttrs, CellColor, CellView, CellVisitor, CursorInfo, CursorShape, FrameSource, ImageInfo,
    ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
use crate::kitty_placeholder::{PLACEHOLDER_CP, diacritic_index};
use crate::selection::GridPos;
use crate::terminal::Terminal;

pub struct LibGhosttyFrameSource<'a> {
    term: &'a mut Terminal,
}

impl<'a> LibGhosttyFrameSource<'a> {
    pub fn new(term: &'a mut Terminal) -> Self {
        Self { term }
    }
}

impl FrameSource for LibGhosttyFrameSource<'_> {
    fn grid_size(&mut self) -> (u16, u16) {
        let vt = self.term.vt_mut();
        (vt.cols().unwrap_or(80), vt.rows().unwrap_or(24))
    }

    fn cursor(&mut self) -> CursorInfo {
        let mut render_state = match RenderState::new() {
            Ok(state) => state,
            Err(_) => return CursorInfo::default(),
        };
        let snapshot = match render_state.update(self.term.vt_mut()) {
            Ok(snapshot) => snapshot,
            Err(_) => return CursorInfo::default(),
        };
        let visible = snapshot.cursor_visible().unwrap_or(true);
        let pos = snapshot
            .cursor_viewport()
            .ok()
            .flatten()
            .map_or(GridPos::default(), |vp| GridPos {
                col: vp.x,
                row: vp.y,
            });
        let shape = snapshot
            .cursor_visual_style()
            .ok()
            .and_then(map_cursor_shape);
        CursorInfo {
            pos,
            visible,
            wide: false,
            shape,
        }
    }

    fn selection(&mut self) -> Option<(GridPos, GridPos)> {
        self.term.selection_range()
    }

    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor) {
        let _ = walk(self.term, visitor);
    }

    fn visit_placements(&mut self, layer: PlacementLayer, visitor: &mut dyn PlacementVisitor) {
        let _ = walk_placements(self.term, layer, visitor);
        let _ = walk_virtual_placements(self.term, layer, visitor);
    }

    fn visit_images(&mut self, visitor: &mut dyn ImageVisitor) {
        let _ = walk_images(self.term, visitor);
    }
}

/// Walks the VT snapshot, invoking `visitor` on each cell.
/// Returns `None` if any libghostty-vt call fails (whole frame is dropped).
fn walk(term: &mut Terminal, visitor: &mut dyn CellVisitor) -> Option<()> {
    let mut render_state = RenderState::new().ok()?;
    let snapshot = render_state.update(term.vt_mut()).ok()?;
    let mut rows = RowIterator::new().ok()?;
    let mut cells = CellIterator::new().ok()?;
    let mut row_iter = rows.update(&snapshot).ok()?;

    let mut scratch = String::with_capacity(4);
    let mut row_idx: u16 = 0;
    while let Some(row) = row_iter.next() {
        let mut cell_iter = cells.update(row).ok()?;
        let mut col_idx: u16 = 0;
        while let Some(cell) = cell_iter.next() {
            scratch.clear();
            if let Ok(graphs) = cell.graphemes() {
                scratch.extend(graphs);
            }
            // Kitty virtual placeholders render as the image pass; emitting
            // the placeholder char + diacritics would draw tofu over the image.
            if scratch.starts_with('\u{10EEEE}') {
                scratch.clear();
            }
            let style = cell.style().ok();
            visitor.cell(
                row_idx,
                col_idx,
                CellView {
                    text: &scratch,
                    fg: resolve_fg(cell, style.as_ref()),
                    bg: resolve_bg(cell, style.as_ref()),
                    attrs: cell_attrs(style.as_ref()),
                },
            );
            col_idx += 1;
        }
        row_idx += 1;
    }
    Some(())
}

fn resolve_bg(cell: &CellIteration<'_, '_>, style: Option<&style::Style>) -> CellColor {
    if let Ok(Some(rgb)) = cell.bg_color() {
        return rgb_to_cell_color(rgb);
    }
    style.map_or(CellColor::Default, |style| {
        style_to_cell_color(&style.bg_color)
    })
}

fn resolve_fg(cell: &CellIteration<'_, '_>, style: Option<&style::Style>) -> CellColor {
    if let Ok(Some(rgb)) = cell.fg_color() {
        return rgb_to_cell_color(rgb);
    }
    style.map_or(CellColor::Default, |style| {
        style_to_cell_color(&style.fg_color)
    })
}

fn cell_attrs(style: Option<&style::Style>) -> CellAttrs {
    CellAttrs {
        bold: style.is_some_and(|style| style.bold),
        italic: style.is_some_and(|style| style.italic),
        faint: style.is_some_and(|style| style.faint),
        inverse: style.is_some_and(|style| style.inverse),
        invisible: style.is_some_and(|style| style.invisible),
    }
}

fn rgb_to_cell_color(c: RgbColor) -> CellColor {
    CellColor::Rgb(c.r, c.g, c.b)
}

fn style_to_cell_color(sc: &style::StyleColor) -> CellColor {
    match sc {
        style::StyleColor::None => CellColor::Default,
        style::StyleColor::Palette(PaletteIndex(idx)) => CellColor::Palette(*idx),
        style::StyleColor::Rgb(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

// `BlockHollow` is ghostty's unfocused-window rendering, not a DECSCUSR
// request — collapse into `Block`. Window-focus hollow cursors are a
// separate feature tracked elsewhere. Unknown future variants (the
// enum is `#[non_exhaustive]`) return `None` so the app falls back to
// the user's configured shape.
fn map_cursor_shape(s: CursorVisualStyle) -> Option<CursorShape> {
    match s {
        CursorVisualStyle::Bar => Some(CursorShape::Bar),
        CursorVisualStyle::Block | CursorVisualStyle::BlockHollow => Some(CursorShape::Block),
        CursorVisualStyle::Underline => Some(CursorShape::Underline),
        _ => None,
    }
}

fn layer_to_kg(layer: PlacementLayer) -> kg::Layer {
    match layer {
        PlacementLayer::BelowBg => kg::Layer::BelowBg,
        PlacementLayer::BelowText => kg::Layer::BelowText,
        PlacementLayer::AboveText => kg::Layer::AboveText,
    }
}

/// Walks the kitty placement iterator for a single z-layer. Skips virtual
/// (unicode placeholder) placements and placements entirely off-screen.
/// Returns `None` if any libghostty call fails; the layer is silently
/// dropped from the frame in that case.
fn walk_placements(
    term: &Terminal,
    layer: PlacementLayer,
    visitor: &mut dyn PlacementVisitor,
) -> Option<()> {
    let vt = term.vt();
    let graphics = vt.kitty_graphics().ok()?;
    let mut iter = kg::PlacementIterator::new().ok()?;
    let mut placements = iter.update(&graphics).ok()?;
    placements.set_layer(layer_to_kg(layer)).ok()?;

    let mut emitted = 0u32;
    while let Some(p) = placements.next() {
        if p.is_virtual().unwrap_or(true) {
            continue;
        }
        let Ok(image_id) = p.image_id() else { continue };
        let Some(image) = graphics.image(image_id) else {
            continue;
        };
        let vpos = match p.viewport_pos(&image, vt) {
            Ok(Some(vp)) => vp,
            _ => continue,
        };
        let Ok(pxs) = p.pixel_size(&image, vt) else {
            continue;
        };
        let Ok(src) = p.source_rect(&image) else {
            continue;
        };
        let Ok(placement_id) = p.placement_id() else {
            continue;
        };
        let Ok(z) = p.z() else { continue };
        let Ok(image_width) = image.width() else {
            continue;
        };
        let Ok(image_height) = image.height() else {
            continue;
        };
        visitor.placement(&PlacementSnapshot {
            image_id,
            placement_id,
            viewport_col: vpos.col,
            viewport_row: vpos.row,
            pixel_width: pxs.width,
            pixel_height: pxs.height,
            source_x: src.x,
            source_y: src.y,
            source_width: src.width,
            source_height: src.height,
            image_width,
            image_height,
            z,
        });
        emitted += 1;
    }
    if emitted > 0 {
        log::debug!("walk_placements layer={:?} emitted={}", layer, emitted);
    }
    Some(())
}

/// Walks all placements (all layers) to collect unique `image_id`s and
/// emits their pixel payloads as RGBA8. The renderer caches by `image_id`;
/// emitting every frame is fine because the cache dedupes uploads.
fn walk_images(term: &Terminal, visitor: &mut dyn ImageVisitor) -> Option<()> {
    let vt = term.vt();
    let graphics = vt.kitty_graphics().ok()?;
    let mut iter = kg::PlacementIterator::new().ok()?;
    let mut placements = iter.update(&graphics).ok()?;

    // Small inline set: image_ids are u32; ordering doesn't matter and
    // per-frame counts are tiny. Linear scan beats a HashSet here.
    let mut seen: Vec<u32> = Vec::new();
    let mut scratch: Vec<u8> = Vec::new();

    while let Some(p) = placements.next() {
        // Virtual placements ARE included: the ghostty storage keeps the
        // image pixel payload in the virtual transmit record, and the
        // placeholder cells reference it by id.
        let Ok(image_id) = p.image_id() else { continue };
        if seen.contains(&image_id) {
            continue;
        }
        seen.push(image_id);

        let Some(image) = graphics.image(image_id) else {
            continue;
        };
        let Ok(width) = image.width() else { continue };
        let Ok(height) = image.height() else { continue };
        let Ok(format) = image.format() else { continue };
        let Ok(data) = image.data() else { continue };

        let rgba = match expand_to_rgba(format, width, height, data, &mut scratch) {
            Some(slice) => slice,
            None => continue,
        };
        log::debug!(
            "walk_images uploading id={} {}x{} format={:?} bytes={}",
            image_id,
            width,
            height,
            format,
            rgba.len()
        );
        visitor.image(&ImageInfo {
            image_id,
            width,
            height,
            rgba,
        });
    }
    Some(())
}

/// Expand non-RGBA formats to tight 8-bit RGBA. Returns a slice into either
/// the input `data` (RGBA passthrough) or `scratch` (expanded). Returns
/// `None` when the input length doesn't match the declared dimensions.
fn expand_to_rgba<'a>(
    format: kg::ImageFormat,
    width: u32,
    height: u32,
    data: &'a [u8],
    scratch: &'a mut Vec<u8>,
) -> Option<&'a [u8]> {
    let pixels = usize::try_from(width).ok()? * usize::try_from(height).ok()?;
    match format {
        kg::ImageFormat::Rgba | kg::ImageFormat::Png => {
            // Ghostty stores PNG as decoded RGBA (our decoder emits RGBA).
            if data.len() == pixels * 4 {
                Some(data)
            } else {
                None
            }
        }
        kg::ImageFormat::Rgb => {
            if data.len() != pixels * 3 {
                return None;
            }
            scratch.clear();
            scratch.reserve(pixels * 4);
            for px in data.chunks_exact(3) {
                scratch.extend_from_slice(px);
                scratch.push(0xff);
            }
            Some(scratch.as_slice())
        }
        kg::ImageFormat::Gray => {
            if data.len() != pixels {
                return None;
            }
            scratch.clear();
            scratch.reserve(pixels * 4);
            for &g in data {
                scratch.extend_from_slice(&[g, g, g, 0xff]);
            }
            Some(scratch.as_slice())
        }
        kg::ImageFormat::GrayAlpha => {
            if data.len() != pixels * 2 {
                return None;
            }
            scratch.clear();
            scratch.reserve(pixels * 4);
            for ga in data.chunks_exact(2) {
                scratch.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
            Some(scratch.as_slice())
        }
        _ => None,
    }
}

/// One transmitted virtual placement: the image's declared grid footprint
/// and z. Keyed by `image_id` during the grid walk.
#[derive(Copy, Clone)]
struct VirtualPlacementInfo {
    grid_cols: u32,
    grid_rows: u32,
    image_width: u32,
    image_height: u32,
    z: i32,
}

/// Walk the active screen for Kitty unicode placeholder cells, group
/// consecutive same-image cells on the same row into runs, and emit one
/// `PlacementSnapshot` per run that belongs to `layer`.
///
/// Pre-condition: [`Terminal::cell_pixels`] must return non-zero; virtual
/// placements can't be sized before the first resize.
fn walk_virtual_placements(
    term: &mut Terminal,
    layer: PlacementLayer,
    visitor: &mut dyn PlacementVisitor,
) -> Option<()> {
    let (cell_w, cell_h) = term.cell_pixels();
    if cell_w == 0 || cell_h == 0 {
        return None;
    }

    // Phase 1: collect per-image metadata from transmitted virtual placements.
    // Kept in a tiny Vec; one entry per unique image referenced this frame.
    let infos: Vec<(u32, VirtualPlacementInfo)> = {
        let vt = term.vt();
        let graphics = vt.kitty_graphics().ok()?;
        let mut iter = kg::PlacementIterator::new().ok()?;
        let mut placements = iter.update(&graphics).ok()?;
        let mut out: Vec<(u32, VirtualPlacementInfo)> = Vec::new();
        while let Some(p) = placements.next() {
            if !p.is_virtual().unwrap_or(false) {
                continue;
            }
            let Ok(image_id) = p.image_id() else { continue };
            let Ok(grid_cols) = p.columns() else { continue };
            let Ok(grid_rows) = p.rows() else { continue };
            let Ok(z) = p.z() else { continue };
            let Some(image) = graphics.image(image_id) else {
                continue;
            };
            let Ok(image_width) = image.width() else {
                continue;
            };
            let Ok(image_height) = image.height() else {
                continue;
            };
            // If the transmit didn't specify rows/cols explicitly, libghostty
            // returns 0. Fall back to one-cell-per-image-pixel, though in
            // practice chafa/timg always set C= and r=.
            let grid_cols = if grid_cols > 0 { grid_cols } else { 1 };
            let grid_rows = if grid_rows > 0 { grid_rows } else { 1 };
            out.push((
                image_id,
                VirtualPlacementInfo {
                    grid_cols,
                    grid_rows,
                    image_width,
                    image_height,
                    z,
                },
            ));
        }
        out
    };
    if infos.is_empty() {
        return Some(());
    }
    log::debug!(
        "walk_virtual_placements layer={:?} virtual_infos={}",
        layer,
        infos.len()
    );

    // Phase 2: walk the grid looking for placeholder cells.
    let mut render_state = RenderState::new().ok()?;
    let snapshot = render_state.update(term.vt_mut()).ok()?;
    let mut rows = RowIterator::new().ok()?;
    let mut cells = CellIterator::new().ok()?;
    let mut row_iter = rows.update(&snapshot).ok()?;

    let mut run: Option<PlaceholderRun> = None;
    let mut screen_row: u32 = 0;
    let mut placeholder_cells = 0u32;
    while let Some(row) = row_iter.next() {
        let mut cell_iter = cells.update(row).ok()?;
        let mut screen_col: u32 = 0;
        while let Some(cell) = cell_iter.next() {
            match decode_placeholder(cell) {
                Some(decoded) => {
                    placeholder_cells += 1;
                    let appended = run
                        .as_mut()
                        .and_then(|r| r.append(&decoded, screen_col))
                        .is_some();
                    if !appended {
                        if let Some(prev) = run.take() {
                            emit_virtual_run(&prev, cell_w, cell_h, &infos, layer, visitor);
                        }
                        run = Some(PlaceholderRun::new(&decoded, screen_row, screen_col));
                    }
                }
                None => {
                    if let Some(prev) = run.take() {
                        emit_virtual_run(&prev, cell_w, cell_h, &infos, layer, visitor);
                    }
                }
            }
            screen_col += 1;
        }
        // Runs never cross rows in the Kitty spec.
        if let Some(prev) = run.take() {
            emit_virtual_run(&prev, cell_w, cell_h, &infos, layer, visitor);
        }
        screen_row += 1;
    }
    if placeholder_cells > 0 {
        log::debug!(
            "walk_virtual_placements layer={:?} placeholder_cells={} cell_px={}x{}",
            layer,
            placeholder_cells,
            cell_w,
            cell_h
        );
    }
    Some(())
}

/// Decoded payload of one placeholder cell. `vp_row` / `vp_col` default
/// to 0 when diacritics are missing (matches Kitty semantics).
struct DecodedPlaceholder {
    image_id_low: u32,
    image_id_high: Option<u8>,
    placement_id: u32,
    vp_row: u32,
    vp_col: u32,
}

impl DecodedPlaceholder {
    fn full_image_id(&self) -> u32 {
        let high = self.image_id_high.unwrap_or(0) as u32;
        self.image_id_low | (high << 24)
    }
}

/// In-progress run of adjacent same-image placeholder cells on one row.
struct PlaceholderRun {
    image_id: u32,
    placement_id: u32,
    screen_row: u32,
    screen_col_start: u32,
    vp_row: u32,
    vp_col_start: u32,
    width: u32,
}

impl PlaceholderRun {
    fn new(d: &DecodedPlaceholder, screen_row: u32, screen_col: u32) -> Self {
        Self {
            image_id: d.full_image_id(),
            placement_id: d.placement_id,
            screen_row,
            screen_col_start: screen_col,
            vp_row: d.vp_row,
            vp_col_start: d.vp_col,
            width: 1,
        }
    }

    /// If `next` continues the run (same image + placement, consecutive
    /// screen column, and consecutive vp_col on the same vp_row), extend
    /// in place and return `Some(())`. Otherwise leave unchanged.
    fn append(&mut self, next: &DecodedPlaceholder, screen_col: u32) -> Option<()> {
        if self.image_id != next.full_image_id()
            || self.placement_id != next.placement_id
            || self.vp_row != next.vp_row
            || self.screen_col_start + self.width != screen_col
            || self.vp_col_start + self.width != next.vp_col
        {
            return None;
        }
        self.width += 1;
        Some(())
    }
}

fn emit_virtual_run(
    run: &PlaceholderRun,
    cell_w: u32,
    cell_h: u32,
    infos: &[(u32, VirtualPlacementInfo)],
    layer: PlacementLayer,
    visitor: &mut dyn PlacementVisitor,
) {
    let Some(info) = infos
        .iter()
        .find(|(id, _)| *id == run.image_id)
        .map(|(_, i)| *i)
    else {
        return;
    };
    if !layer_matches(layer, info.z) {
        return;
    }
    // Naive pixel slicing: image_width / grid_cols pixels per cell. Ignores
    // aspect-ratio centering that ghostty's RenderPlacement does; for
    // chafa/timg-generated grids the image already fills the grid so this
    // is accurate.
    let px_per_col = (info.image_width as f32) / (info.grid_cols as f32);
    let px_per_row = (info.image_height as f32) / (info.grid_rows as f32);
    let source_x = (run.vp_col_start as f32 * px_per_col).round() as u32;
    let source_y = (run.vp_row as f32 * px_per_row).round() as u32;
    let source_w = (run.width as f32 * px_per_col).round() as u32;
    let source_h = px_per_row.round() as u32;

    visitor.placement(&PlacementSnapshot {
        image_id: run.image_id,
        placement_id: run.placement_id,
        viewport_col: run.screen_col_start as i32,
        viewport_row: run.screen_row as i32,
        pixel_width: run.width * cell_w,
        pixel_height: cell_h,
        source_x,
        source_y,
        source_width: source_w,
        source_height: source_h,
        image_width: info.image_width,
        image_height: info.image_height,
        z: info.z,
    });
}

fn layer_matches(layer: PlacementLayer, z: i32) -> bool {
    match layer {
        PlacementLayer::BelowBg => z < i32::MIN / 2,
        PlacementLayer::BelowText => (i32::MIN / 2..0).contains(&z),
        PlacementLayer::AboveText => z >= 0,
    }
}

/// Decode a single cell as a placeholder. Returns `None` if the cell's
/// first grapheme codepoint is not `U+10EEEE`. Missing diacritics yield
/// `vp_row=0` / `vp_col=0` per Kitty semantics.
fn decode_placeholder(cell: &CellIteration<'_, '_>) -> Option<DecodedPlaceholder> {
    let graphemes = cell.graphemes().ok()?;
    let mut it = graphemes.iter();
    let first = *it.next()?;
    if first as u32 != PLACEHOLDER_CP {
        return None;
    }

    let vp_row = it
        .next()
        .and_then(|c| diacritic_index(*c as u32))
        .unwrap_or(0);
    let vp_col = it
        .next()
        .and_then(|c| diacritic_index(*c as u32))
        .unwrap_or(0);
    let image_id_high = it
        .next()
        .and_then(|c| diacritic_index(*c as u32))
        .and_then(|v| u8::try_from(v).ok());

    // Low 24 bits of the image ID come from the foreground color. Truecolor
    // packs `R<<16 | G<<8 | B`; palette indices map directly to the low u24.
    let image_id_low = cell_fg_to_id24(cell);

    // Placement ID (optional) is encoded in underline color. libghostty-vt's
    // Rust wrapper doesn't surface underline color yet; treat as 0, which
    // matches chafa/timg (they don't emit placement IDs).
    let placement_id = 0;

    Some(DecodedPlaceholder {
        image_id_low,
        image_id_high,
        placement_id,
        vp_row,
        vp_col,
    })
}

/// Extract the 24-bit image-ID value encoded in a cell's foreground color.
/// Mirrors ghostty's `colorToId` (graphics_unicode.zig): must branch on the
/// style-color variant, because `CellIteration::fg_color` flattens palette
/// indices through the palette — which would yield a totally different
/// 24-bit value from the encoded ID.
fn cell_fg_to_id24(cell: &CellIteration<'_, '_>) -> u32 {
    let Ok(style) = cell.style() else { return 0 };
    match &style.fg_color {
        style::StyleColor::None => 0,
        style::StyleColor::Palette(PaletteIndex(idx)) => *idx as u32,
        style::StyleColor::Rgb(rgb) => {
            let r = rgb.r as u32;
            let g = rgb.g as u32;
            let b = rgb.b as u32;
            (r << 16) | (g << 8) | b
        }
    }
}
