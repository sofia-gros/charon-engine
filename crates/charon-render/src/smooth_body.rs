//! 仕様 05 §11: 滑らかな剛体。画面の解像度で、正確な位置と角度で、縁を滑らかに描く。
//!
//! 合成 (光を掛ける) のあとに、合成結果へ重ねる。剛体のセルは `FLAG_SMOOTH` を付けてスタンプされ、
//! セルの描画では描かれない。物理・衝突・押しのけ・光の遮りと発光は、ドットの剛体と同じ。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{Cell, MaterialId};
use charon_core::material::MaterialTable;

use crate::{FrameContext, GpuContext, LightQuality, RenderPass};

/// 1 つの滑らかな剛体の、描画に必要な状態。`id` は剛体を見分ける番号 (描画のキャッシュのキー)。
#[derive(Clone, Copy, Debug)]
pub struct SmoothBodySprite<'a> {
    pub id: u64,
    pub width: u16,
    pub height: u16,
    /// 行優先のセル (`width * height` 個)。空のセルは材質が `EMPTY`。
    pub cells: &'a [Cell],
    /// 重心のワールド座標 (セル)。
    pub center: [f32; 2],
    /// 重心の、剛体のローカル座標 (セル、`PixelBody::origin`)。
    pub origin: [f32; 2],
    /// 回転 (ラジアン)。
    pub angle: f32,
}

/// テクスチャの 1 辺の上限 (縁を含む)。
const MAX_SIDE: u16 = 512;
const UNIFORM_BYTES: usize = 64;

fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(value: f32) -> u8 {
    let c = value.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// 1 セルの色。材質のパレットの色に、`shade` の変化 (`chunk.wgsl` と同じ式) を掛ける。
fn cell_color(cell: Cell, materials: &MaterialTable) -> Option<[u8; 4]> {
    if cell.material == MaterialId::EMPTY || usize::from(cell.material.0) >= materials.len() {
        return None;
    }
    let material = materials.get(cell.material);
    if material.color[3] == 0 {
        return None;
    }
    let variation = 1.0
        + (f32::from(cell.shade) / 255.0 - 0.5)
            * (f32::from(material.color_variation) / 255.0)
            * 8.0;
    let channel = |index: usize| linear_to_srgb(srgb_to_linear(material.color[index]) * variation);
    Some([channel(0), channel(1), channel(2), material.color[3]])
}

/// 縁に 1 セルの透明な余白を足した `(width + 2) × (height + 2)` の RGBA8 (sRGB、ストレートアルファ)。
/// 透明なセルの色は、隣り合う (8 近傍の) 不透明なセルの色の平均で埋める。バイリニア補間のとき、
/// 縁が黒く暗くならないようにするため。
pub(crate) fn sprite_texels(
    cells: &[Cell],
    width: usize,
    height: usize,
    materials: &MaterialTable,
) -> Vec<u8> {
    let (w, h) = (width + 2, height + 2);
    let mut solid: Vec<Option<[u8; 4]>> = vec![None; w * h];
    for y in 0..height {
        for x in 0..width {
            solid[(y + 1) * w + x + 1] = cell_color(cells[y * width + x], materials);
        }
    }
    let mut bytes = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            if let Some(color) = solid[y * w + x] {
                bytes[at..at + 4].copy_from_slice(&color);
                continue;
            }
            let (mut sum, mut count) = ([0u32; 3], 0u32);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if let Some(c) = solid[ny as usize * w + nx as usize] {
                        for i in 0..3 {
                            sum[i] += u32::from(c[i]);
                        }
                        count += 1;
                    }
                }
            }
            for i in 0..3 {
                // 隣に不透明なセルがなければ (count が 0)、黒のまま。アルファも 0 のまま。
                if let Some(average) = sum[i].checked_div(count) {
                    bytes[at + i] = average as u8;
                }
            }
        }
    }
    bytes
}

/// 材質と `shade` の内容のハッシュ (FNV-1a 64-bit)。変わったときだけテクスチャを作り直す。
pub(crate) fn content_hash(cells: &[Cell], width: u16, height: u16) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut feed = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    for byte in width.to_le_bytes().into_iter().chain(height.to_le_bytes()) {
        feed(byte);
    }
    for cell in cells {
        for byte in cell.material.0.to_le_bytes() {
            feed(byte);
        }
        feed(cell.shade);
    }
    hash
}

