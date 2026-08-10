use etagere::{AtlasAllocator, Size as ESize};

const GRAYSCALE_SIZE: u32 = 2048;
const COLOR_SIZE: u32 = 1024;

/// A texel region of an atlas plane that changed since the last GPU upload.
/// Coordinates are in texels; `w`/`h` are the inserted glyph's bitmap extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// What an atlas plane needs uploaded to its GPU texture this frame.
///
/// `Full` is forced once after (re)allocation — there is no prior texture
/// content to layer onto. Steady-state frames that touched no new glyph
/// report `None`, so the renderer skips the upload entirely instead of
/// re-pushing the whole plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneUpload<'a> {
    None,
    Full,
    Rects(&'a [DirtyRect]),
}

pub struct GlyphAtlas {
    grayscale: AtlasPlane,
    color: AtlasPlane,
}

struct AtlasPlane {
    allocator: AtlasAllocator,
    data: Vec<u8>,
    size: u32,
    bpp: u32,
    /// Set on (re)allocation: the whole plane must be uploaded before any
    /// sub-rect upload is meaningful. Cleared by [`Self::clear_dirty`] once
    /// the full upload lands.
    full_dirty: bool,
    /// Glyph regions written since the last [`Self::clear_dirty`]. Empty in
    /// steady state. Not populated while `full_dirty` holds — the full
    /// upload already covers them.
    dirty_rects: Vec<DirtyRect>,
}

#[derive(Debug, Clone, Copy)]
pub struct AtlasEntry {
    pub pos: [u32; 2],
    pub size: [u32; 2],
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub is_color: bool,
}

impl GlyphAtlas {
    pub fn new() -> Self {
        Self {
            grayscale: AtlasPlane::new(GRAYSCALE_SIZE, 1),
            color: AtlasPlane::new(COLOR_SIZE, 4),
        }
    }

    pub fn insert(
        &mut self,
        bitmap: &[u8],
        width: u32,
        height: u32,
        bearing_x: i32,
        bearing_y: i32,
        is_color: bool,
    ) -> Option<AtlasEntry> {
        let plane = if is_color {
            &mut self.color
        } else {
            &mut self.grayscale
        };
        let alloc = plane
            .allocator
            .allocate(ESize::new(width as i32, height as i32))?;
        let pos = [alloc.rectangle.min.x as u32, alloc.rectangle.min.y as u32];
        plane.copy_bitmap(bitmap, pos, width, height);
        Some(AtlasEntry {
            pos,
            size: [width, height],
            bearing_x,
            bearing_y,
            is_color,
        })
    }

    pub fn grayscale_data(&self) -> (&[u8], u32) {
        (&self.grayscale.data, self.grayscale.size)
    }

    pub fn color_data(&self) -> (&[u8], u32) {
        (&self.color.data, self.color.size)
    }

    pub(crate) fn grayscale_upload(&self) -> PlaneUpload<'_> {
        self.grayscale.upload()
    }

    pub(crate) fn color_upload(&self) -> PlaneUpload<'_> {
        self.color.upload()
    }

    pub fn clear_dirty(&mut self) {
        self.grayscale.clear_dirty();
        self.color.clear_dirty();
    }

    pub fn reset(&mut self) {
        self.grayscale = AtlasPlane::new(GRAYSCALE_SIZE, 1);
        self.color = AtlasPlane::new(COLOR_SIZE, 4);
    }
}

impl AtlasPlane {
    fn new(size: u32, bpp: u32) -> Self {
        Self {
            allocator: AtlasAllocator::new(ESize::new(size as i32, size as i32)),
            data: vec![0u8; (size * size * bpp) as usize],
            size,
            bpp,
            full_dirty: true,
            dirty_rects: Vec::new(),
        }
    }

    fn copy_bitmap(&mut self, bitmap: &[u8], pos: [u32; 2], width: u32, height: u32) {
        let bpp = self.bpp as usize;
        let dst_stride = self.size as usize * bpp;
        let src_stride = width as usize * bpp;
        let x_bytes = pos[0] as usize * bpp;

        for row in 0..height as usize {
            let src_start = row * src_stride;
            let dst_start = (pos[1] as usize + row) * dst_stride + x_bytes;
            let src_end = src_start + src_stride;
            let dst_end = dst_start + src_stride;
            if src_end <= bitmap.len() && dst_end <= self.data.len() {
                self.data[dst_start..dst_end].copy_from_slice(&bitmap[src_start..src_end]);
            }
        }
        // A pending full upload already covers this region; only track the
        // sub-rect once the plane is otherwise clean.
        if !self.full_dirty {
            self.dirty_rects.push(DirtyRect {
                x: pos[0],
                y: pos[1],
                w: width,
                h: height,
            });
        }
    }

    fn upload(&self) -> PlaneUpload<'_> {
        if self.full_dirty {
            PlaneUpload::Full
        } else if self.dirty_rects.is_empty() {
            PlaneUpload::None
        } else {
            PlaneUpload::Rects(&self.dirty_rects)
        }
    }

    fn clear_dirty(&mut self) {
        self.full_dirty = false;
        self.dirty_rects.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_glyph(atlas: &mut GlyphAtlas, w: u32, h: u32) -> AtlasEntry {
        let bitmap = vec![0xffu8; (w * h) as usize];
        atlas.insert(&bitmap, w, h, 0, 0, false).expect("alloc")
    }

    #[test]
    fn fresh_plane_demands_a_full_upload() {
        let atlas = GlyphAtlas::new();
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::Full);
        assert_eq!(atlas.color_upload(), PlaneUpload::Full);
    }

    #[test]
    fn insert_while_full_dirty_adds_no_rects() {
        // Before the first upload the whole plane is dirty, so a sub-rect
        // would be redundant — the upload stays `Full`, not `Rects`.
        let mut atlas = GlyphAtlas::new();
        insert_glyph(&mut atlas, 4, 6);
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::Full);
    }

    #[test]
    fn clear_dirty_then_insert_records_one_rect() {
        let mut atlas = GlyphAtlas::new();
        atlas.clear_dirty();
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::None);

        let entry = insert_glyph(&mut atlas, 4, 6);
        let PlaneUpload::Rects(rects) = atlas.grayscale_upload() else {
            panic!("expected sub-rect upload after a clean insert");
        };
        assert_eq!(
            rects,
            &[DirtyRect {
                x: entry.pos[0],
                y: entry.pos[1],
                w: 4,
                h: 6,
            }]
        );
    }

    #[test]
    fn clear_dirty_drops_pending_rects() {
        let mut atlas = GlyphAtlas::new();
        atlas.clear_dirty();
        insert_glyph(&mut atlas, 3, 3);
        assert!(matches!(atlas.grayscale_upload(), PlaneUpload::Rects(_)));

        atlas.clear_dirty();
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::None);
    }

    #[test]
    fn color_glyph_dirties_only_the_color_plane() {
        let mut atlas = GlyphAtlas::new();
        atlas.clear_dirty();

        let bitmap = vec![0xffu8; 4 * 5 * 4];
        atlas.insert(&bitmap, 4, 5, 0, 0, true).expect("alloc");

        assert!(matches!(atlas.color_upload(), PlaneUpload::Rects(_)));
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::None);
    }

    #[test]
    fn reset_restores_full_upload() {
        let mut atlas = GlyphAtlas::new();
        atlas.clear_dirty();
        insert_glyph(&mut atlas, 2, 2);
        atlas.reset();
        assert_eq!(atlas.grayscale_upload(), PlaneUpload::Full);
        assert_eq!(atlas.color_upload(), PlaneUpload::Full);
    }
}
