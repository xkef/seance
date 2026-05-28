use std::sync::Arc;

use wgpu::*;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use super::atlas_texture::{atlas_view, write_atlas_plane};
use super::dynamic_buffer::DynamicBuffer;
use super::pipeline::Pipelines;
use super::schedule::{DrawOp, LayerSchedule};
use super::uniforms::Uniforms;
use crate::image::ImageRenderer;
use crate::renderer::RenderInputs;
use crate::text::{CellText, FrameInfo, GlyphAtlas};
use seance_config::Theme;
use seance_frame::{FrameSource, PlacementLayer, SubRole, Z_MAIN};
use seance_protocol::frame::DirtySnapshot;
use seance_protocol::image_cache::ImageCacheEvent;

const ATLAS_GRAYSCALE_FORMAT: TextureFormat = TextureFormat::R8Unorm;
const ATLAS_COLOR_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Per-frame cell data the GPU layer consumes — bundled to keep
/// `render_frame`'s arg count down.
pub(crate) struct CellFrame<'a> {
    pub bg_cells: &'a [[u8; 4]],
    pub text_cells: &'a [CellText],
    pub dirty: &'a DirtySnapshot,
}

pub(crate) struct GpuState {
    // `None` in the headless render path (regression harness, benches),
    // which renders to an owned offscreen texture instead of a swapchain.
    surface: Option<Surface<'static>>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    pipelines: Pipelines,

    uniform_buffer: Buffer,
    uniform_bind_group: BindGroup,

    bg_cells: DynamicBuffer,
    text_instances: DynamicBuffer,
    text_instance_count: u32,

    atlas_grayscale: Option<Texture>,
    atlas_color: Option<Texture>,
    atlas_bind_group: Option<BindGroup>,
    atlas_sampler: Sampler,

    images: ImageRenderer,

    schedule: LayerSchedule,

    size: PhysicalSize<u32>,
    surface_dirty: bool,
}

impl GpuState {
    pub(crate) async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("seance"),
                required_features: Features::empty(),
                required_limits: Limits::default(),
                memory_hints: MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .expect("failed to create device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: PresentMode::AutoVsync,
            alpha_mode: CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Self::assemble(device, queue, config, Some(surface), size)
    }

    /// Build a surfaceless `GpuState` that renders to an owned offscreen
    /// texture. Used by the regression harness and benches. Returns `None`
    /// when no GPU adapter is available (CI without a GPU, sandboxes).
    ///
    /// The target format is `Rgba8Unorm` so [`Self::render_to_rgba`] reads
    /// back tightly-packed RGBA without a swizzle. The windowed path picks a
    /// non-srgb surface format for the same reason, so the two paths shade
    /// identically.
    pub(crate) async fn new_headless(width: u32, height: u32) -> Option<Self> {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::PRIMARY,
            ..InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok()?;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("seance-headless"),
                required_features: Features::empty(),
                required_limits: Limits::default(),
                memory_hints: MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .ok()?;

        let size = PhysicalSize::new(width.max(1), height.max(1));
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: TextureFormat::Rgba8Unorm,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::AutoVsync,
            alpha_mode: CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        Some(Self::assemble(device, queue, config, None, size))
    }

