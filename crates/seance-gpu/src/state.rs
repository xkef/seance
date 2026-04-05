//! GPU state: wgpu surface, device, buffers, and frame rendering.

use std::sync::Arc;

use wgpu::*;
use wgpu::util::DeviceExt;

use ghostty_renderer::FrameSnapshot;

use crate::pipeline::Pipelines;
use crate::uniforms::Uniforms;

/// Manages the wgpu device, surface, pipelines, and per-frame GPU resources.
pub struct GpuState {
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    pipelines: Pipelines,
    uniform_buffer: Buffer,
    uniform_bind_group: BindGroup,
    bg_cells_buffer: Option<Buffer>,
    bg_cells_bind_group: Option<BindGroup>,
    text_instance_buffer: Option<Buffer>,
    text_instance_count: u32,
    atlas_grayscale_texture: Option<Texture>,
    atlas_color_texture: Option<Texture>,
    atlas_bind_group: Option<BindGroup>,
    atlas_sampler: Sampler,
    size: winit::dpi::PhysicalSize<u32>,
    surface_dirty: bool,
}

impl GpuState {
    /// Create the GPU state for a given window.
    pub async fn new(window: Arc<winit::window::Window>) -> Self {
        let size = window.inner_size();
        let instance = Instance::default();
        let surface = instance.create_surface(window).unwrap();

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
        // Use a non-sRGB format to match Ghostty's Metal renderer, which
        // uses bgra8Unorm (no automatic gamma encoding). Colors from
        // ghostty are already in sRGB and are passed through directly.
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
            bg_cells_buffer: None,
            bg_cells_bind_group: None,
            text_instance_buffer: None,
            text_instance_count: 0,
            atlas_grayscale_texture: None,
            atlas_color_texture: None,
            atlas_bind_group: None,
            atlas_sampler,
            size,
            surface_dirty: false,
        }
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            // Defer configure to render_frame so the reconfigure and
            // draw happen atomically, avoiding a blank flash.
            self.surface_dirty = true;
        }
    }

    /// Render only the background color (no cell content). Used during
    /// resize transitions to avoid showing a half-redrawn terminal.
    pub fn render_frame_bg_only(&mut self, snapshot: &FrameSnapshot<'_>) -> bool {
        if self.surface_dirty {
            self.surface.configure(&self.device, &self.config);
            self.surface_dirty = false;
        }

        let output = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame)
            | CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => {
                log::debug!("surface not ready: {other:?}");
                self.surface.configure(&self.device, &self.config);
                return false;
            }
        };

        let frame_data = snapshot.frame_data();
        let uniforms = Uniforms {
            projection: Uniforms::ortho(self.size.width as f32, self.size.height as f32),
            cell_size: [frame_data.cell_width, frame_data.cell_height],
            grid_size: [frame_data.grid_cols as u32, frame_data.grid_rows as u32],
            grid_padding: frame_data.grid_padding,
            bg_color: [
                frame_data.bg_color[0] as f32 / 255.0,
                frame_data.bg_color[1] as f32 / 255.0,
                frame_data.bg_color[2] as f32 / 255.0,
                frame_data.bg_color[3] as f32 / 255.0,
            ],
            min_contrast: frame_data.min_contrast,
            _pad0: 0,
            cursor_pos: [frame_data.cursor_pos[0] as u32, frame_data.cursor_pos[1] as u32],
            cursor_color: [
                frame_data.cursor_color[0] as f32 / 255.0,
                frame_data.cursor_color[1] as f32 / 255.0,
                frame_data.cursor_color[2] as f32 / 255.0,
                frame_data.cursor_color[3] as f32 / 255.0,
            ],
            cursor_wide: if frame_data.cursor_wide { 1 } else { 0 },
            _pad1: [0; 3],
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let view = output.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("frame_bg_only"),
        });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("seance_bg_only"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
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

            pass.set_pipeline(&self.pipelines.bg_color);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        true
    }

    /// Upload Level 2 data from a `FrameSnapshot` and render one frame.
    pub fn render_frame(&mut self, snapshot: &FrameSnapshot<'_>) -> bool {
        // Apply any deferred surface reconfiguration so the configure
        // and draw happen in the same frame (no blank flash on resize).
        if self.surface_dirty {
            self.surface.configure(&self.device, &self.config);
            self.surface_dirty = false;
        }

        // Acquire the surface. This blocks on vsync, rate-limiting
        // the upload+draw path to the display refresh rate.
        let output = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame)
            | CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => {
                log::debug!("surface not ready: {other:?}");
                self.surface.configure(&self.device, &self.config);
                return false;
            }
        };

        let frame_data = snapshot.frame_data();

        // Build uniforms.
        // Surface format is non-sRGB (matching Ghostty's Metal renderer),
        // so colors are passed through as sRGB without conversion.
        let uniforms = Uniforms {
            projection: Uniforms::ortho(self.size.width as f32, self.size.height as f32),
            cell_size: [frame_data.cell_width, frame_data.cell_height],
            grid_size: [frame_data.grid_cols as u32, frame_data.grid_rows as u32],
            grid_padding: frame_data.grid_padding,
            bg_color: [
                frame_data.bg_color[0] as f32 / 255.0,
                frame_data.bg_color[1] as f32 / 255.0,
                frame_data.bg_color[2] as f32 / 255.0,
                frame_data.bg_color[3] as f32 / 255.0,
            ],
            min_contrast: frame_data.min_contrast,
            _pad0: 0,
            cursor_pos: [frame_data.cursor_pos[0] as u32, frame_data.cursor_pos[1] as u32],
            cursor_color: [
                frame_data.cursor_color[0] as f32 / 255.0,
                frame_data.cursor_color[1] as f32 / 255.0,
                frame_data.cursor_color[2] as f32 / 255.0,
                frame_data.cursor_color[3] as f32 / 255.0,
            ],
            cursor_wide: if frame_data.cursor_wide { 1 } else { 0 },
            _pad1: [0; 3],
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Upload cell backgrounds
        let bg_cells = snapshot.bg_cells();
        if !bg_cells.is_empty() {
            let bg_data = bytemuck::cast_slice::<[u8; 4], u8>(bg_cells);
            self.upload_bg_cells(bg_data);
        }

        // Upload text cell instances
        let text_cells = snapshot.text_cells();
        self.text_instance_count = text_cells.len() as u32;
        if !text_cells.is_empty() {
            let text_data: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    text_cells.as_ptr().cast(),
                    text_cells.len() * size_of::<ghostty_renderer::CellText>(),
                )
            };
            self.upload_text_instances(text_data);
        }

        // Upload atlas textures. Reuses existing GPU textures via
        // write_texture; only allocates new ones on atlas resize.
        let gs = snapshot.atlas_grayscale();
        if gs.size > 0 {
            self.upload_atlas_grayscale(gs.data, gs.size);
        }
        let color = snapshot.atlas_color();
        if color.size > 0 {
            self.upload_atlas_color(color.data, color.size);
        }

        // Ensure atlas bind group exists (even with placeholder textures)
        self.ensure_atlas_bind_group();

        let view = output.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("frame"),
        });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("seance_frame"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
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

            // Pass 1: background color
            pass.set_pipeline(&self.pipelines.bg_color);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);

            // Pass 2: cell backgrounds
            if let Some(bg_bg) = &self.bg_cells_bind_group {
                pass.set_pipeline(&self.pipelines.cell_bg);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_bind_group(1, bg_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            // Pass 3: cell text (instanced quads)
            let have_inst = self.text_instance_buffer.is_some();
            let have_atlas = self.atlas_bind_group.is_some();
            let have_bg = self.bg_cells_bind_group.is_some();
            if self.text_instance_count == 0 || !have_inst || !have_atlas || !have_bg {
                log::debug!(
                    "skip text: count={} inst={} atlas={} bg={}",
                    self.text_instance_count, have_inst, have_atlas, have_bg
                );
            } else {
                log::debug!("drawing {} text instances", self.text_instance_count);
                pass.set_pipeline(&self.pipelines.cell_text);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_bind_group(1, self.bg_cells_bind_group.as_ref().unwrap(), &[]);
                pass.set_bind_group(2, self.atlas_bind_group.as_ref().unwrap(), &[]);
                pass.set_vertex_buffer(0, self.text_instance_buffer.as_ref().unwrap().slice(..));
                pass.draw(0..4, 0..self.text_instance_count);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        true
    }

    fn upload_bg_cells(&mut self, data: &[u8]) {
        let needed = data.len() as u64;
        let recreate = self
            .bg_cells_buffer
            .as_ref()
            .map_or(true, |b| b.size() < needed);

        if recreate {
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bg_cells"),
                contents: data,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            });
            let bind_group = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some("bg_cells_bg"),
                layout: &self.pipelines.bg_cells_bgl,
                entries: &[BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.bg_cells_buffer = Some(buffer);
            self.bg_cells_bind_group = Some(bind_group);
        } else {
            self.queue.write_buffer(self.bg_cells_buffer.as_ref().unwrap(), 0, data);
        }
    }

    fn upload_text_instances(&mut self, data: &[u8]) {
        let needed = data.len() as u64;
        let recreate = self
            .text_instance_buffer
            .as_ref()
            .map_or(true, |b| b.size() < needed);

        if recreate {
            self.text_instance_buffer = Some(
                self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("text_instances"),
                    contents: data,
                    usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                }),
            );
        } else {
            self.queue.write_buffer(self.text_instance_buffer.as_ref().unwrap(), 0, data);
        }
    }

    fn write_atlas(
        device: &Device,
        queue: &Queue,
        existing: &mut Option<Texture>,
        data: &[u8],
        size: u32,
        format: TextureFormat,
        label: &str,
    ) -> bool {
        let bpp: u32 = match format {
            TextureFormat::R8Unorm => 1,
            TextureFormat::Bgra8Unorm => 4,
            _ => panic!("unsupported atlas format"),
        };
        let tex_size = Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        };

        // Only allocate a new texture when the size changes.
        let need_new = existing
            .as_ref()
            .map_or(true, |t| t.width() != size || t.height() != size);

        if need_new {
            *existing = Some(device.create_texture(&TextureDescriptor {
                label: Some(label),
                size: tex_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            }));
        }

        queue.write_texture(
            TexelCopyTextureInfo {
                texture: existing.as_ref().unwrap(),
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * bpp),
                rows_per_image: None,
            },
            tex_size,
        );
        need_new
    }

    fn upload_atlas_grayscale(&mut self, data: &[u8], size: u32) {
        let resized = Self::write_atlas(
            &self.device, &self.queue,
            &mut self.atlas_grayscale_texture,
            data, size, TextureFormat::R8Unorm, "atlas_grayscale",
        );
        if resized {
            self.atlas_bind_group = None;
        }
    }

    fn upload_atlas_color(&mut self, data: &[u8], size: u32) {
        let resized = Self::write_atlas(
            &self.device, &self.queue,
            &mut self.atlas_color_texture,
            data, size, TextureFormat::Bgra8Unorm, "atlas_color",
        );
        if resized {
            self.atlas_bind_group = None;
        }
    }

    fn ensure_atlas_bind_group(&mut self) {
        if self.atlas_bind_group.is_some() {
            return;
        }

        // Create 1x1 placeholder textures if atlas hasn't been uploaded yet
        let grayscale_view = match &self.atlas_grayscale_texture {
            Some(t) => t.create_view(&TextureViewDescriptor::default()),
            None => {
                let t = self.device.create_texture(&TextureDescriptor {
                    label: Some("placeholder_grayscale"),
                    size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::R8Unorm,
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                t.create_view(&TextureViewDescriptor::default())
            }
        };

        let color_view = match &self.atlas_color_texture {
            Some(t) => t.create_view(&TextureViewDescriptor::default()),
            None => {
                let t = self.device.create_texture(&TextureDescriptor {
                    label: Some("placeholder_color"),
                    size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Bgra8Unorm,
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                t.create_view(&TextureViewDescriptor::default())
            }
        };

        self.atlas_bind_group = Some(self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("atlas_bg"),
            layout: &self.pipelines.atlas_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&grayscale_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&color_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&self.atlas_sampler),
                },
            ],
        }));
    }
}

