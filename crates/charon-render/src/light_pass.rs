//! 仕様 06 §5: 光源の準備と 4 近傍のピンポン伝播。

use charon_core::cell::{Cell, FLAG_BODY, FLAG_BURNING, MaterialId};
use charon_core::chunk::Chunk;
use charon_core::coords::{CHUNK_MASK, CellPos, ChunkPos};
use charon_core::material::{MaterialTable, Phase};
use charon_core::world::World;

use crate::{FrameContext, GpuContext, LightQuality, RenderPass};

const SKY: [u8; 3] = [204, 220, 255];

/// 光源 RGB と、その地点での光の透過率。
fn source_cell(cell: Cell, materials: &MaterialTable, sky_visible: bool) -> [u8; 4] {
    let mat = materials.get(cell.material);
    let mut emission = mat.emission;
    if cell.has(FLAG_BURNING)
        && let Some(fire) = materials.fire
    {
        let flame = materials.get(fire).emission;
        for i in 0..3 {
            emission[i] = emission[i].max(flame[i]);
        }
    }
    if cell.material == MaterialId::EMPTY && sky_visible {
        emission = SKY;
    }
    let occlusion = if cell.has(FLAG_BODY) || matches!(mat.phase, Phase::Solid | Phase::Powder) {
        153 // 0.6
    } else if mat.phase == Phase::Liquid {
        230 // 0.9
    } else {
        255 // 1.0
    };
    [emission[0], emission[1], emission[2], occlusion]
}

/// 直前に引いたチャンクを覚えておく読み出し (近くを続けて読むとき、ハッシュ引きを省く)。
struct CellReader<'a> {
    world: &'a World,
    last: Option<(ChunkPos, Option<&'a Chunk>)>,
}

impl<'a> CellReader<'a> {
    fn new(world: &'a World) -> Self {
        Self { world, last: None }
    }

    fn get(&mut self, pos: CellPos) -> Cell {
        let key = pos.chunk();
        let chunk = match self.last {
            Some((k, chunk)) if k == key => chunk,
            _ => {
                let chunk = self.world.chunk(key);
                self.last = Some((key, chunk));
                chunk
            }
        };
        match chunk {
            Some(chunk) => chunk.get((pos.x & CHUNK_MASK) as u8, (pos.y & CHUNK_MASK) as u8),
            None => Cell::BOUNDARY,
        }
    }
}

fn sources(world: &World, origin: [f32; 2], scene: [u32; 2], divisor: u32) -> Vec<u8> {
    let d = divisor;
    let width = scene[0].div_ceil(d);
    let height = scene[1].div_ceil(d);
    let top = world
        .chunk_positions()
        .iter()
        .map(|pos| pos.origin().y)
        .min()
        .unwrap_or(origin[1].floor() as i32);
    let mut reader = CellReader::new(world);
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    let mut sky_open = vec![true; width as usize];
    let mut scanned_until = top;
    for y in 0..height {
        let wy = (origin[1].floor() as i32).saturating_add((y * d + d / 2) as i32);
        for x in 0..width {
            let wx = (origin[0].floor() as i32).saturating_add((x * d + d / 2) as i32);
            for above in scanned_until..wy {
                let cell = reader.get(CellPos::new(wx, above));
                if cell.has(FLAG_BODY)
                    || !matches!(
                        world.materials().get(cell.material).phase,
                        Phase::Empty | Phase::Gas | Phase::Liquid
                    )
                {
                    sky_open[x as usize] = false;
                    break;
                }
            }
            let center = reader.get(CellPos::new(wx, wy));
            let mut source = source_cell(center, world.materials(), sky_open[x as usize]);
            // 1 セル幅の火も四分の一解像度で失わない。
            for dy in 0..d {
                for dx in 0..d {
                    if y * d + dy >= scene[1] || x * d + dx >= scene[0] {
                        continue;
                    }
                    let pos = CellPos::new(
                        (origin[0].floor() as i32).saturating_add((x * d + dx) as i32),
                        (origin[1].floor() as i32).saturating_add((y * d + dy) as i32),
                    );
                    let emitted = source_cell(reader.get(pos), world.materials(), false);
                    for channel in 0..3 {
                        source[channel] = source[channel].max(emitted[channel]);
                    }
                }
            }
            bytes.extend_from_slice(&source);
        }
        scanned_until = wy.max(top);
    }
    bytes
}

