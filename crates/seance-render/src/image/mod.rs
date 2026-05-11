//! Kitty graphics image compositing.
//!
//! Keyed by scoped image key: one wgpu texture per image, cached across frames
//! and evicted when unreferenced for `EVICT_AGE_FRAMES`. Placements for
//! each z-layer (below-bg, below-text, above-text) are collected into a
//! single instance buffer; `record_layer` issues one draw per placement
//! against the right per-image bind group.

mod builder;
mod cache;
mod pipeline;

use seance_frame::{FrameSource, PlacementLayer};
use seance_protocol::image_cache::ImageCacheEvent;
use wgpu::*;

use crate::text::FrameInfo;

use self::builder::{ImageFrame, PlacementCollector};
use self::cache::{EVICT_AGE_FRAMES, ImageCache, ImageUploader};
use self::pipeline::ImagePipeline;

pub(crate) struct ImageRenderer {
    pipeline: ImagePipeline,
    cache: ImageCache,
    frame: ImageFrame,
    instance_buf: Option<Buffer>,
    instance_capacity: u64,
}

impl ImageRenderer {
    pub(crate) fn new(
        device: &Device,
        format: TextureFormat,
        uniform_bgl: &BindGroupLayout,
    ) -> Self {
        let pipeline = ImagePipeline::new(device, format, uniform_bgl);
        let cache = ImageCache::new(device, pipeline.image_bgl());
        Self {
            pipeline,
            cache,
            frame: ImageFrame::default(),
            instance_buf: None,
            instance_capacity: 0,
        }
    }

    pub(crate) fn apply_cache_event(
        &mut self,
        device: &Device,
        queue: &Queue,
        event: &ImageCacheEvent,
    ) {
        self.cache.apply_event(device, queue, event);
    }

    /// Drive per-frame image cache upload and placement collection.
    pub(crate) fn update_frame(
        &mut self,
        device: &Device,
        queue: &Queue,
        source: &mut dyn FrameSource,
        fi: &FrameInfo,
    ) {
        self.cache.begin_frame();
        self.frame.clear();

        let pane = source.pane_ref();
        let mut uploader = ImageUploader {
            cache: &mut self.cache,
            device,
            queue,
            pane,
        };
        source.visit_images(&mut uploader);
        for layer in [
            PlacementLayer::BelowBg,
            PlacementLayer::BelowText,
            PlacementLayer::AboveText,
        ] {
            let mut collector = PlacementCollector {
                frame: &mut self.frame,
                layer,
                pane,
                fi,
            };
            source.visit_placements(layer, &mut collector);
        }
        self.frame.finalize();
        for layer in [
            PlacementLayer::BelowBg,
            PlacementLayer::BelowText,
            PlacementLayer::AboveText,
        ] {
            for draw in self.frame.draws_for(layer) {
                self.cache.touch(draw.image_key);
            }
        }

        self.cache.evict_stale(EVICT_AGE_FRAMES);

        if !self.frame.instances.is_empty() {
            self.ensure_instance_buffer(device, queue);
        }
    }

    fn ensure_instance_buffer(&mut self, device: &Device, queue: &Queue) {
        let data: &[u8] = bytemuck::cast_slice(&self.frame.instances);
        let needed = data.len() as u64;
        if self.instance_capacity < needed {
            self.instance_buf = Some(device.create_buffer(&BufferDescriptor {
                label: Some("image_instances"),
                size: needed,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.instance_capacity = needed;
        }
        queue.write_buffer(self.instance_buf.as_ref().unwrap(), 0, data);
    }

    pub(crate) fn record_layer(
        &self,
        pass: &mut RenderPass<'_>,
        layer: PlacementLayer,
        uniform_bg: &BindGroup,
    ) {
        let draws = self.frame.draws_for(layer);
        if draws.is_empty() {
            return;
        }
        let Some(buf) = &self.instance_buf else {
            return;
        };

        pass.set_pipeline(self.pipeline.pipeline());
        pass.set_bind_group(0, uniform_bg, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        for draw in draws {
            let Some(bg) = self.cache.bind_group(draw.image_key) else {
                continue;
            };
            pass.set_bind_group(1, bg, &[]);
            pass.draw(0..4, draw.instance..draw.instance + 1);
        }
    }
}
