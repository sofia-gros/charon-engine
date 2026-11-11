//! 仕様 07 §2.2、§4: エンティティを前景セルと光の間に描く。

use crate::{FrameContext, GpuContext, RenderPass};

/// シミュレーションから描画へ渡す、セルに書き込まないプレイヤーの形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntitySprite {
    pub x16: i64,
    pub y16: i64,
    pub width: u8,
    pub height: u8,
}

#[derive(Debug)]
pub struct EntityPass {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    camera: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
}

impl EntityPass {
    pub fn new(ctx: &GpuContext) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon entity.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("entity.wgsl").into()),
        });
        let camera = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon entity camera"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon entities"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::PassList::scene_format(),
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon entity camera bind group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() }],
        });
        let capacity = 1;
        let instances = make_instances(ctx, capacity);
        Self { pipeline, bind_group, camera, instances, capacity }
    }
}

fn make_instances(ctx: &GpuContext, capacity: usize) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("charon entity instances"),
        size: (capacity * 16) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl RenderPass for EntityPass {
    fn name(&self) -> &str {
        "entities"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        if frame.entities.is_empty() {
            return;
        }
        if frame.entities.len() > self.capacity {
            self.capacity = frame.entities.len();
            self.instances = make_instances(ctx, self.capacity);
        }
        let mut bytes = Vec::with_capacity(frame.entities.len() * 16);
        for entity in frame.entities {
            for number in [
                entity.x16 as f32,
                entity.y16 as f32,
                f32::from(entity.width),
                f32::from(entity.height),
            ] {
                bytes.extend_from_slice(&number.to_le_bytes());
            }
        }
        ctx.queue.write_buffer(&self.instances, 0, &bytes);
        let mut camera = Vec::with_capacity(16);
        for number in [
            frame.camera.origin[0],
            frame.camera.origin[1],
            frame.camera.viewport[0] as f32,
            frame.camera.viewport[1] as f32,
        ] {
            camera.extend_from_slice(&number.to_le_bytes());
        }
        ctx.queue.write_buffer(&self.camera, 0, &camera);
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon entities"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.scene,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..frame.entities.len() as u32);
    }
}
