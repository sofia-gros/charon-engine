//! セルテクスチャ配列と、全チャンクを 1 回で描くインスタンスパス (仕様 06 §3)。

use charon_core::chunk::Rect;
use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::world::World;

use crate::{FrameContext, GpuContext, LayerAllocator, RenderPass, cell_to_texel, palette_bytes};

#[derive(Debug)]
pub enum CellPassError {
    TooManyLayers,
    TooManyMaterials,
    LayersFull,
}

impl std::fmt::Display for CellPassError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyLayers => write!(f, "テクスチャ配列の層数が GPU の上限を超えています"),
            Self::TooManyMaterials => write!(f, "パレットの材質数が上限を超えています"),
            Self::LayersFull => write!(f, "画面内のチャンクに割り当てる層が足りません"),
        }
    }
}
impl std::error::Error for CellPassError {}

/// セル描画に使うカメラの原点・整数拡大率・画面サイズ。
#[derive(Clone, Copy, Debug)]
pub struct CellView {
    pub origin: [f32; 2],
    pub zoom: u32,
    pub viewport: [u32; 2],
}

#[derive(Debug)]
pub struct CellPass {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    cells: wgpu::Texture,
    instance_buffer: wgpu::Buffer,
    camera_buffer: wgpu::Buffer,
    layers: LayerAllocator,
}

impl CellPass {
    /// `capacity` は画面内チャンク数に合わせる (通常 256、4K 等倍ならアダプターの上限まで)。
    pub fn new(
        ctx: &GpuContext,
        materials: &MaterialTable,
        output_format: wgpu::TextureFormat,
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
            label: Some("charon cell layers"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: capacity },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palettes = palette_bytes(materials);
        let make_palette = |label, format, bytes: &[u8]| {
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: materials.len() as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(materials.len() as u32 * 4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: materials.len() as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            texture
        };
        let palette =
            make_palette("charon palette", wgpu::TextureFormat::Rgba8UnormSrgb, &palettes.color);
        let aux =
            make_palette("charon palette aux", wgpu::TextureFormat::Rgba8Unorm, &palettes.aux);
        let camera_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon chunk instances"),
            size: capacity as u64 * 12,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon chunk.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("chunk.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![0 => Sint32x2, 1 => Uint32];
        let vertex_buffer = wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &attributes,
        };
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon cells"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_buffer)],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon cell textures"),
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &aux.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry { binding: 3, resource: camera_buffer.as_entire_binding() },
            ],
        });
        Ok(Self {
            pipeline,
            bind_group,
            cells,
            instance_buffer,
            camera_buffer,
            layers: LayerAllocator::new(capacity),
        })
    }

    /// 可視チャンクの変更部分だけを GPU に送り、1 回のインスタンス描画を記録する。
    /// `visible` は画面に交差するチャンクの位置を昇順に並べたもの。
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        world: &mut World,
        visible: &[ChunkPos],
        target: &wgpu::TextureView,
        view: CellView,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), CellPassError> {
        // 画面から 2 チャンク以上離れた層を返す。可視チャンクは画面に交差する範囲。
        for pos in self.layers.positions() {
            if world.chunk(pos).is_none()
                || !visible.iter().any(|v| (v.x - pos.x).abs() < 2 && (v.y - pos.y).abs() < 2)
            {
                self.layers.release(pos);
            }
        }
        let mut instances = Vec::with_capacity(visible.len() * 12);
        for &pos in visible {
            if world.chunk(pos).is_none() {
                continue;
            }
            let fresh = self.layers.layer(pos).is_none();
            let layer = self.layers.allocate(pos).ok_or(CellPassError::LayersFull)?;
            let rect = if fresh {
                world.take_render_dirty(pos);
                Some(Rect { min_x: 0, min_y: 0, max_x: 63, max_y: 63 })
            } else {
                world.take_render_dirty(pos)
            };
            if let Some(rect) = rect {
                self.upload(ctx, world, pos, layer, rect);
            }
            instances.extend_from_slice(&pos.x.to_le_bytes());
            instances.extend_from_slice(&pos.y.to_le_bytes());
            instances.extend_from_slice(&layer.to_le_bytes());
        }
        if !instances.is_empty() {
            ctx.queue.write_buffer(&self.instance_buffer, 0, &instances);
        }
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
        let mut camera_bytes = Vec::with_capacity(64);
        for value in camera {
            camera_bytes.extend_from_slice(&value.to_le_bytes());
        }
        ctx.queue.write_buffer(&self.camera_buffer, 0, &camera_bytes);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("charon cells"),
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
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            pass.draw(0..6, 0..(instances.len() / 12) as u32);
        }
        Ok(())
    }

    fn upload(&self, ctx: &GpuContext, world: &World, pos: ChunkPos, layer: u32, rect: Rect) {
        let Some(chunk) = world.chunk(pos) else { return };
        let width = u32::from(rect.max_x - rect.min_x) + 1;
        let height = u32::from(rect.max_y - rect.min_y) + 1;
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in rect.min_y..=rect.max_y {
            for x in rect.min_x..=rect.max_x {
                bytes.extend_from_slice(&cell_to_texel(chunk.get(x, y)));
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

impl RenderPass for CellPass {
    fn name(&self) -> &str {
        "cells"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        frame.error = CellPass::render(
            self,
            ctx,
            frame.world,
            frame.visible,
            frame.scene,
            frame.camera,
            frame.encoder,
        )
        .err();
    }
}
