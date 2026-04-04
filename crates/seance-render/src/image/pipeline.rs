//! wgpu pipeline + bind group layouts for the image compositor pass.

use wgpu::*;

pub(crate) struct ImagePipeline {
    pipeline: RenderPipeline,
    image_bgl: BindGroupLayout,
}

impl ImagePipeline {
    pub(crate) fn new(
        device: &Device,
        format: TextureFormat,
        uniform_bgl: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("image_shader"),
            source: ShaderSource::Wgsl(include_str!("../gpu/shaders/image.wgsl").into()),
        });

        let image_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("image_bgl"),
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
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("image_layout"),
            bind_group_layouts: &[Some(uniform_bgl), Some(&image_bgl)],
            immediate_size: 0,
        });

        let instance_layout = VertexBufferLayout {
            array_stride: 32,
            step_mode: VertexStepMode::Instance,
            attributes: &[
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("image_pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_image"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_image"),
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
        });

        Self {
            pipeline,
            image_bgl,
        }
    }

    pub(crate) fn pipeline(&self) -> &RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn image_bgl(&self) -> BindGroupLayout {
        self.image_bgl.clone()
    }
}

fn premultiplied_blend() -> BlendState {
    let c = BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        operation: BlendOperation::Add,
    };
    BlendState { color: c, alpha: c }
}
