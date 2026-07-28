use etagere::{AtlasAllocator, Size as ESize};

const GRAYSCALE_SIZE: u32 = 2048;
const COLOR_SIZE: u32 = 1024;

pub struct GlyphAtlas {
    grayscale: AtlasPlane,
    color: AtlasPlane,
}

struct AtlasPlane {
    allocator: AtlasAllocator,
    data: Vec<u8>,
    size: u32,
    bpp: u32,
    dirty: bool,
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
        // swash emits color glyphs as straight-alpha RGBA, but every pipeline
        // blends premultiplied (src=One, dst=OneMinusSrcAlpha). Fold alpha into
        // RGB here so antialiased edges do not read over-bright (#314). The
        // grayscale (mask) plane carries coverage only and stays untouched.
        if is_color {
            let premultiplied = premultiply_rgba(bitmap);
            plane.copy_bitmap(&premultiplied, pos, width, height);
        } else {
            plane.copy_bitmap(bitmap, pos, width, height);
        }
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

    pub fn clear_dirty(&mut self) {
        self.grayscale.dirty = false;
        self.color.dirty = false;
    }

    pub fn reset(&mut self) {
        self.grayscale = AtlasPlane::new(GRAYSCALE_SIZE, 1);
        self.color = AtlasPlane::new(COLOR_SIZE, 4);
    }
}

fn premultiply_rgba(bitmap: &[u8]) -> Vec<u8> {
    let mut out = bitmap.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u16;
        px[0] = (px[0] as u16 * a / 255) as u8;
        px[1] = (px[1] as u16 * a / 255) as u8;
        px[2] = (px[2] as u16 * a / 255) as u8;
    }
    out
}

impl AtlasPlane {
    fn new(size: u32, bpp: u32) -> Self {
        Self {
            allocator: AtlasAllocator::new(ESize::new(size as i32, size as i32)),
            data: vec![0u8; (size * size * bpp) as usize],
            size,
            bpp,
            dirty: true,
        }
    }

    #[cfg(test)]
    fn texel_at(&self, pos: [u32; 2]) -> [u8; 4] {
        let bpp = self.bpp as usize;
        let start = (pos[1] as usize * self.size as usize + pos[0] as usize) * bpp;
        [
            self.data[start],
            self.data[start + 1],
            self.data[start + 2],
            self.data[start + 3],
        ]
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
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_glyph_stored_premultiplied() {
        let mut atlas = GlyphAtlas::new();
        let entry = atlas
            .insert(&[200, 100, 50, 128], 1, 1, 0, 0, true)
            .expect("color glyph allocates");
        assert_eq!(atlas.color.texel_at(entry.pos), [100, 50, 25, 128]);
    }

    #[test]
    fn opaque_color_texel_is_unchanged() {
        let mut atlas = GlyphAtlas::new();
        let entry = atlas
            .insert(&[200, 100, 50, 255], 1, 1, 0, 0, true)
            .expect("color glyph allocates");
        assert_eq!(atlas.color.texel_at(entry.pos), [200, 100, 50, 255]);
    }

    #[test]
    fn grayscale_coverage_is_not_premultiplied() {
        let mut atlas = GlyphAtlas::new();
        let entry = atlas
            .insert(&[128], 1, 1, 0, 0, false)
            .expect("mask glyph allocates");
        let bpp = atlas.grayscale.bpp as usize;
        let start =
            (entry.pos[1] as usize * atlas.grayscale.size as usize + entry.pos[0] as usize) * bpp;
        assert_eq!(atlas.grayscale.data[start], 128);
    }
}
