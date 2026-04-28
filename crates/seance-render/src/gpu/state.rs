use std::sync::Arc;

use wgpu::*;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use super::atlas_texture::{atlas_view, write_atlas_plane};
use super::dynamic_buffer::DynamicBuffer;
use super::pipeline::Pipelines;
use super::uniforms::Uniforms;
use crate::image::ImageRenderer;
use crate::renderer::RenderInputs;
use crate::text::{CellText, FrameInfo, GlyphAtlas};
use seance_config::Theme;
use seance_vt::{DirtySnapshot, FrameSource, PlacementLayer};

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
    surface: Surface<'static>,
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

        let pipelines = Pipelines::new(&device, format);
        let images = ImageRenderer::new(&device, format, &pipelines.uniform_bgl);

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
            size,
            surface_dirty: false,
        }
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
        if self.surface_dirty {
            self.surface.configure(&self.device, &self.config);
            self.surface_dirty = false;
        }

        let Some(output) = self.acquire_surface_texture() else {
            return false;
        };

        self.upload_uniforms(frame_info, inputs, theme);
        self.upload_cell_data(cells.bg_cells, cells.text_cells, cells.dirty);
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

    fn acquire_surface_texture(&mut self) -> Option<SurfaceTexture> {
        match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                Some(frame)
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => None,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                log::warn!("surface acquire failed: {other:?}");
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
    ) {
        // `Clean` keeps last frame's GPU buffers verbatim — both the bg
        // grid and `text_instance_count` stay valid because the CPU
        // rebuild is byte-identical. This is the dominant #196 savings.
        //
        // `Partial` originally wrote only the dirty row span of bg_cells,
        // but that produced stray glyph fragments above the lualine in
        // nvim (#197). Until that is root-caused, treat `Partial` like
        // `Full` and re-upload the whole bg buffer.
        if matches!(dirty, DirtySnapshot::Clean) {
            return;
        }

        self.upload_bg_full(bytemuck::cast_slice(bg_cells));

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

    fn record_passes(&self, encoder: &mut CommandEncoder, view: &TextureView) {
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

        // Pass 1: solid background.
        pass.set_pipeline(&self.pipelines.bg_color);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.draw(0..3, 0..1);

        // Kitty images below the cell background layer.
        self.images
            .record_layer(&mut pass, PlacementLayer::BelowBg, &self.uniform_bind_group);

        // Pass 2: per-cell backgrounds + selection + cursor shapes.
        let maybe_bg_bg = self.bg_cells.bind_group.as_ref();
        if let Some(bg_bg) = maybe_bg_bg {
            pass.set_pipeline(&self.pipelines.cell_bg);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_bind_group(1, bg_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // Kitty images between cell bg and text.
        self.images.record_layer(
            &mut pass,
            PlacementLayer::BelowText,
            &self.uniform_bind_group,
        );

        // Pass 3: text (instanced quads).
        if let (Some(bg_bg), Some(atlas_bg), Some(text_buf)) = (
            maybe_bg_bg,
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

        // Kitty images above the text layer.
        self.images.record_layer(
            &mut pass,
            PlacementLayer::AboveText,
            &self.uniform_bind_group,
        );
    }
}
