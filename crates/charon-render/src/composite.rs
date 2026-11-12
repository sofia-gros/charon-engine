//! 仕様 06 §5: シーンとライトの合成。Off は元のシーンをそのまま表示する。

use crate::{FrameContext, GpuContext, LightQuality, RenderPass};

#[derive(Debug)]
pub struct CompositePass {
    off: wgpu::RenderPipeline,
    lit: wgpu::RenderPipeline,
    /// xHigh 用の、補間・発色を強めた合成。
    rich: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    ambient: wgpu::Buffer,
}

/// 光が届かない場所の明るさの既定値 (仕様 06 §5)。
pub const DEFAULT_AMBIENT: f32 = 0.12;

impl CompositePass {
    pub fn new(ctx: &GpuContext) -> Self {
        Self::with_ambient(ctx, DEFAULT_AMBIENT)
    }

    /// `ambient` (0.0..=1.0) は光が届かない場所の明るさ。暗い洞窟では小さくする。
    pub fn with_ambient(ctx: &GpuContext, ambient: f32) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon composite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("charon composite inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("charon composite layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let format =
            ctx.surface_config().map_or(crate::PassList::scene_format(), |config| config.format);
        let pipeline = |entry| {
            ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("charon composite"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let ambient_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon ambient"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&ambient.clamp(0.0, 1.0).to_le_bytes());
        ctx.queue.write_buffer(&ambient_buffer, 0, &bytes);
        Self {
            ambient: ambient_buffer,
            off: pipeline("fs_off"),
            lit: pipeline("fs_lit"),
            rich: pipeline("fs_lit_rich"),
            layout,
            nearest: ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            }),
            linear: ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }
}

impl RenderPass for CompositePass {
    fn name(&self) -> &str {
        "composite"
    }
    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}
    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon composite bind"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(frame.scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.nearest),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(frame.light[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.linear),
                },
                wgpu::BindGroupEntry { binding: 4, resource: self.ambient.as_entire_binding() },
            ],
        });
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.post_scene,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(match frame.light_quality {
            LightQuality::Off => &self.off,
            LightQuality::XHigh => &self.rich,
            LightQuality::Low | LightQuality::High => &self.lit,
        });
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
