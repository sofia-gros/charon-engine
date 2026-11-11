//! 仕様 06 §4: 登録順に実行する描画パスと、低解像度のシーン・全画面転送。

use charon_core::{coords::ChunkPos, world::World};

use crate::{BackgroundSource, CellPassError, CellView, GpuContext};

const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// 仕様 06 §5: 光の品質。Off は光パスを実行しない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightQuality {
    Off,
    Low,
    #[default]
    High,
}

/// 1 フレームを構成するパスが共有する入力とコマンド記録先。
#[derive(Debug)]
pub struct FrameContext<'a, 'source> {
    pub world: &'a mut World,
    pub background: Option<&'a mut (dyn BackgroundSource + 'source)>,
    pub entities: &'a [crate::EntitySprite],
    pub visible: &'a [ChunkPos],
    pub scene: &'a wgpu::TextureView,
    /// ping-pong 光テクスチャ。偶数回の伝播後は 0 が最終結果。
    pub light: [&'a wgpu::TextureView; 2],
    pub light_quality: LightQuality,
    /// 合成と後処理の間に置く画面解像度の中間テクスチャ。
    pub post_scene: &'a wgpu::TextureView,
    pub output: &'a wgpu::TextureView,
    pub camera: CellView,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub error: Option<CellPassError>,
}

/// 登録した順番で描画する共通インターフェイス。
pub trait RenderPass {
    fn name(&self) -> &str;
    fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32);
    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>);
}

/// 背景を描いたシーンにセル等を重ね、最後の全画面パスで画面に転送する。
pub struct PassList {
    passes: Vec<Box<dyn RenderPass>>,
    scene: wgpu::Texture,
    scene_view: wgpu::TextureView,
    post_scene: wgpu::Texture,
    post_view: wgpu::TextureView,
    light: [wgpu::Texture; 2],
    light_views: [wgpu::TextureView; 2],
    light_format: wgpu::TextureFormat,
    quality: LightQuality,
    viewport: [u32; 2],
    scene_size: [u32; 2],
    zoom: u32,
}

impl std::fmt::Debug for PassList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassList")
            .field("passes", &self.names().collect::<Vec<_>>())
            .field("viewport", &self.viewport)
            .field("scene_size", &self.scene_size)
            .field("zoom", &self.zoom)
            .finish()
    }
}

impl PassList {
    pub fn new(ctx: &GpuContext, width: u32, height: u32, zoom: u32) -> Self {
        let zoom = zoom.max(1);
        let scene_size = scene_dimensions(width, height, zoom);
        let (scene, scene_view) = make_scene(ctx, scene_size);
        let (post_scene, post_view) = make_post_scene(ctx, [width.max(1), height.max(1)]);
        let light_format = light_format(ctx);
        let (light, light_views) = make_light(ctx, scene_size, light_format);
        Self {
            passes: Vec::new(),
            scene,
            scene_view,
            post_scene,
            post_view,
            light,
            light_views,
            light_format,
            quality: LightQuality::High,
            viewport: [width, height],
            scene_size,
            zoom,
        }
    }

    /// パスを追加する。並び替えはせず、この順序で毎フレーム実行する。
    pub fn add(&mut self, pass: impl RenderPass + 'static) {
        self.passes.push(Box::new(pass));
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.passes.iter().map(|pass| pass.name())
    }

    pub fn set_light_quality(&mut self, quality: LightQuality) {
        self.quality = quality;
    }

    pub fn set_zoom(&mut self, ctx: &GpuContext, zoom: u32) {
        self.zoom = zoom.max(1);
        self.resize(ctx, self.viewport[0], self.viewport[1]);
    }