fn uniform_bytes(
    sprite: &SmoothBodySprite<'_>,
    camera_origin: [f32; 2],
    scene_size: [u32; 2],
    ambient: f32,
    lit: bool,
) -> [u8; UNIFORM_BYTES] {
    let (sin, cos) = sprite.angle.sin_cos();
    let floats = [
        camera_origin[0],
        camera_origin[1],
        scene_size[0] as f32,
        scene_size[1] as f32,
        sprite.center[0],
        sprite.center[1],
        sprite.origin[0],
        sprite.origin[1],
        cos,
        sin,
        f32::from(sprite.width),
        f32::from(sprite.height),
        ambient.clamp(0.0, 1.0),
        f32::from(u8::from(lit)),
        0.0,
        0.0,
    ];
    let mut bytes = [0u8; UNIFORM_BYTES];
    for (i, value) in floats.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[derive(Debug)]
struct BodyTexture {
    hash: u64,
    size: [u16; 2],
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
}

#[derive(Debug)]
pub struct SmoothBodyPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    materials: MaterialTable,
    ambient: f32,
    cache: BTreeMap<u64, BodyTexture>,
}

impl SmoothBodyPass {
    /// `ambient` は合成の環境光 (`CompositePass::with_ambient` と同じ値)。
    pub fn new(ctx: &GpuContext, materials: &MaterialTable, ambient: f32) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon smooth_body.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("smooth_body.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("charon smooth body inputs"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
            label: Some("charon smooth body layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // 合成結果 (後処理の入力) と同じ形式に描く。
        let format =
            ctx.surface_config().map_or(crate::PassList::scene_format(), |config| config.format);
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon smooth body"),
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
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            sampler: ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            materials: materials.clone(),
            ambient,
            cache: BTreeMap::new(),
        }
    }

