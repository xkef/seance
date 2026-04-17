use cosmic_text::{Attrs, Buffer, CacheKeyFlags, Family, Metrics, Shaping};

use libghostty_vt::RenderState;
use libghostty_vt::Terminal as VtTerminal;
use libghostty_vt::render::{CellIterator, RowIterator};

use super::cache::GlyphCache;
use crate::theme::Theme;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CellText {
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub grid_pos: [u16; 2],
    pub color: [u8; 4],
    pub atlas_and_flags: u32,
}

const _: () = assert!(size_of::<CellText>() == 32);

pub struct FrameInfo {
    pub cell_width: f32,
    pub cell_height: f32,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub grid_padding: [f32; 4],
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

pub struct CellBuilder {
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    last_frame: Option<FrameInfo>,
}

impl CellBuilder {
    pub fn new() -> Self {
        Self {
            bg_cells: Vec::new(),
            text_cells: Vec::new(),
            last_frame: None,
        }
    }

    pub fn build_frame(
        &mut self,
        vt: &mut VtTerminal<'_, '_>,
        cache: &mut GlyphCache,
        surface_width: u32,
        surface_height: u32,
        theme: &Theme,
    ) -> bool {
        let Some(frame) = self.build_frame_inner(vt, cache, surface_width, surface_height, theme)
        else {
            eprintln!("build_frame_inner returned None");
            return false;
        };
        self.last_frame = Some(frame);
        true
    }

    fn build_frame_inner(
        &mut self,
        vt: &mut VtTerminal<'_, '_>,
        cache: &mut GlyphCache,
        surface_width: u32,
        surface_height: u32,
        theme: &Theme,
    ) -> Option<FrameInfo> {
        let mut render_state = RenderState::new().ok()?;
        let snapshot = render_state.update(vt).ok()?;

        let cols = vt.cols().unwrap_or(80);
        let rows = vt.rows().unwrap_or(24);

        let metrics = cache.font.metrics();
        let cw = metrics.cell_width;
        let ch = metrics.cell_height;

        let total_w = cols as f32 * cw;
        let total_h = rows as f32 * ch;
        let pad_x = ((surface_width as f32 - total_w) / 2.0).max(0.0);
        let pad_y = ((surface_height as f32 - total_h) / 2.0).max(0.0);

        self.bg_cells.clear();
        self.bg_cells
            .resize((cols as usize) * (rows as usize), theme.bg);
        self.text_cells.clear();

        let font_size = cache.font.font_size();
        let scale = cache.font.scale() as f32;
        let scaled_size = font_size * scale;
        let cosmic_metrics = Metrics::new(scaled_size, ch);
        let family = Family::Name("JetBrainsMono Nerd Font");
        let attrs = Attrs::new().family(family);

        let mut row_iter_obj = RowIterator::new().ok()?;
        let mut cell_iter_obj = CellIterator::new().ok()?;
        let mut row_iter = row_iter_obj.update(&snapshot).ok()?;
        let mut row_idx: u16 = 0;

        while let Some(row) = row_iter.next() {
            if row_idx >= rows {
                break;
            }

            let mut cell_iter = cell_iter_obj.update(row).ok()?;
            let mut col_idx: u16 = 0;

            while let Some(cell) = cell_iter.next() {
                if col_idx >= cols {
                    break;
                }

                let cell_index = row_idx as usize * cols as usize + col_idx as usize;

                // Background color
                if let Ok(Some(bg)) = cell.bg_color() {
                    self.bg_cells[cell_index] = [bg.r, bg.g, bg.b, 255];
                } else if let Ok(style) = cell.style()
                    && let Some(rgb) = theme.resolve_color(&style.bg_color)
                {
                    self.bg_cells[cell_index] = [rgb[0], rgb[1], rgb[2], 255];
                }

                // Foreground / text
                let graphemes = cell.graphemes().ok()?;
                if !graphemes.is_empty() {
                    let fg = if let Ok(Some(fg)) = cell.fg_color() {
                        [fg.r, fg.g, fg.b, 255]
                    } else if let Ok(style) = cell.style() {
                        theme
                            .resolve_color(&style.fg_color)
                            .map(|c| [c[0], c[1], c[2], 255])
                            .unwrap_or([theme.fg[0], theme.fg[1], theme.fg[2], 255])
                    } else {
                        [theme.fg[0], theme.fg[1], theme.fg[2], 255]
                    };

                    let text: String = graphemes.into_iter().collect();

                    let mut buffer = Buffer::new(&mut cache.font.inner, cosmic_metrics);
                    buffer.set_text(
                        &mut cache.font.inner,
                        &text,
                        &attrs,
                        Shaping::Advanced,
                        None,
                    );
                    buffer.shape_until_scroll(&mut cache.font.inner, false);

                    for run in buffer.layout_runs() {
                        for glyph in run.glyphs.iter() {
                            let (key, _, _) = cosmic_text::CacheKey::new(
                                glyph.font_id,
                                glyph.glyph_id,
                                scaled_size,
                                (0.0, 0.0),
                                cosmic_text::fontdb::Weight::NORMAL,
                                CacheKeyFlags::empty(),
                            );

                            if let Some(entry) = cache.get_or_insert(key) {
                                let atlas_val = if entry.is_color { 1u32 } else { 0u32 };
                                self.text_cells.push(CellText {
                                    glyph_pos: entry.pos,
                                    glyph_size: entry.size,
                                    bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
                                    grid_pos: [col_idx, row_idx],
                                    color: fg,
                                    atlas_and_flags: atlas_val,
                                });
                            }
                        }
                    }
                }

                col_idx += 1;
            }

            row_idx += 1;
        }

        cache.atlas.clear_dirty();

        Some(FrameInfo {
            cell_width: cw,
            cell_height: ch,
            grid_cols: cols,
            grid_rows: rows,
            grid_padding: [pad_x, pad_y, pad_x, pad_y],
            bg_color: theme.bg,
            min_contrast: 1.0,
            cursor_pos: [0, 0],
            cursor_color: theme.cursor,
            cursor_wide: false,
        })
    }

    pub fn bg_cells(&self) -> &[[u8; 4]] {
        &self.bg_cells
    }

    pub fn text_cells(&self) -> &[CellText] {
        &self.text_cells
    }

    pub fn last_frame(&self) -> Option<&FrameInfo> {
        self.last_frame.as_ref()
    }
}
