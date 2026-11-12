//! 仕様 09 §9: 背景のさらに奥で、視差で動く虹色の塊。
//!
//! 合成 (光を掛ける) のあとに、合成結果へ重ねる。光の影響を受けず、シーン (光を掛ける前) が
//! 「空」の画素にだけ描くので、前景・背景のセルの後ろに隠れる。後処理 (ブルーム) の前に置く。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{FrameContext, GpuContext, RenderPass};

/// 塊の見た目。長さはセル。格子の 1 区画に最大 1 つの塊があり、位置・大きさ・形は種から決まる。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeepBackgroundParams {
    /// 乱数の種。同じ種なら同じ塊の配置になる。
    pub seed: u32,
    /// 視差 (0..=1)。カメラの動きに対する、塊の動きの割合。小さいほど奥にある。
    pub parallax: f32,
    /// 格子の 1 区画の大きさ (視差を掛けたあとの座標で)。
    pub grid: f32,
    /// 塊のある区画の割合 (0..=1)。
    pub density: f32,
    pub min_radius: f32,
    pub max_radius: f32,
    /// 動き回る速さ・形がゆらぐ速さ (ラジアン / tick)。0 で動かない。
    pub drift: f32,
    /// 塊が動き回る範囲 (格子の大きさに対する割合、0..=0.45)。塊ごとに速さと向きが違い、
    /// 隣の区画の方へも行き来する。0 で、その場で形だけゆらぐ。
    pub roam: f32,
    /// 色の明るさの倍率。
    pub brightness: f32,
    /// シーンの「空」の色 (線形、`BackgroundPass` のクリア色)。この色の画素にだけ描く。
    pub empty: [f32; 3],
}

impl Default for DeepBackgroundParams {
    fn default() -> Self {
        Self {
            seed: 1,
            parallax: 0.5,
            grid: 128.0,
            density: 0.5,
            min_radius: 8.0,
            max_radius: 20.0,
            drift: 0.01,
            roam: 0.3,
            brightness: 1.0,
            empty: [0.0; 3],
        }
    }
}

/// 塊を動かす時刻 (tick)。描画パスとは別に、アプリが毎フレーム更新する。
#[derive(Clone, Debug, Default)]
pub struct DeepBackgroundClock(Arc<AtomicU32>);

impl DeepBackgroundClock {
    pub fn set_tick(&self, tick: u64) {
        // 長く動かしても止まらないよう、2^32 で折り返す。
        self.0.store(tick as u32, Ordering::Relaxed);
    }

    pub fn tick(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct DeepBackgroundPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    uniform: wgpu::Buffer,
    params: DeepBackgroundParams,
    clock: DeepBackgroundClock,
}

impl DeepBackgroundPass {
    /// 時刻を更新するための `DeepBackgroundClock` も返す。
    pub fn new(ctx: &GpuContext, params: DeepBackgroundParams) -> (Self, DeepBackgroundClock) {
        let shader = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("charon deep_background.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("deep_background.wgsl").into()),
        });
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("charon deep background inputs"),
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
            label: Some("charon deep background layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // 合成結果 (後処理の入力) と同じ形式に描く。
        let format =
            ctx.surface_config().map_or(crate::PassList::scene_format(), |config| config.format);
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("charon deep background"),
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
        let uniform = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("charon deep background params"),
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clock = DeepBackgroundClock::default();
        let pass = Self {
            pipeline,
            layout,
            nearest: ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            }),
            uniform,
            params,
            clock: clock.clone(),
        };
        (pass, clock)
    }
}

const UNIFORM_BYTES: usize = 96;

/// シェーダーの `Params` (vec4 が 6 つ) に並べる。
fn uniform_bytes(
    params: &DeepBackgroundParams,
    origin: [f32; 2],
    scene_size: [u32; 2],
    tick: u32,
) -> [u8; UNIFORM_BYTES] {
    let floats = [
        // camera
        origin[0],
        origin[1],
        scene_size[0] as f32,
        scene_size[1] as f32,
        // shape
        params.parallax.clamp(0.0, 1.0),
        params.grid.max(1.0),
        params.density.clamp(0.0, 1.0),
        tick as f32,
        // size
        params.min_radius.max(0.5),
        params.max_radius.max(params.min_radius).max(0.5),
        params.drift,
        params.brightness.max(0.0),
        // empty (rgb, 許容誤差)。8 bit の量子化 (暗い色では約 0.003) より大きくとる。
        params.empty[0],
        params.empty[1],
        params.empty[2],
        0.02,
    ];
    let mut bytes = [0u8; UNIFORM_BYTES];
    for (i, value) in floats.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[64..68].copy_from_slice(&params.seed.to_le_bytes());
    bytes[80..84].copy_from_slice(&params.roam.clamp(0.0, 0.45).to_le_bytes());
    bytes
}

impl RenderPass for DeepBackgroundPass {
    fn name(&self) -> &str {
        "deep_background"
    }

    fn resize(&mut self, _ctx: &GpuContext, _width: u32, _height: u32) {}

    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext<'_, '_>) {
        let bytes = uniform_bytes(
            &self.params,
            frame.camera.origin,
            frame.camera.viewport,
            self.clock.tick(),
        );
        ctx.queue.write_buffer(&self.uniform, 0, &bytes);
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("charon deep background bind"),
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
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            ],
        });
        let mut pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("charon deep background pass"),
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
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_matches_the_shader() {
        let params = DeepBackgroundParams { seed: 0xABCD, ..Default::default() };
        let bytes = uniform_bytes(&params, [10.0, -20.0], [400, 225], 7);
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(0), f(1), f(2), f(3)], [10.0, -20.0, 400.0, 225.0], "camera");
        assert_eq!([f(4), f(5), f(6), f(7)], [0.5, 128.0, 0.5, 7.0], "shape");
        assert_eq!([f(8), f(9), f(10), f(11)], [8.0, 20.0, 0.01, 1.0], "size");
        assert_eq!(f(15), 0.02, "empty の許容誤差");
        assert_eq!(u32::from_le_bytes(bytes[64..68].try_into().unwrap()), 0xABCD, "seed");
        assert_eq!(bytes.len(), 96);
        assert_eq!(f(20), 0.3, "extra.x = roam");
    }

    #[test]
    fn uniform_values_are_clamped() {
        let params = DeepBackgroundParams {
            parallax: 9.0,
            grid: -4.0,
            density: -1.0,
            min_radius: 30.0,
            max_radius: 5.0,
            brightness: -2.0,
            roam: 5.0,
            ..Default::default()
        };
        let bytes = uniform_bytes(&params, [0.0; 2], [1, 1], 0);
        let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((f(4), f(5), f(6)), (1.0, 1.0, 0.0));
        assert_eq!((f(8), f(9), f(11)), (30.0, 30.0, 0.0), "最大は最小以上");
        assert_eq!(f(20), 0.45, "roam は 0.45 まで");
    }

    #[test]
    fn clock_is_shared_and_wraps() {
        let clock = DeepBackgroundClock::default();
        let other = clock.clone();
        clock.set_tick(12);
        assert_eq!(other.tick(), 12);
        clock.set_tick(u64::from(u32::MAX) + 5);
        assert_eq!(other.tick(), 4, "2^32 で折り返す");
    }
}
