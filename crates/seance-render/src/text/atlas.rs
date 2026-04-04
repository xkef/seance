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

    pub fn clear_dirty(&mut self) {
        self.grayscale.dirty = false;
        self.color.dirty = false;
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
            dirty: true,
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
        self.dirty = true;
    }
}