    pub fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        if self.viewport != [width, height] {
            (self.post_scene, self.post_view) = make_post_scene(ctx, [width.max(1), height.max(1)]);
        }
        self.viewport = [width, height];
        let size = scene_dimensions(width, height, self.zoom);
        if size != self.scene_size {
            (self.scene, self.scene_view) = make_scene(ctx, size);
            (self.light, self.light_views) = make_light(ctx, size, self.light_format);
            self.scene_size = size;
        }
        for pass in &mut self.passes {
            pass.resize(ctx, width, height);
        }
    }

    pub fn render(
        &mut self,
        ctx: &GpuContext,
        world: &mut World,
        visible: &[ChunkPos],
        output: &wgpu::TextureView,
        origin: [f32; 2],
    ) -> Result<(), CellPassError> {
        self.render_with_background(ctx, world, None, visible, output, origin)
    }

    /// 背景のデータ供給元を指定する。描画クレートは保存クレートに依存しない。
    pub fn render_with_background(
        &mut self,
        ctx: &GpuContext,
        world: &mut World,
        background: Option<&mut dyn BackgroundSource>,
        visible: &[ChunkPos],
        output: &wgpu::TextureView,
        origin: [f32; 2],
    ) -> Result<(), CellPassError> {
        self.render_with_game(ctx, world, background, &[], visible, output, origin)
    }

    /// 背景とエンティティをセルとは独立して、登録順に描く。
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_game(
        &mut self,
        ctx: &GpuContext,
        world: &mut World,
        background: Option<&mut dyn BackgroundSource>,
        entities: &[crate::EntitySprite],
        visible: &[ChunkPos],
        output: &wgpu::TextureView,
        origin: [f32; 2],
    ) -> Result<(), CellPassError> {
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("charon frame"),
        });
        let mut frame = FrameContext {
            world,
            background,
            entities,
            visible,
            scene: &self.scene_view,
            light: [&self.light_views[0], &self.light_views[1]],
            light_quality: self.quality,
            post_scene: &self.post_view,
            output,
            camera: CellView { origin, zoom: 1, viewport: self.scene_size },
            encoder: &mut encoder,
            error: None,
        };
        for pass in &mut self.passes {
            pass.render(ctx, &mut frame);
            if let Some(error) = frame.error.take() {
                return Err(error);
            }
        }
        ctx.queue.submit([encoder.finish()]);
        Ok(())
    }

    pub fn scene_size(&self) -> [u32; 2] {
        self.scene_size
    }

    pub fn light_format(&self) -> wgpu::TextureFormat {
        self.light_format
    }

    pub fn scene_format() -> wgpu::TextureFormat {
        SCENE_FORMAT
    }
}

fn scene_dimensions(width: u32, height: u32, zoom: u32) -> [u32; 2] {
    [width.div_ceil(zoom).max(1), height.div_ceil(zoom).max(1)]
}

fn light_format(ctx: &GpuContext) -> wgpu::TextureFormat {
    let format = wgpu::TextureFormat::Rgba16Float;
    let features = ctx.adapter.get_texture_format_features(format);
    let required = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    if features.allowed_usages.contains(required)
        && features.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
    {
        format
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

fn make_light(
    ctx: &GpuContext,
    scene_size: [u32; 2],
    format: wgpu::TextureFormat,
) -> ([wgpu::Texture; 2], [wgpu::TextureView; 2]) {
    let size = wgpu::Extent3d {
        width: scene_size[0].div_ceil(4),
        height: scene_size[1].div_ceil(4),
        depth_or_array_layers: 1,
    };
    let textures = std::array::from_fn(|_| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("charon light ping-pong"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let views = std::array::from_fn(|index| textures[index].create_view(&Default::default()));
    (textures, views)
}

fn make_post_scene(ctx: &GpuContext, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
    let format = ctx.surface_config().map_or(SCENE_FORMAT, |config| config.format);
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("charon postprocess input"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn make_scene(ctx: &GpuContext, size: [u32; 2]) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("charon scene"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// シーンを描く前に背景色でクリアするパス。
#[derive(Debug)]
pub struct BackgroundPass {
    color: wgpu::Color,
}

impl BackgroundPass {
    pub fn new(color: wgpu::Color) -> Self {
        Self { color }
    }
}

impl RenderPass for BackgroundPass {
    fn name(&self) -> &str {
        "background"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, _ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let _pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon background"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.scene,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
}

/// シーンを最近傍で拡大する全画面三角形の共通部品。
/// シェーダーは group(0) の binding 0 に入力テクスチャ、binding 1 にサンプラーを使う。
#[derive(Debug)]
pub struct FullscreenPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    postprocess: bool,
}

impl FullscreenPass {
    pub fn new(ctx: &GpuContext, source: &str) -> Self {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon fullscreen shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("charon fullscreen layout"),
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
            ],
        });
        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("charon fullscreen pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let format = ctx.surface_config().map_or(SCENE_FORMAT, |config| config.format);
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon fullscreen"),
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, layout, sampler, postprocess: false }
    }

    /// 合成後の画面テクスチャを入力とする汎用後処理。効果は WGSL 1 ファイルで指定する。
    pub fn post_process(ctx: &GpuContext, source: &str) -> Self {
        Self { postprocess: true, ..Self::new(ctx, source) }
    }
}

impl RenderPass for FullscreenPass {
    fn name(&self) -> &str {
        if self.postprocess { "postprocess" } else { "fullscreen" }
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon fullscreen input"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(if self.postprocess {
                        frame.post_scene
                    } else {
                        frame.scene
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon fullscreen"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_uses_cell_resolution() {
        assert_eq!(scene_dimensions(1024, 768, 4), [256, 192]);
        assert_eq!(scene_dimensions(101, 65, 4), [26, 17]);
        assert_eq!(scene_dimensions(0, 0, 4), [1, 1]);
    }
}
