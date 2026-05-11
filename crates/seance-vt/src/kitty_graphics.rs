use libghostty_vt::RenderState;
use libghostty_vt::Terminal as VtTerminal;
use libghostty_vt::kitty::graphics as kg;
use libghostty_vt::render::{CellIteration, CellIterator, RowIterator};
use libghostty_vt::style::{self, PaletteIndex};

use crate::frame::{
    ImageId, ImageInfo, ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
use crate::snapshot::SnapshotImage;

#[derive(Default)]
pub(crate) struct KittyGraphicsSnapshot {
    pub(crate) placements: Vec<PlacementSnapshot>,
    pub(crate) images: Vec<SnapshotImage>,
}

pub(crate) fn extract_kitty_graphics(
    vt: &mut VtTerminal<'static, 'static>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> KittyGraphicsSnapshot {
    let mut out = KittyGraphicsSnapshot::default();

    let mut placement_collector = SnapshotPlacementCollector::default();
    for layer in [
        PlacementLayer::BelowBg,
        PlacementLayer::BelowText,
        PlacementLayer::AboveText,
    ] {
        let _ = walk_placements(vt, layer, &mut placement_collector);
        let _ = walk_virtual_placements(
            vt,
            cell_width_px,
            cell_height_px,
            layer,
            &mut placement_collector,
        );
    }
    out.placements = placement_collector.placements;

    let mut image_collector = SnapshotImageCollector::default();
    let _ = walk_images(vt, &mut image_collector);
    out.images = image_collector.images;

    out
}

pub(crate) fn is_placeholder_text(text: &str) -> bool {
    text.starts_with('\u{10EEEE}')
}

/// The unicode placeholder base codepoint. Any cell whose first grapheme
/// codepoint matches this is a Kitty virtual-placement placeholder.
const PLACEHOLDER_CP: u32 = 0x10EEEE;

/// Return the 0-based index of `cp` in the Kitty row/col diacritic
/// alphabet, or `None` if `cp` is not a valid diacritic.
fn diacritic_index(cp: u32) -> Option<u32> {
    DIACRITICS.binary_search(&cp).ok().map(|i| i as u32)
}

/// Sorted diacritic codepoints used by the Kitty graphics protocol to
/// encode row/column/image-id-high values.
///
/// Derived verbatim from:
///   <https://sw.kovidgoyal.net/kitty/_downloads/f0a0de9ec8d9ff4456206db8e0814937/rowcolumn-diacritics.txt>
///
/// The index in this array is the encoded value.
const DIACRITICS: &[u32] = &[
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186,
    0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

#[derive(Default)]
struct SnapshotPlacementCollector {
    placements: Vec<PlacementSnapshot>,
}

impl PlacementVisitor for SnapshotPlacementCollector {
    fn placement(&mut self, p: &PlacementSnapshot) {
        self.placements.push(*p);
    }
}

#[derive(Default)]
struct SnapshotImageCollector {
    images: Vec<SnapshotImage>,
}

impl ImageVisitor for SnapshotImageCollector {
    fn image(&mut self, info: &ImageInfo<'_>) {
        self.images.push(SnapshotImage {
            image_id: info.image_id,
            width: info.width,
            height: info.height,
            rgba: info.rgba.to_vec(),
        });
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
    vt: &VtTerminal<'static, 'static>,
    layer: PlacementLayer,
    visitor: &mut dyn PlacementVisitor,
) -> Option<()> {
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
        let Ok(Some(vpos)) = p.viewport_pos(&image, vt) else {
            continue;
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
            image_id: ImageId::from(image_id),
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
        tracing::debug!("walk_placements layer={layer:?} emitted={emitted}");
    }
    Some(())
}

/// Walks all placements (all layers) to collect unique `image_id`s and
/// emits their pixel payloads as RGBA8. The renderer caches by `image_id`;
/// emitting every frame is fine because the cache dedupes uploads.
fn walk_images(vt: &VtTerminal<'static, 'static>, visitor: &mut dyn ImageVisitor) -> Option<()> {
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

        let Some(rgba) = expand_to_rgba(format, width, height, data, &mut scratch) else {
            continue;
        };
        tracing::debug!(
            "walk_images uploading id={image_id} {width}x{height} format={format:?} bytes={}",
            rgba.len()
        );
        visitor.image(&ImageInfo {
            image_id: ImageId::from(image_id),
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
/// Pre-condition: `cell_w` and `cell_h` must be non-zero; virtual placements
/// can't be sized before the first resize.
fn walk_virtual_placements(
    vt: &mut VtTerminal<'static, 'static>,
    cell_w: u32,
    cell_h: u32,
    layer: PlacementLayer,
    visitor: &mut dyn PlacementVisitor,
) -> Option<()> {
    if cell_w == 0 || cell_h == 0 {
        return Some(());
    }

    // Phase 1: collect per-image metadata from transmitted virtual placements.
    // Kept in a tiny Vec; one entry per unique image referenced this frame.
    let infos: Vec<(u32, VirtualPlacementInfo)> = {
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
    tracing::debug!(
        "walk_virtual_placements layer={layer:?} virtual_infos={}",
        infos.len()
    );

    // Phase 2: walk the grid looking for placeholder cells.
    let mut render_state = RenderState::new().ok()?;
    let snapshot = render_state.update(vt).ok()?;
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
        tracing::debug!(
            "walk_virtual_placements layer={layer:?} placeholder_cells={placeholder_cells} \
             cell_px={cell_w}x{cell_h}"
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
        image_id: ImageId::from(run.image_id),
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
    layer.contains_z(z)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_is_sorted() {
        for pair in DIACRITICS.windows(2) {
            assert!(pair[0] < pair[1], "not sorted at {:#x}", pair[0]);
        }
    }

    #[test]
    fn alphabet_has_297_entries() {
        assert_eq!(DIACRITICS.len(), 297);
    }

    #[test]
    fn placeholder_text_detects_base_codepoint() {
        assert!(is_placeholder_text("\u{10EEEE}"));
        assert!(is_placeholder_text("\u{10EEEE}\u{0305}"));
        assert!(!is_placeholder_text(""));
        assert!(!is_placeholder_text("x\u{10EEEE}"));
    }

    #[test]
    fn spot_checks_match_upstream() {
        assert_eq!(diacritic_index(0x0305), Some(0));
        assert_eq!(diacritic_index(0x0483), Some(30));
        assert_eq!(diacritic_index(0x1D244), Some(296));
        assert_eq!(diacritic_index(0x0000), None);
    }
}