    /// 内容が変わったときだけテクスチャを作り直して、キャッシュから返す。
    fn texture(&mut self, ctx: &GpuContext, sprite: &SmoothBodySprite<'_>) -> &BodyTexture {
        let hash = content_hash(sprite.cells, sprite.width, sprite.height);
        let size = [sprite.width, sprite.height];
        let stale = self
            .cache
            .get(&sprite.id)
            .is_none_or(|cached| cached.hash != hash || cached.size != size);
        if stale {
            let (w, h) = (u32::from(sprite.width) + 2, u32::from(sprite.height) + 2);
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("charon smooth body"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let bytes = sprite_texels(
                sprite.cells,
                usize::from(sprite.width),
                usize::from(sprite.height),
                &self.materials,
            );
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            let uniform = self.cache.remove(&sprite.id).map_or_else(
                || {
                    ctx.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("charon smooth body params"),
                        size: UNIFORM_BYTES as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                },
                |old| old.uniform,
            );
            self.cache.insert(
                sprite.id,
                BodyTexture { hash, size, view: texture.create_view(&Default::default()), uniform },
            );
        }
        &self.cache[&sprite.id]
    }
}

impl RenderPass for SmoothBodyPass {
    fn name(&self) -> &str {
        "smooth_body"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let mut used = BTreeSet::new();
        let mut binds = Vec::new();
        let lit = frame.light_quality != LightQuality::Off;
        for sprite in frame.smooth {
            let valid = sprite.width > 0
                && sprite.height > 0
                && sprite.width + 2 <= MAX_SIDE
                && sprite.height + 2 <= MAX_SIDE
                && sprite.cells.len() == usize::from(sprite.width) * usize::from(sprite.height)
                && sprite.center.iter().chain(&sprite.origin).all(|v| v.is_finite())
                && sprite.angle.is_finite();
            if !valid {
                continue;
            }
            used.insert(sprite.id);
            let ambient = self.ambient;
            self.texture(ctx, sprite);
            let cached = &self.cache[&sprite.id];
            ctx.queue.write_buffer(
                &cached.uniform,
                0,
                &uniform_bytes(sprite, frame.camera.origin, frame.camera.viewport, ambient, lit),
            );
            binds.push(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("charon smooth body bind"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&cached.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(frame.light[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: cached.uniform.as_entire_binding(),
                    },
                ],
            }));
        }
        // 使われなくなった剛体のテクスチャを捨てる。
        self.cache.retain(|id, _| used.contains(id));
        if binds.is_empty() {
            return;
        }
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon smooth body pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.post_scene,
                depth_slice: None,
                resolve_target: None,
                // 合成結果の上に重ねる。
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        for bind in &binds {
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> MaterialTable {
        MaterialTable::builtin().unwrap()
    }

    #[test]
    fn sprite_texels_are_dilated_and_padded() {
        let materials = table();
        let ice = materials.id("ice").unwrap();
        // 3×3 の中央 1 セルだけが氷。
        let mut cells = vec![Cell::default(); 9];
        cells[4] = Cell { material: ice, shade: 128, ..Cell::default() };
        let bytes = sprite_texels(&cells, 3, 3, &materials);
        assert_eq!(bytes.len(), 5 * 5 * 4, "縁に 1 セルの余白が付く");
        let at = |x: usize, y: usize| &bytes[(y * 5 + x) * 4..(y * 5 + x) * 4 + 4];
        let center = at(2, 2);
        assert_eq!(center[3], materials.get(ice).color[3], "不透明なセルは材質のアルファ");
        // 隣り合う透明なセルは、アルファ 0 で、色は不透明なセルの色で埋まる (縁が黒くならない)。
        for (x, y) in [(1, 1), (2, 1), (3, 3), (1, 2)] {
            let c = at(x, y);
            assert_eq!(c[3], 0, "({x}, {y}) は透明");
            assert_eq!(c[..3], center[..3], "({x}, {y}) の色は隣の色");
        }
        // 離れた透明なセルは黒のまま。
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
        assert_eq!(at(4, 4), [0, 0, 0, 0]);
    }

    #[test]
    fn sprite_color_follows_shade_variation() {
        let materials = table();
        let sand = materials.id("sand").unwrap();
        assert!(materials.get(sand).color_variation > 0, "砂は色のばらつきがある");
        let bright =
            cell_color(Cell { material: sand, shade: 255, ..Cell::default() }, &materials).unwrap();
        let dark =
            cell_color(Cell { material: sand, shade: 0, ..Cell::default() }, &materials).unwrap();
        assert!(bright[0] > dark[0] && bright[1] > dark[1], "shade が大きいほど明るい");
        assert_eq!(cell_color(Cell::default(), &materials), None, "空のセルは色がない");
        // 範囲外の材質 ID でも落ちない。
        assert_eq!(
            cell_color(Cell { material: MaterialId(60_000), ..Cell::default() }, &materials),
            None
        );
    }

    #[test]
    fn content_hash_changes_with_material_and_shade() {
        let materials = table();
        let (ice, sand) = (materials.id("ice").unwrap(), materials.id("sand").unwrap());
        let base = vec![Cell { material: ice, shade: 10, ..Cell::default() }; 4];
        let hash = content_hash(&base, 2, 2);
        assert_eq!(hash, content_hash(&base.clone(), 2, 2));
        let mut other = base.clone();
        other[3].shade = 11;
        assert_ne!(hash, content_hash(&other, 2, 2), "shade が変わる");
        other[3] = Cell { material: sand, shade: 10, ..Cell::default() };
        assert_ne!(hash, content_hash(&other, 2, 2), "材質が変わる");
        // 温度などは見た目に使わないので、ハッシュに影響しない。
        let mut warm = base.clone();
        warm[0].temp = 900;
        assert_eq!(hash, content_hash(&warm, 2, 2));
        assert_ne!(hash, content_hash(&base, 4, 1), "形が違う");
    }

    #[test]
    fn uniform_layout_matches_the_shader() {
        let cells = [Cell::default(); 6];
        let sprite = SmoothBodySprite {
            id: 1,
            width: 3,
            height: 2,
            cells: &cells,
            center: [10.5, -4.0],
            origin: [1.5, 1.0],
            angle: std::f32::consts::FRAC_PI_2,
        };
        let bytes = uniform_bytes(&sprite, [100.0, 50.0], [400, 225], 0.045, true);
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2), f(3)], [100.0, 50.0, 400.0, 225.0], "camera");
        assert_eq!([f(4), f(5), f(6), f(7)], [10.5, -4.0, 1.5, 1.0], "center");
        assert!(f(8).abs() < 1e-6 && (f(9) - 1.0).abs() < 1e-6, "cos・sin");
        assert_eq!([f(10), f(11)], [3.0, 2.0], "幅・高さ");
        assert_eq!([f(12), f(13)], [0.045, 1.0], "light");
        assert_eq!(bytes.len(), 64);
    }
}
