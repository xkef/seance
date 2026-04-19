//! GPU-side image cache keyed by `image_id`.
//!
//! One wgpu texture per distinct image; bind groups are created once per
//! texture and reused across frames. Entries survive a grace window of
//! `EVICT_AGE_FRAMES` after last reference so short-lived unbinds (e.g.
//! during a scroll) don't churn the GPU.

use rustc_hash::FxHashMap;
use seance_vt::{ImageInfo, ImageVisitor};
use wgpu::*;

/// Frames of grace before an unreferenced image is dropped. At ~240Hz
/// this is roughly half a second.
pub(crate) const EVICT_AGE_FRAMES: u64 = 120;

pub(crate) struct CachedImage {
    // Held to keep the texture alive; accessed via `bind_group`.
    _texture: Texture,
    pub bind_group: BindGroup,
    pub width: u32,
    pub height: u32,
    pub last_seen_frame: u64,
}

pub(crate) struct ImageCache {
    entries: FxHashMap<u32, CachedImage>,
    current_frame: u64,
    sampler: Sampler,
    bgl: BindGroupLayout,
}

impl ImageCache {
    pub(crate) fn new(device: &Device, bgl: BindGroupLayout) -> Self {
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("image_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        Self {
            entries: FxHashMap::default(),
            current_frame: 0,
            sampler,
            bgl,
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.current_frame = self.current_frame.wrapping_add(1);
    }

    pub(crate) fn bind_group(&self, image_id: u32) -> Option<&BindGroup> {
        self.entries.get(&image_id).map(|e| &e.bind_group)
    }

    pub(crate) fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        info: &ImageInfo<'_>,
    ) {
        if info.width == 0 || info.height == 0 {
            return;
        }
        let expected_len = (info.width as usize)
            .checked_mul(info.height as usize)
            .and_then(|p| p.checked_mul(4));
        if Some(info.rgba.len()) != expected_len {
            return;
        }

        let entry = self.entries.entry(info.image_id);
        match entry {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let existing = slot.get_mut();
                if existing.width == info.width && existing.height == info.height {
                    existing.last_seen_frame = self.current_frame;
                    return;
                }
                *existing = create_entry(
                    device,
                    queue,
                    &self.bgl,
                    &self.sampler,
                    info,
                    self.current_frame,
                );
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(create_entry(
                    device,
                    queue,
                    &self.bgl,
                    &self.sampler,
                    info,
                    self.current_frame,
                ));
            }
        }
    }

    pub(crate) fn evict_stale(&mut self, max_age: u64) {
        let now = self.current_frame;
        self.entries
            .retain(|_, e| now.wrapping_sub(e.last_seen_frame) <= max_age);
    }
}

fn create_entry(
    device: &Device,
    queue: &Queue,
    bgl: &BindGroupLayout,
    sampler: &Sampler,
    info: &ImageInfo<'_>,
    frame: u64,
) -> CachedImage {
    let extent = Extent3d {
        width: info.width,
        height: info.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("kitty_image"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        info.rgba,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(info.width * 4),
            rows_per_image: None,
        },
        extent,
    );

    let view = texture.create_view(&TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("kitty_image_bg"),
        layout: bgl,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    });

    CachedImage {
        _texture: texture,
        bind_group,
        width: info.width,
        height: info.height,
        last_seen_frame: frame,
    }
}

/// Bridges `ImageVisitor` into the wgpu cache. Owns transient references
/// to the cache + device/queue for the duration of one `visit_images`.
pub(crate) struct ImageUploader<'a> {
    pub(crate) cache: &'a mut ImageCache,
    pub(crate) device: &'a Device,
    pub(crate) queue: &'a Queue,
}

impl ImageVisitor for ImageUploader<'_> {
    fn image(&mut self, info: &ImageInfo<'_>) {
        self.cache.upload(self.device, self.queue, info);
    }
}