    fn assemble(
        device: Device,
        queue: Queue,
        config: SurfaceConfiguration,
        surface: Option<Surface<'static>>,
        size: PhysicalSize<u32>,
    ) -> Self {
        let pipelines = Pipelines::new(&device, config.format);
        let images = ImageRenderer::new(&device, config.format, &pipelines.uniform_bgl);

        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("uniform_bg"),
            layout: &pipelines.uniform_bgl,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let atlas_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("atlas_sampler"),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            surface,
            device,
            queue,
            config,
            pipelines,
            uniform_buffer,
            uniform_bind_group,
            bg_cells: DynamicBuffer::new(
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
                "bg_cells",
            ),
            text_instances: DynamicBuffer::new(
                BufferUsages::VERTEX | BufferUsages::COPY_DST,
                "text_instances",
            ),
            text_instance_count: 0,
            atlas_grayscale: None,
            atlas_color: None,
            atlas_bind_group: None,
            atlas_sampler,
            images,
            schedule: LayerSchedule::default(),
            size,
            surface_dirty: false,
        }
    }

    pub(crate) fn apply_image_cache_event(&mut self, event: &ImageCacheEvent) {
        self.images
            .apply_cache_event(&self.device, &self.queue, event);
    }

    /// Collect kitty image placements + upload image textures. Call
    /// between `update_frame` and `render_frame`.
    pub(crate) fn update_image_frame(&mut self, source: &mut dyn FrameSource, fi: &FrameInfo) {
        self.images
            .update_frame(&self.device, &self.queue, source, fi);
    }

    pub(crate) fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface_dirty = true;
        }
    }

    pub(crate) fn render_frame(
        &mut self,
        frame_info: &FrameInfo,
        cells: CellFrame<'_>,
        atlas: &GlyphAtlas,
        inputs: &RenderInputs,
        theme: &Theme,
    ) -> bool {
        let _span = tracing::trace_span!("gpu::submit").entered();
        let Some(surface) = self.surface.as_ref() else {
            return false;
        };
        if self.surface_dirty {
            surface.configure(&self.device, &self.config);
            self.surface_dirty = false;
        }

        let Some(output) = self.acquire_surface_texture() else {
            return false;
        };

        self.upload_uniforms(frame_info, inputs, theme);
        self.upload_cell_data(
            cells.bg_cells,
            cells.text_cells,
            cells.dirty,
            frame_info.grid_cols,
        );
        self.upload_atlas(atlas);
        self.ensure_atlas_bind_group();

        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.record_passes(&mut encoder, &view);
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        true
    }

    /// Render one frame to an owned offscreen texture and read it back as
    /// tightly-packed `Rgba8Unorm` (`width * height * 4` bytes, row-major,
    /// top-left origin). The regression harness and benches use this; there
    /// is no swapchain involved, so it works without a window.
    ///
    /// UNVERIFIED: written against the wgpu 29 readback contract but not yet
    /// executed on a GPU (the dev container has no adapter). Validate on a
    /// GPU host before relying on the exact bytes.
    pub(crate) fn render_to_rgba(
        &mut self,
        frame_info: &FrameInfo,
        cells: CellFrame<'_>,
        atlas: &GlyphAtlas,
        inputs: &RenderInputs,
        theme: &Theme,
    ) -> Vec<u8> {
        self.upload_uniforms(frame_info, inputs, theme);
        self.upload_cell_data(
            cells.bg_cells,
            cells.text_cells,
            cells.dirty,
            frame_info.grid_cols,
        );
        self.upload_atlas(atlas);
        self.ensure_atlas_bind_group();

        let (width, height) = (self.size.width.max(1), self.size.height.max(1));
        let target = self.device.create_texture(&TextureDescriptor {
            label: Some("headless_target"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: self.config.format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&TextureViewDescriptor::default());

        // 256-byte row alignment is required by copy_texture_to_buffer; we
        // strip the padding back out after mapping.
        let bytes_per_pixel = 4u32;
        let unpadded_row = width * bytes_per_pixel;
        let align = COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;
        let readback = self.device.create_buffer(&BufferDescriptor {
            label: Some("headless_readback"),
            size: (padded_row * height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("headless_frame"),
            });
        self.record_passes(&mut encoder, &view);
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: &readback,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        slice.map_async(MapMode::Read, |_| {});
        let _ = self.device.poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        });

        let mapped = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_row) as usize;
            out.extend_from_slice(&mapped[start..start + unpadded_row as usize]);
        }
        drop(mapped);
        readback.unmap();
        out
    }

    fn acquire_surface_texture(&mut self) -> Option<SurfaceTexture> {
        let surface = self.surface.as_ref()?;
        match surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                Some(frame)
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => None,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                surface.configure(&self.device, &self.config);
                None
            }
            other => {
                tracing::warn!("surface acquire failed: {other:?}");
                None
            }
        }
    }

    fn upload_uniforms(&self, fi: &FrameInfo, inputs: &RenderInputs, theme: &Theme) {
        let uniforms = Uniforms::from_frame_info(
            fi,
            self.size.width as f32,
            self.size.height as f32,
            inputs,
            theme,
        );
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    fn upload_cell_data(
        &mut self,
        bg_cells: &[[u8; 4]],
        text_cells: &[CellText],
        dirty: &DirtySnapshot,
        grid_cols: u16,
    ) {
        // `Clean` short-circuits both buffers — the GPU still holds last
        // frame's data and the CPU rebuild is byte-identical, so the
        // upload is wasted bandwidth. `text_instance_count` is intentionally
        // left at its previous value (the count is unchanged on Clean).
        if matches!(dirty, DirtySnapshot::Clean) {
            return;
        }

        let bg_bytes: &[u8] = bytemuck::cast_slice(bg_cells);

        // Partial upload is only safe when the existing buffer already
        // covers the full slice. On first frame / resize the buffer is
        // either absent or smaller, so degrade to Full and let
        // `DynamicBuffer::upload` reallocate.
        let bg_buffer_covers = self
            .bg_cells
            .buffer
            .as_ref()
            .is_some_and(|b| b.size() >= bg_bytes.len() as u64);

        match dirty {
            DirtySnapshot::Clean => unreachable!("handled above"),
            DirtySnapshot::Full => {
                self.upload_bg_full(bg_bytes);
            }
            DirtySnapshot::Partial(_) if !bg_buffer_covers => {
                // Defensive: VT marks resize as Full so this path is
                // unlikely in practice, but we handle it anyway.
                self.upload_bg_full(bg_bytes);
            }
            DirtySnapshot::Partial(rows) => {
                // `Terminal::dirty_snapshot` folds empty Partial -> Clean,
                // so `rows` is non-empty here. Indices are sorted ascending,
                // so first/last give a contiguous min..=max span.
                let row_min = *rows.first().expect("non-empty Partial") as usize;
                let row_max = *rows.last().expect("non-empty Partial") as usize;
                let (offset, len) = bg_byte_range(row_min, row_max, grid_cols as usize);
                let buffer = self
                    .bg_cells
                    .buffer
                    .as_ref()
                    .expect("bg_buffer_covers checked");
                self.queue
                    .write_buffer(buffer, offset, &bg_bytes[offset as usize..][..len]);
            }
        }

        // text_cells is glyph-indexed (densely packed; a row → instance
        // mapping isn't stable across frames), so the partial-by-row
        // strategy doesn't apply. Re-upload the full buffer whenever the
        // VT reported any change. Skipping on Clean is the dominant win.
        self.text_instance_count = text_cells.len() as u32;
        if !text_cells.is_empty() {
            self.text_instances
                .upload(&self.device, &self.queue, bytemuck::cast_slice(text_cells));
        }
    }

    fn upload_bg_full(&mut self, bg_bytes: &[u8]) {
        if bg_bytes.is_empty() {
            return;
        }
        if self.bg_cells.upload(&self.device, &self.queue, bg_bytes) {
            self.bg_cells.bind_group = Some(self.device.create_bind_group(&BindGroupDescriptor {
                label: Some("bg_cells_bg"),
                layout: &self.pipelines.bg_cells_bgl,
                entries: &[BindGroupEntry {
                    binding: 0,
                    resource: self.bg_cells.buffer.as_ref().unwrap().as_entire_binding(),
                }],
            }));
        }
    }

    fn upload_atlas(&mut self, atlas: &GlyphAtlas) {
        let (gs_data, gs_size) = atlas.grayscale_data();
        if gs_size > 0
            && write_atlas_plane(
                &self.device,
                &self.queue,
                &mut self.atlas_grayscale,
                gs_data,
                gs_size,
                ATLAS_GRAYSCALE_FORMAT,
                "atlas_grayscale",
            )
        {
            self.atlas_bind_group = None;
        }

        let (color_data, color_size) = atlas.color_data();
        if color_size > 0
            && write_atlas_plane(
                &self.device,
                &self.queue,
                &mut self.atlas_color,
                color_data,
                color_size,
                ATLAS_COLOR_FORMAT,
                "atlas_color",
            )
        {
            self.atlas_bind_group = None;
        }
    }

    fn ensure_atlas_bind_group(&mut self) {
        if self.atlas_bind_group.is_some() {
            return;
        }
        let grayscale = atlas_view(
            &self.device,
            self.atlas_grayscale.as_ref(),
            ATLAS_GRAYSCALE_FORMAT,
        );
        let color = atlas_view(&self.device, self.atlas_color.as_ref(), ATLAS_COLOR_FORMAT);
        self.atlas_bind_group = Some(self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("atlas_bg"),
            layout: &self.pipelines.atlas_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&grayscale),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&color),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&self.atlas_sampler),
                },
            ],
        }));
    }

    /// Rebuild the per-frame draw schedule. Today this is a single `Z_MAIN`
    /// layer whose sub-roles reproduce the legacy fixed pass order: the three
    /// Kitty bands and `cell_bg` sit in `Below`, glyphs in `Content`, and the
    /// above-text band in `Above`. New layers (window backgrounds at negative
    /// z, floating UI at positive z) are added with `layer_for_z(N)` and need
    /// no changes here.
    fn rebuild_schedule(&mut self) {
        self.schedule.clear();
        let main = self.schedule.layer_for_z(Z_MAIN);
        main.push(SubRole::Below, DrawOp::BgColorFill);
        main.push(SubRole::Below, DrawOp::KittyBand(PlacementLayer::BelowBg));
        main.push(SubRole::Below, DrawOp::CellBg);
        main.push(SubRole::Below, DrawOp::KittyBand(PlacementLayer::BelowText));
        main.push(SubRole::Content, DrawOp::CellText);
        main.push(SubRole::Above, DrawOp::KittyBand(PlacementLayer::AboveText));
    }

    fn record_passes(&mut self, encoder: &mut CommandEncoder, view: &TextureView) {
        self.rebuild_schedule();

        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("seance_frame"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        for layer in self.schedule.layers() {
            for op in layer.ops() {
                self.record_op(&mut pass, op);
            }
        }
    }

    fn record_op(&self, pass: &mut RenderPass<'_>, op: DrawOp) {
        match op {
            DrawOp::BgColorFill => {
                pass.set_pipeline(&self.pipelines.bg_color);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            DrawOp::CellBg => {
                if let Some(bg_bg) = self.bg_cells.bind_group.as_ref() {
                    pass.set_pipeline(&self.pipelines.cell_bg);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_bind_group(1, bg_bg, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            DrawOp::CellText => {
                if let (Some(bg_bg), Some(atlas_bg), Some(text_buf)) = (
                    self.bg_cells.bind_group.as_ref(),
                    self.atlas_bind_group.as_ref(),
                    self.text_instances.buffer.as_ref(),
                ) && self.text_instance_count > 0
                {
                    pass.set_pipeline(&self.pipelines.cell_text);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_bind_group(1, bg_bg, &[]);
                    pass.set_bind_group(2, atlas_bg, &[]);
                    pass.set_vertex_buffer(0, text_buf.slice(..));
                    pass.draw(0..4, 0..self.text_instance_count);
                }
            }
            DrawOp::KittyBand(band) => {
                self.images
                    .record_layer(pass, band, &self.uniform_bind_group);
            }
        }
    }
}

/// Translate an inclusive row range into a byte `(offset, len)` over a
/// row-major `[u8; 4]`-per-cell buffer.
fn bg_byte_range(row_min: usize, row_max: usize, grid_cols: usize) -> (u64, usize) {
    debug_assert!(row_min <= row_max);
    let stride = grid_cols * size_of::<[u8; 4]>();
    let offset = (row_min * stride) as u64;
    let len = (row_max - row_min + 1) * stride;
    (offset, len)
}

#[cfg(test)]
mod tests {
    use super::bg_byte_range;

    #[test]
    fn bg_byte_range_single_row_zero() {
        assert_eq!(bg_byte_range(0, 0, 80), (0, 80 * 4));
    }

    #[test]
    fn bg_byte_range_single_row_n() {
        let (offset, len) = bg_byte_range(7, 7, 80);
        assert_eq!(offset, 7 * 80 * 4);
        assert_eq!(len, 80 * 4);
    }

    #[test]
    fn bg_byte_range_inclusive_span() {
        let (offset, len) = bg_byte_range(5, 10, 80);
        assert_eq!(offset, 5 * 80 * 4);
        assert_eq!(len, 6 * 80 * 4); // rows 5..=10 inclusive
    }
}
