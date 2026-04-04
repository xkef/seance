//! Render pipeline construction for the cell shaders.

use wgpu::*;

use crate::uniforms::Uniforms;

/// All render pipelines and bind group layouts used by the renderer.
pub struct Pipelines {
    pub bg_color: RenderPipeline,
    pub cell_bg: RenderPipeline,
    pub cell_text: RenderPipeline,
    pub uniform_bgl: BindGroupLayout,
    pub bg_cells_bgl: BindGroupLayout,
    pub atlas_bgl: BindGroupLayout,
}

impl Pipelines {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("cell_shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/cell.wgsl").into()),
        });

        // Bind group layout 0: uniforms (shared by all passes)
        let uniform_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("uniform_bgl"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(
                        std::num::NonZero::new(size_of::<Uniforms>() as u64).unwrap(),
                    ),
                },
                count: None,
            }],
        });

        // Bind group layout 1: cell backgrounds (storage buffer)
        let bg_cells_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("bg_cells_bgl"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Bind group layout 2: atlas textures
        let atlas_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("atlas_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bg_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("bg_layout"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });

        let cell_bg_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("cell_bg_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&bg_cells_bgl)],
            immediate_size: 0,
        });

        let cell_text_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("cell_text_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&bg_cells_bgl), Some(&atlas_bgl)],
            immediate_size: 0,
        });

        let blend_premultiplied = BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
        };

        let bg_color = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("bg_color"),
            layout: Some(&bg_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_bg_color"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cell_bg = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("cell_bg"),
            layout: Some(&cell_bg_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_cell_bg"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(blend_premultiplied),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Instance buffer layout for CellText (32 bytes per instance)
        let cell_text_instance_layout = VertexBufferLayout {
            array_stride: 32,
            step_mode: VertexStepMode::Instance,
            attributes: &[
                // glyph_pos: vec2<u32> at offset 0
                VertexAttribute {
                    format: VertexFormat::Uint32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // glyph_size: vec2<u32> at offset 8
                VertexAttribute {
                    format: VertexFormat::Uint32x2,
                    offset: 8,
                    shader_location: 1,
                },
                // bearings: [2]i16 at offset 16 -> vec2<i32> in shader
                VertexAttribute {
                    format: VertexFormat::Sint16x2,
                    offset: 16,
                    shader_location: 2,
                },
                // grid_pos: [2]u16 at offset 20 -> vec2<u32> in shader
                VertexAttribute {
                    format: VertexFormat::Uint16x2,
                    offset: 20,
                    shader_location: 3,
                },
                // color: vec4<f32> at offset 24 (u8x4 -> unorm)
                VertexAttribute {
                    format: VertexFormat::Unorm8x4,
                    offset: 24,
                    shader_location: 4,
                },
                // atlas + flags: u32 at offset 28 (u8 + u8 packed)
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 28,
                    shader_location: 5,
                },
            ],
        };

        let cell_text = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("cell_text"),
            layout: Some(&cell_text_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_cell_text"),
                buffers: &[cell_text_instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_cell_text"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(blend_premultiplied),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            bg_color,
            cell_bg,
            cell_text,
            uniform_bgl,
            bg_cells_bgl,
            atlas_bgl,
        }
    }
}
