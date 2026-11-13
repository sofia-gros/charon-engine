//! 仕様 06 §10: スプライト (画像) の描画。**拡大率 `scale` (画像の 1 画素あたりのセル数) を指定できる**。
//!
//! 例: キャラクターを 32×64 の画像で作り、`scale = 10 / 64` で 5×10 セルの大きさに描く。大きさを変えたいときは
//! `scale` を変えるだけでよい (画像は作り直さない)。合成 (光を掛ける) のあと、後処理の前に重ねる。
//! 位置は、画像上の基準点 `pivot` (足元など) が、ワールドの `position` に来るように置く。

use std::collections::{BTreeMap, BTreeSet};

use crate::{FrameContext, GpuContext, LightQuality, RenderPass};

/// 拡大したときの画素の補間。ドット絵は `Nearest`、滑らかな絵は `Linear`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpriteFilter {
    #[default]
    Nearest,
    Linear,
}

/// 描く画像 1 枚。`id` は画像を見分ける番号 (描画のキャッシュのキー)。同じ `id` で画素が変わると作り直す。
#[derive(Clone, Copy, Debug)]
pub struct Sprite<'a> {
    pub id: u64,
    pub width: u16,
    pub height: u16,
    /// 行優先の RGBA8 (sRGB、ストレートアルファ)。長さは `width * height * 4`。
    pub rgba: &'a [u8],
    /// 基準点のワールド座標 (セル)。
    pub position: [f32; 2],
    /// 基準点の画像上の位置 (画素)。足元の中央なら `[width / 2, height]`。
    pub pivot: [f32; 2],
    /// 画像の 1 画素あたりのセル数。0 より大きい有限の値。
    pub scale: f32,
    /// 回転 (ラジアン、基準点まわり)。
    pub angle: f32,
    pub flip_x: bool,
    pub filter: SpriteFilter,
    /// 光を掛けるか。
    pub lit: bool,
    /// 画像の一部だけを描くコマ `[x, y, 幅, 高さ]` (画素)。スプライトシートのアニメーションに使う。
    /// `None` なら画像の全体。`pivot` はコマの中の位置。
    pub frame: Option<[u16; 4]>,
}

/// 画像の 1 辺の上限 (画素)。
const MAX_SIDE: u16 = 2048;
const UNIFORM_BYTES: usize = 80;

impl Sprite<'_> {
    /// 描くコマ `[x, y, 幅, 高さ]` (画素)。`frame` がなければ画像の全体。
    pub fn frame_rect(&self) -> [u16; 4] {
        self.frame.unwrap_or([0, 0, self.width, self.height])
    }

    /// 描いてよい値か (大きさ・長さ・有限の数)。コマは画像の中に収まっていること。
    fn valid(&self) -> bool {
        let [fx, fy, fw, fh] = self.frame_rect();
        let inside = fw > 0
            && fh > 0
            && u32::from(fx) + u32::from(fw) <= u32::from(self.width)
            && u32::from(fy) + u32::from(fh) <= u32::from(self.height);
        inside
            && self.width > 0
            && self.height > 0
            && self.width <= MAX_SIDE
            && self.height <= MAX_SIDE
            && self.rgba.len() == usize::from(self.width) * usize::from(self.height) * 4
            && self.scale.is_finite()
            && self.scale > 0.0
            && self.angle.is_finite()
            && self.position.iter().chain(&self.pivot).all(|v| v.is_finite())
    }

    /// 画面に描かれる大きさ (セル)。`(幅, 高さ)`。
    pub fn size_in_cells(&self) -> [f32; 2] {
        let [_, _, w, h] = self.frame_rect();
        [f32::from(w) * self.scale, f32::from(h) * self.scale]
    }
}

/// 画素の内容のハッシュ (FNV-1a 64-bit)。変わったときだけテクスチャを作り直す。
pub(crate) fn content_hash(width: u16, height: u16, rgba: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut feed = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    for byte in width.to_le_bytes().into_iter().chain(height.to_le_bytes()) {
        feed(byte);
    }
    for &byte in rgba {
        feed(byte);
    }
    hash
}

fn uniform_bytes(
    sprite: &Sprite<'_>,
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
        sprite.position[0],
        sprite.position[1],
        sprite.pivot[0],
        sprite.pivot[1],
        f32::from(sprite.frame_rect()[2]),
        f32::from(sprite.frame_rect()[3]),
        sprite.scale,
        f32::from(u8::from(sprite.flip_x)),
        cos,
        sin,
        ambient.clamp(0.0, 1.0),
        f32::from(u8::from(lit)),
        f32::from(sprite.frame_rect()[0]),
        f32::from(sprite.frame_rect()[1]),
        f32::from(sprite.width),
        f32::from(sprite.height),
    ];
    let mut bytes = [0u8; UNIFORM_BYTES];
    for (i, value) in floats.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[derive(Debug)]
struct ImageTexture {
    hash: u64,
    size: [u16; 2],
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
}

#[derive(Debug)]
pub struct SpritePass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    light_sampler: wgpu::Sampler,
    ambient: f32,
    cache: BTreeMap<u64, ImageTexture>,
}

