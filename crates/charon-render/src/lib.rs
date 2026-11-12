//! GPU の初期化とサーフェスのライフサイクル (仕様 06 §1〜§2、§7)。
//!
//! ウィンドウの作成は呼び出し側の責務。`resumed` で安全な `SurfaceTarget` を渡し、
//! `suspended` でサーフェスだけを破棄する。デバイスとキューは保持する。

mod background_pass;
mod cell_pass;
mod cells;
mod composite;
mod deep_background;
mod entity_pass;
mod layers;
mod light_pass;
mod passes;
mod smooth_body;

pub use background_pass::{BackgroundLayerPass, BackgroundSource};
pub use cell_pass::{CellPass, CellPassError, CellView};
pub use cells::{PaletteBytes, cell_to_texel, palette_bytes};
pub use composite::{CompositePass, DEFAULT_AMBIENT};
pub use deep_background::{DeepBackgroundClock, DeepBackgroundParams, DeepBackgroundPass};
pub use entity_pass::{EntityPass, EntitySprite};
pub use layers::LayerAllocator;
pub use light_pass::LightPass;
pub use passes::{
    BackgroundPass, FrameContext, FullscreenPass, LightQuality, PassList, RenderPass,
};
pub use smooth_body::{SmoothBodyPass, SmoothBodySprite};

/// GPU またはサーフェスの初期化エラー。
#[derive(Debug)]
pub enum GpuError {
    Surface(wgpu::CreateSurfaceError),
    Adapter(wgpu::RequestAdapterError),
    Device(wgpu::RequestDeviceError),
    UnsupportedSurface,
    UnsupportedSrgb,
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(err) => write!(f, "サーフェスの作成に失敗: {err}"),
            Self::Adapter(err) => write!(f, "GPU アダプターの取得に失敗: {err}"),
            Self::Device(err) => write!(f, "GPU デバイスの取得に失敗: {err}"),
            Self::UnsupportedSurface => write!(f, "このアダプターはサーフェスに対応していません"),
            Self::UnsupportedSrgb => write!(f, "sRGB のサーフェス形式がありません"),
        }
    }
}

impl std::error::Error for GpuError {}

/// アダプター、デバイス、キューを保持し、サーフェスを再作成できる GPU コンテキスト。
#[derive(Debug)]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
}

impl GpuContext {
    /// ウィンドウを作成した後に呼ぶ。`descriptor` にウィンドウと同じ display handle を
    /// 指定できるため、GLES を含むバックエンドを wgpu に選ばせられる。
    pub async fn new(
        mut descriptor: wgpu::InstanceDescriptor,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, GpuError> {
        descriptor.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(target).map_err(GpuError::Surface)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(GpuError::Adapter)?;
        let limits = requested_limits(&adapter.limits());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(GpuError::Device)?;
        let config = configure_surface(&surface, &adapter, &device, width, height)?;
        Ok(Self { instance, adapter, device, queue, surface: Some(surface), config })
    }

    /// ウィンドウなしの GPU。描画テストや画像の書き出しに使い、サーフェスは持たない。
    /// 使えるアダプターがなければ `GpuError::Adapter` を返す。
    pub async fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::default();
        let adapter =
            instance.request_adapter(&Default::default()).await.map_err(GpuError::Adapter)?;
        let limits = requested_limits(&adapter.limits());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(GpuError::Device)?;
        Ok(Self { instance, adapter, device, queue, surface: None, config: None })
    }

    /// `suspended` でサーフェスだけを手放す。デバイスとキューは維持する。
    pub fn suspend(&mut self) {
        self.surface = None;
        self.config = None;
    }

    /// `resumed` で新しいウィンドウからサーフェスを作り直す。
    pub fn recreate_surface(
        &mut self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        let surface = self.instance.create_surface(target).map_err(GpuError::Surface)?;
        let config = configure_surface(&surface, &self.adapter, &self.device, width, height)?;
        self.surface = Some(surface);
        self.config = config;
        Ok(())
    }

