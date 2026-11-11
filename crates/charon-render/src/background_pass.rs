//! 仕様 07 §4: 背景セルを前景と独立したテクスチャ配列で描く。

use charon_core::cell::MaterialId;
use charon_core::chunk::Rect;
use charon_core::coords::{CHUNK_AREA, ChunkPos};
use charon_core::material::MaterialTable;

use crate::{
    CellPassError, CellView, FrameContext, GpuContext, LayerAllocator, RenderPass, palette_bytes,
};

/// 描画側は材質 ID と変更矩形だけを読み、保存形式には依存しない。
pub trait BackgroundSource: std::fmt::Debug {
    fn chunk(&self, pos: ChunkPos) -> Option<&[MaterialId; CHUNK_AREA]>;
    fn take_render_dirty(&mut self, pos: ChunkPos) -> Option<Rect>;
}

#[derive(Debug)]
pub struct BackgroundLayerPass {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    cells: wgpu::Texture,
    instances: wgpu::Buffer,
    camera: wgpu::Buffer,
    layers: LayerAllocator,
}

impl BackgroundLayerPass {
    pub fn new(
        ctx: &GpuContext,
        materials: &MaterialTable,
        capacity: u32,
    ) -> Result<Self, CellPassError> {
        if capacity == 0 || capacity > ctx.device.limits().max_texture_array_layers {
            return Err(CellPassError::TooManyLayers);
        }
        if materials.len() > 2048
            || materials.len() as u32 > ctx.device.limits().max_texture_dimension_2d
        {
            return Err(CellPassError::TooManyMaterials);
        }
        let cells = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("charon background layers"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: capacity },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("charon background palette"),
            size: wgpu::Extent3d {
                width: materials.len() as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &palette,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &palette_bytes(materials).color,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(materials.len() as u32 * 4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d { width: materials.len() as u32, height: 1, depth_or_array_layers: 1 },
        );
        let camera = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon background camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon background instances"),
            size: u64::from(capacity) * 12,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon background.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("background.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![0 => Sint32x2, 1 => Uint32];
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon background layer"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
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
            label: Some("charon background textures"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cells.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            ..Default::default()
                        },
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &palette.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry { binding: 2, resource: camera.as_entire_binding() },
            ],
        });
        Ok(Self {
            pipeline,
            bind_group,
            cells,
            instances,
            camera,
            layers: LayerAllocator::new(capacity),
        })
    }

    fn draw(
        &mut self,
        ctx: &GpuContext,
        source: &mut dyn BackgroundSource,
        visible: &[ChunkPos],
        target: &wgpu::TextureView,
        view: CellView,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), CellPassError> {
        for pos in self.layers.positions() {
            if source.chunk(pos).is_none()
                || !visible.iter().any(|v| (v.x - pos.x).abs() < 2 && (v.y - pos.y).abs() < 2)
            {
                self.layers.release(pos);
            }
        }
        let mut instances = Vec::with_capacity(visible.len() * 12);
        for &pos in visible {
            if source.chunk(pos).is_none() {
                continue;
            }
            let fresh = self.layers.layer(pos).is_none();
            let layer = self.layers.allocate(pos).ok_or(CellPassError::LayersFull)?;
            let rect = if fresh {
                source.take_render_dirty(pos);
                Some(Rect { min_x: 0, min_y: 0, max_x: 63, max_y: 63 })
            } else {
                source.take_render_dirty(pos)
            };
            if let Some(rect) = rect {
                self.upload(ctx, source, pos, layer, rect);
            }
            instances.extend_from_slice(&pos.x.to_le_bytes());
            instances.extend_from_slice(&pos.y.to_le_bytes());
            instances.extend_from_slice(&layer.to_le_bytes());
        }
        if instances.is_empty() {
            return Ok(());
        }
        ctx.queue.write_buffer(&self.instances, 0, &instances);
        let sx = 2.0 * view.zoom as f32 / view.viewport[0].max(1) as f32;
        let sy = -2.0 * view.zoom as f32 / view.viewport[1].max(1) as f32;
        let camera = [
            sx,
            0.0,
            0.0,
            0.0,
            0.0,
            sy,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            -1.0 - view.origin[0] * sx,
            1.0 - view.origin[1] * sy,
            0.0,
            1.0,
        ];
        let mut bytes = Vec::with_capacity(64);
        for value in camera {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        ctx.queue.write_buffer(&self.camera, 0, &bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon background layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..(instances.len() / 12) as u32);
        Ok(())
    }

    fn upload(
        &self,
        ctx: &GpuContext,
        source: &dyn BackgroundSource,
        pos: ChunkPos,
        layer: u32,
        rect: Rect,
    ) {
        let Some(chunk) = source.chunk(pos) else { return };
        let width = u32::from(rect.max_x - rect.min_x) + 1;
        let height = u32::from(rect.max_y - rect.min_y) + 1;
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in rect.min_y..=rect.max_y {
            for x in rect.min_x..=rect.max_x {
                let id = chunk[usize::from(y) * 64 + usize::from(x)].0;
                bytes.extend_from_slice(&[(id & 255) as u8, (id >> 8) as u8, 0, 0]);
            }
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.cells,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: u32::from(rect.min_x),
                    y: u32::from(rect.min_y),
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}

impl RenderPass for BackgroundLayerPass {
    fn name(&self) -> &str {
        "background_layer"
    }
    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}
    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let Some(background) = frame.background.as_deref_mut() else { return };
        frame.error = self
            .draw(ctx, background, frame.visible, frame.scene, frame.camera, frame.encoder)
            .err();
    }
}