fn texture(ctx: &GpuContext, scene: [u32; 2], divisor: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("charon light sources"),
        size: wgpu::Extent3d {
            width: scene[0].div_ceil(divisor),
            height: scene[1].div_ceil(divisor),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

#[derive(Debug)]
pub struct LightPass {
    source: wgpu::Texture,
    size: [u32; 2],
    /// `source` の解像度の割り算。
    divisor: u32,
    layout: wgpu::BindGroupLayout,
    prepare: wgpu::RenderPipeline,
    propagate: wgpu::RenderPipeline,
    /// xHigh (2 倍の解像度) 用。減衰を 1 セルあたりそろえる。
    propagate_fine: wgpu::RenderPipeline,
}

impl LightPass {
    /// デスクトップ 16 回、モバイル 8 回。両方偶数なので最終結果は `light[0]`。
    pub fn new(ctx: &GpuContext, scene_size: [u32; 2], format: wgpu::TextureFormat) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon light.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("light.wgsl").into()),
        });
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("charon light inputs"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }),
        });
        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("charon light pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("charon light"),
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
        Self {
            source: texture(ctx, scene_size, 4),
            size: scene_size,
            divisor: 4,
            layout,
            prepare: pipeline("fs_prepare"),
            propagate: pipeline("fs_propagate"),
            propagate_fine: pipeline("fs_propagate_fine"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        ctx: &GpuContext,
        frame: &mut FrameContext<'_, '_>,
        source: &wgpu::TextureView,
        previous: &wgpu::TextureView,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        _prepare: bool,
    ) {
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon light bind"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(previous),
                },
            ],
        });
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon light pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl RenderPass for LightPass {
    fn name(&self) -> &str {
        "light"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let iterations = match frame.light_quality {
            LightQuality::Off => return,
            LightQuality::Low => {
                if cfg!(target_os = "android") {
                    4
                } else {
                    8
                }
            }
            LightQuality::High => {
                if cfg!(target_os = "android") {
                    8
                } else {
                    16
                }
            }
            // 解像度が 2 倍なので、同じ距離まで届くには 2 倍の歩数が要る。
            LightQuality::XHigh => {
                if cfg!(target_os = "android") {
                    16
                } else {
                    32
                }
            }
        };
        let divisor = frame.light_quality.divisor();
        let scene = frame.camera.viewport;
        if self.size != scene || self.divisor != divisor {
            self.source = texture(ctx, scene, divisor);
            self.size = scene;
            self.divisor = divisor;
        }
        let size = wgpu::Extent3d {
            width: scene[0].div_ceil(divisor),
            height: scene[1].div_ceil(divisor),
            depth_or_array_layers: 1,
        };
        let bytes = sources(frame.world, frame.camera.origin, scene, divisor);
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
        let source = self.source.create_view(&Default::default());
        self.draw(ctx, frame, &source, &source, frame.light[0], &self.prepare, true);
        for iteration in 0..iterations {
            let from = (iteration % 2) as usize;
            let to = 1 - from;
            let pipeline = if divisor == 2 { &self.propagate_fine } else { &self.propagate };
            self.draw(ctx, frame, &source, frame.light[from], frame.light[to], pipeline, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::testkit::{box_art, fill, material_of, place, world_from_ascii};

    #[test]
    fn emission_and_cave_occlusion() {
        let mut world = world_from_ascii(&box_art(64, 64), 1);
        let lava = material_of(&world, 'l');
        let sample =
            source_cell(Cell { material: lava, ..Cell::default() }, world.materials(), false);
        assert!(sample[0] > 0 && sample[1] > 0);
        assert_eq!(source_cell(Cell::default(), world.materials(), true)[..3], SKY);
        assert_eq!(source_cell(Cell::default(), world.materials(), false)[..3], [0, 0, 0]);
        fill(&mut world, 0, 8, 63, 8, '#');
        let pixels = sources(&world, [0.0, 0.0], [64, 64], 4);
        let at = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 3];
        assert_eq!(at(8, 4), [0, 0, 0]);
        assert_eq!(at(8, 0), SKY);
        place(&mut world, 32, 16, 'l'); // 中央サンプル (34,18) から外れた 1 セルの光源。
        let pixels = sources(&world, [0.0, 0.0], [64, 64], 4);
        assert!(pixels[(4 * 16 + 8) * 4] > 0);
    }
}
