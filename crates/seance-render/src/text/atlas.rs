use etagere::{AtlasAllocator, Size as ESize};

pub struct GlyphAtlas {
    grayscale: AtlasPlane,
    color: AtlasPlane,
}

struct AtlasPlane {
    allocator: AtlasAllocator,
    data: Vec<u8>,
    size: u32,
    dirty: bool,
    bpp: u32,
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
            grayscale: AtlasPlane::new(2048, 1),
            color: AtlasPlane::new(1024, 4),
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

        let dst_stride = plane.size as usize * plane.bpp as usize;
        let src_stride = width as usize * plane.bpp as usize;
        for row in 0..height as usize {
            let dst_y = pos[1] as usize + row;
            let dst_x = pos[0] as usize * plane.bpp as usize;
            let dst_start = dst_y * dst_stride + dst_x;
            let src_start = row * src_stride;
            if src_start + src_stride <= bitmap.len() && dst_start + src_stride <= plane.data.len()
            {
                plane.data[dst_start..dst_start + src_stride]
                    .copy_from_slice(&bitmap[src_start..src_start + src_stride]);
            }
        }
        plane.dirty = true;

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
        self.grayscale = AtlasPlane::new(2048, 1);
        self.color = AtlasPlane::new(1024, 4);
    }
}

impl AtlasPlane {
    fn new(size: u32, bpp: u32) -> Self {
        Self {
            allocator: AtlasAllocator::new(ESize::new(size as i32, size as i32)),
            data: vec![0u8; (size * size * bpp) as usize],
            size,
            dirty: true,
            bpp,
        }
    }
}
