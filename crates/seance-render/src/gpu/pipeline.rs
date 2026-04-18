//! Render pipelines and bind group layouts for the cell shaders.

use wgpu::*;

use super::uniforms::Uniforms;

const CELL_TEXT_INSTANCE_STRIDE: u64 = 32;

pub(crate) struct Pipelines {
    pub(crate) bg_color: RenderPipeline,
    pub(crate) cell_bg: RenderPipeline,
    pub(crate) cell_text: RenderPipeline,
    pub(crate) uniform_bgl: BindGroupLayout,
    pub(crate) bg_cells_bgl: BindGroupLayout,
    pub(crate) atlas_bgl: BindGroupLayout,
}

impl Pipelines {
    pub(crate) fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("cell_shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/cell.wgsl").into()),
        });

        let uniform_bgl = uniform_bind_group_layout(device);
        let bg_cells_bgl = bg_cells_bind_group_layout(device);
        let atlas_bgl = atlas_bind_group_layout(device);

        let bg_color = make_fullscreen_pipeline(
            device,
            &shader,
            format,
            "bg_color",
            "fs_bg_color",
            None,
            &[&uniform_bgl],
        );
        let cell_bg = make_fullscreen_pipeline(
            device,
            &shader,
            format,
            "cell_bg",
            "fs_cell_bg",
            Some(premultiplied_blend()),
            &[&uniform_bgl, &bg_cells_bgl],
        );
        let cell_text = make_cell_text_pipeline(
            device,
            &shader,
            format,
            &[&uniform_bgl, &bg_cells_bgl, &atlas_bgl],
        );

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

fn uniform_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
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
    })
}

fn bg_cells_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("bg_cells_bgl"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

fn atlas_bind_group_layout(device: &Device) -> BindGroupLayout {
    let texture_entry = |binding: u32| BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("atlas_bgl"),
        entries: &[
            texture_entry(0),
            texture_entry(1),
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn premultiplied_blend() -> BlendState {
    let c = BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        operation: BlendOperation::Add,
    };
    BlendState { color: c, alpha: c }
}

/// A pipeline that draws a single fullscreen triangle.
fn make_fullscreen_pipeline(
    device: &Device,
    shader: &ShaderModule,
    format: TextureFormat,
    label: &str,
    fs_entry: &str,
    blend: Option<BlendState>,
    bgls: &[&BindGroupLayout],
) -> RenderPipeline {
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &bgls.iter().map(|l| Some(*l)).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            targets: &[Some(ColorTargetState {
                format,
                blend,
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
    })
}

fn make_cell_text_pipeline(
    device: &Device,
    shader: &ShaderModule,
    format: TextureFormat,
    bgls: &[&BindGroupLayout],
) -> RenderPipeline {
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("cell_text_layout"),
        bind_group_layouts: &bgls.iter().map(|l| Some(*l)).collect::<Vec<_>>(),
        immediate_size: 0,
    });

    let instance_layout = VertexBufferLayout {
        array_stride: CELL_TEXT_INSTANCE_STRIDE,
        step_mode: VertexStepMode::Instance,
        attributes: &[
            VertexAttribute { format: VertexFormat::Uint32x2, offset: 0, shader_location: 0 },
            VertexAttribute { format: VertexFormat::Uint32x2, offset: 8, shader_location: 1 },
            VertexAttribute { format: VertexFormat::Sint16x2, offset: 16, shader_location: 2 },
            VertexAttribute { format: VertexFormat::Uint16x2, offset: 20, shader_location: 3 },
            VertexAttribute { format: VertexFormat::Unorm8x4, offset: 24, shader_location: 4 },
            VertexAttribute { format: VertexFormat::Uint32, offset: 28, shader_location: 5 },
        ],
    };

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("cell_text"),
        layout: Some(&layout),
        vertex: VertexState {
            module: shader,
            entry_point: Some("vs_cell_text"),
            buffers: &[instance_layout],
            compilation_options: Default::default(),
        },
        fragment: Some(FragmentState {
            module: shader,
            entry_point: Some("fs_cell_text"),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(premultiplied_blend()),
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
    })
}