    /// ウィンドウのサイズを変更する。最小化中の 0 サイズでは configure しない。
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        self.config = match &self.surface {
            Some(surface) => {
                configure_surface(surface, &self.adapter, &self.device, width, height)?
            }
            None => None,
        };
        Ok(())
    }

    pub fn surface(&self) -> Option<&wgpu::Surface<'static>> {
        self.surface.as_ref()
    }

    pub fn surface_config(&self) -> Option<&wgpu::SurfaceConfiguration> {
        self.config.as_ref()
    }
}

fn requested_limits(adapter: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits {
        max_texture_dimension_2d: adapter.max_texture_dimension_2d,
        max_texture_array_layers: adapter.max_texture_array_layers,
        ..wgpu::Limits::downlevel_webgl2_defaults()
    }
}

fn srgb_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats.iter().copied().find(wgpu::TextureFormat::is_srgb)
}

fn configure_surface(
    surface: &wgpu::Surface<'static>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<Option<wgpu::SurfaceConfiguration>, GpuError> {
    let mut config =
        surface.get_default_config(adapter, width, height).ok_or(GpuError::UnsupportedSurface)?;
    config.format =
        srgb_format(&surface.get_capabilities(adapter).formats).ok_or(GpuError::UnsupportedSrgb)?;
    if width == 0 || height == 0 {
        return Ok(None);
    }
    surface.configure(device, &config);
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::cell::MaterialId;
    use charon_core::chunk::Rect;
    use charon_core::coords::{CHUNK_AREA, ChunkPos};

    #[derive(Debug)]
    struct TestBackground {
        cells: [MaterialId; CHUNK_AREA],
        dirty: Option<Rect>,
    }

    impl BackgroundSource for TestBackground {
        fn chunk(&self, pos: ChunkPos) -> Option<&[MaterialId; CHUNK_AREA]> {
            (pos == ChunkPos::new(0, 0)).then_some(&self.cells)
        }

        fn take_render_dirty(&mut self, _pos: ChunkPos) -> Option<Rect> {
            self.dirty.take()
        }
    }

    #[test]
    fn shaders_parse() {
        for source in [
            include_str!("chunk.wgsl"),
            include_str!("background.wgsl"),
            include_str!("entity.wgsl"),
            include_str!("present.wgsl"),
            include_str!("light.wgsl"),
            include_str!("composite.wgsl"),
            include_str!("bloom.wgsl"),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(source).unwrap();
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap();
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)] // GPU が存在すれば失敗をその場で検出する。
    fn light_pass_renders_offscreen() {
        let instance = wgpu::Instance::default();
        let required_adapter = std::env::var("CHARON_REQUIRED_GPU_ADAPTER").ok();
        let capture_path = std::env::var_os("CHARON_GPU_CAPTURE_PATH");
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            assert!(
                required_adapter.is_none() && capture_path.is_none(),
                "指定した GPU アダプターを取得できません"
            );
            return; // GPU のない CI では WGSL 構文テストが動く。
        };
        let info = adapter.get_info();
        if let Some(name) = required_adapter.as_deref() {
            assert!(
                info.name.contains(name) && info.device_type == wgpu::DeviceType::DiscreteGpu,
                "離散 GPU {name:?} が必要ですが、選択されたアダプターは {info:?} です"
            );
        }
        println!("GPU adapter: {info:?}");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: requested_limits(&adapter.limits()),
            ..Default::default()
        }))
        .unwrap();
        let ctx = GpuContext { instance, adapter, device, queue, surface: None, config: None };
        let materials = charon_core::material::MaterialTable::builtin().unwrap();
        let mut passes = PassList::new(&ctx, 256, 256, 4);
        passes.add(BackgroundPass::new(wgpu::Color::WHITE));
        passes.add(CellPass::new(&ctx, &materials, PassList::scene_format(), 4).unwrap());
        passes.add(BackgroundLayerPass::new(&ctx, &materials, 4).unwrap());
        passes.add(EntityPass::new(&ctx));
        passes.add(LightPass::new(&ctx, passes.scene_size(), passes.light_format()));
        passes.add(CompositePass::new(&ctx));
        passes.add(FullscreenPass::post_process(&ctx, include_str!("bloom.wgsl")));
        assert_eq!(
            passes.names().collect::<Vec<_>>(),
            [
                "background",
                "cells",
                "background_layer",
                "entities",
                "light",
                "composite",
                "postprocess"
            ]
        );
        let output = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("charon light test output"),
            size: wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut world =
            charon_core::testkit::world_from_ascii(&charon_core::testkit::box_art(64, 64), 1);
        charon_core::testkit::fill(&mut world, 0, 8, 63, 8, '#');
        charon_core::testkit::place(&mut world, 5, 3, 's');
        let mut background = TestBackground { cells: [MaterialId::EMPTY; CHUNK_AREA], dirty: None };
        background.cells[10 * 64 + 10] = world.materials().id("wood").unwrap();
        background.dirty = Some(Rect::point(10, 10));
        let view = output.create_view(&Default::default());
        for quality in [LightQuality::Off, LightQuality::Low, LightQuality::High] {
            passes.set_light_quality(quality);
            passes
                .render_with_game(
                    &ctx,
                    &mut world,
                    Some(&mut background),
                    &[EntitySprite { x16: 15 * 16, y16: 15 * 16, width: 8, height: 16 }],
                    &[ChunkPos::new(0, 0)],
                    &view,
                    [0.0, 0.0],
                )
                .unwrap();
        }
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon light readback"),
            size: 256 * 256 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256 * 4),
                    rows_per_image: Some(256),
                },
            },
            wgpu::Extent3d { width: 256, height: 256, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let pixels = buffer.slice(..).get_mapped_range().unwrap();
        let top = pixels[(4 * 256 + 128) * 4];
        let cave = pixels[(80 * 256 + 128) * 4];
        assert!(top > cave, "sky={top}, cave={cave}");
        let wood = pixels[(40 * 256 + 40) * 4];
        let empty = pixels[(40 * 256 + 44) * 4];
        assert!(wood < empty, "background wood={wood}, empty={empty}");
        let entity = &pixels[(64 * 256 + 64) * 4..(64 * 256 + 64) * 4 + 4];
        let sky = &pixels[(64 * 256 + 96) * 4..(64 * 256 + 96) * 4 + 4];
        assert_ne!(entity, sky, "entity should cover the sky independently of cells");
        let sand = &pixels[(14 * 256 + 22) * 4..(14 * 256 + 22) * 4 + 4];
        let adjacent_sky = &pixels[(14 * 256 + 30) * 4..(14 * 256 + 30) * 4 + 4];
        assert_ne!(sand, adjacent_sky, "sand should be rendered independently of the sky");
        if let Some(path) = capture_path {
            std::fs::write(path, &pixels[..]).unwrap();
        }
    }

    #[test]
    fn srgb_format_is_required() {
        use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8UnormSrgb};
        assert_eq!(
            srgb_format(&[Bgra8Unorm, Bgra8UnormSrgb, Rgba8UnormSrgb]),
            Some(Bgra8UnormSrgb)
        );
        assert_eq!(srgb_format(&[Bgra8Unorm]), None);
    }

    #[test]
    fn downlevel_limits_only_override_texture_bounds() {
        let default = wgpu::Limits::downlevel_webgl2_defaults();
        let adapter = wgpu::Limits {
            max_texture_dimension_2d: 16384,
            max_texture_array_layers: 2048,
            ..wgpu::Limits::default()
        };
        let required = requested_limits(&adapter);
        assert_eq!(required.max_texture_dimension_2d, 16384);
        assert_eq!(required.max_texture_array_layers, 2048);
        assert_eq!(required.max_bind_groups, default.max_bind_groups);
        assert_eq!(
            required.max_storage_buffers_per_shader_stage,
            default.max_storage_buffers_per_shader_stage
        );
    }
}