impl SpritePass {
    /// `ambient` は合成の環境光 (`CompositePass::with_ambient` と同じ値)。
    pub fn new(ctx: &GpuContext, ambient: f32) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon sprite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
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
            label: Some("charon sprite inputs"),
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
            label: Some("charon sprite layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let format =
            ctx.surface_config().map_or(crate::PassList::scene_format(), |config| config.format);
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon sprite"),
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
        let sampler = |filter| {
            ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        Self {
            pipeline,
            layout,
            nearest: sampler(wgpu::FilterMode::Nearest),
            linear: sampler(wgpu::FilterMode::Linear),
            light_sampler: sampler(wgpu::FilterMode::Linear),
            ambient,
            cache: BTreeMap::new(),
        }
    }

    /// 内容が変わったときだけテクスチャを作り直して、キャッシュに入れる。
    fn upload(&mut self, ctx: &GpuContext, sprite: &Sprite<'_>) {
        let hash = content_hash(sprite.width, sprite.height, sprite.rgba);
        let size = [sprite.width, sprite.height];
        let stale = self
            .cache
            .get(&sprite.id)
            .is_none_or(|cached| cached.hash != hash || cached.size != size);
        if !stale {
            return;
        }
        let (w, h) = (u32::from(sprite.width), u32::from(sprite.height));
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("charon sprite"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            sprite.rgba,
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
                    label: Some("charon sprite params"),
                    size: UNIFORM_BYTES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            },
            |old| old.uniform,
        );
        self.cache.insert(
            sprite.id,
            ImageTexture { hash, size, view: texture.create_view(&Default::default()), uniform },
        );
    }
}

impl RenderPass for SpritePass {
    fn name(&self) -> &str {
        "sprite"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let mut used = BTreeSet::new();
        let mut binds = Vec::new();
        let any_light = frame.light_quality != LightQuality::Off;
        for sprite in frame.sprites {
            if !sprite.valid() {
                continue;
            }
            used.insert(sprite.id);
            self.upload(ctx, sprite);
            let cached = &self.cache[&sprite.id];
            let lit = sprite.lit && any_light;
            ctx.queue.write_buffer(
                &cached.uniform,
                0,
                &uniform_bytes(
                    sprite,
                    frame.camera.origin,
                    frame.camera.viewport,
                    self.ambient,
                    lit,
                ),
            );
            let image_sampler = match sprite.filter {
                SpriteFilter::Nearest => &self.nearest,
                SpriteFilter::Linear => &self.linear,
            };
            binds.push(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("charon sprite bind"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&cached.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(image_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(frame.light[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.light_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: cached.uniform.as_entire_binding(),
                    },
                ],
            }));
        }
        // 使われなくなった画像のテクスチャを捨てる。
        self.cache.retain(|id, _| used.contains(id));
        if binds.is_empty() {
            return;
        }
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon sprite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.post_scene,
                depth_slice: None,
                resolve_target: None,
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

    fn sprite(rgba: &[u8]) -> Sprite<'_> {
        Sprite {
            id: 1,
            width: 2,
            height: 2,
            rgba,
            position: [10.0, 20.0],
            pivot: [1.0, 2.0],
            scale: 0.5,
            angle: 0.0,
            flip_x: false,
            filter: SpriteFilter::Nearest,
            lit: true,
            frame: None,
        }
    }

    #[test]
    fn scale_sets_the_size_in_cells() {
        let pixels = [0u8; 16];
        let mut s = sprite(&pixels);
        assert_eq!(s.size_in_cells(), [1.0, 1.0]);
        s.scale = 2.0;
        assert_eq!(s.size_in_cells(), [4.0, 4.0]);
    }

    /// 32×64 の画像を `scale = 10 / 64` で描くと 5×10 セル。
    #[test]
    fn a_32_by_64_character_scales_to_5_by_10_cells() {
        let pixels = vec![0u8; 32 * 64 * 4];
        let mut s = sprite(&pixels);
        s.width = 32;
        s.height = 64;
        s.scale = 10.0 / 64.0;
        let [w, h] = s.size_in_cells();
        assert!((w - 5.0).abs() < 1e-5 && (h - 10.0).abs() < 1e-5, "{w}x{h}");
    }

    #[test]
    fn invalid_sprites_are_rejected() {
        let pixels = [0u8; 16];
        assert!(sprite(&pixels).valid());
        let mut s = sprite(&pixels);
        s.scale = 0.0;
        assert!(!s.valid(), "拡大率 0");
        s.scale = f32::NAN;
        assert!(!s.valid(), "拡大率 NaN");
        let mut s = sprite(&pixels);
        s.rgba = &pixels[..15];
        assert!(!s.valid(), "画素の長さが合わない");
        let mut s = sprite(&pixels);
        s.width = 0;
        assert!(!s.valid(), "幅 0");
        let mut s = sprite(&pixels);
        s.position = [f32::INFINITY, 0.0];
        assert!(!s.valid(), "位置が有限でない");
    }

    /// コマ (スプライトシートの一部) の大きさで描き、画像の外にはみ出すコマは描かない。
    #[test]
    fn frames_select_part_of_the_sheet() {
        let pixels = vec![0u8; 64 * 32 * 4];
        let mut s = sprite(&pixels);
        s.width = 64;
        s.height = 32;
        s.scale = 0.5;
        s.frame = Some([32, 0, 32, 32]);
        assert!(s.valid());
        assert_eq!(s.frame_rect(), [32, 0, 32, 32]);
        assert_eq!(s.size_in_cells(), [16.0, 16.0], "コマの大きさ × scale");
        s.frame = Some([40, 0, 32, 32]);
        assert!(!s.valid(), "画像の外にはみ出す");
        s.frame = Some([0, 0, 0, 32]);
        assert!(!s.valid(), "幅 0");
    }

    #[test]
    fn content_hash_tracks_pixels_and_size() {
        let a = [1u8, 2, 3, 4];
        let b = [1u8, 2, 3, 5];
        assert_eq!(content_hash(1, 1, &a), content_hash(1, 1, &a));
        assert_ne!(content_hash(1, 1, &a), content_hash(1, 1, &b));
        assert_ne!(content_hash(1, 1, &a), content_hash(2, 1, &a));
    }
}
