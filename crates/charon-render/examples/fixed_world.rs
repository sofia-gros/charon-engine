//! T4.3 の実機確認: 固定の砂・水・壁をウィンドウに表示する。
//! `cargo run -p charon-render --example fixed_world`

use std::sync::Arc;

use charon_core::testkit::{box_art, fill, world_from_ascii};
use charon_core::world::World;
use charon_render::{BackgroundPass, CellPass, FullscreenPass, GpuContext, PassList};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct Demo {
    window: Option<Arc<Window>>,
    gpu: Option<GpuContext>,
    passes: Option<PassList>,
    world: Option<World>,
}

impl ApplicationHandler for Demo {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("Charon — fixed world")
                .with_inner_size(LogicalSize::new(1024, 768)),
        ) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                eprintln!("ウィンドウを開けません: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        if let Some(gpu) = &mut self.gpu {
            if let Err(err) = gpu.recreate_surface(Arc::clone(&window), size.width, size.height) {
                eprintln!("サーフェスを復帰できません: {err}");
                event_loop.exit();
                return;
            }
        } else {
            let descriptor = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            ));
            let gpu = match pollster::block_on(GpuContext::new(
                descriptor,
                Arc::clone(&window),
                size.width,
                size.height,
            )) {
                Ok(gpu) => gpu,
                Err(err) => {
                    eprintln!("GPU を初期化できません: {err}");
                    event_loop.exit();
                    return;
                }
            };
            let mut world = world_from_ascii(&box_art(160, 120), 1);
            fill(&mut world, 20, 8, 75, 36, 's');
            fill(&mut world, 85, 38, 138, 65, 'w');
            fill(&mut world, 82, 70, 142, 71, '#');
            for _ in 0..45 {
                world.step();
            }
            let Some(config) = gpu.surface_config() else {
                eprintln!("ゼロサイズのサーフェスです");
                event_loop.exit();
                return;
            };
            let cells = match CellPass::new(&gpu, world.materials(), PassList::scene_format(), 256)
            {
                Ok(cells) => cells,
                Err(err) => {
                    eprintln!("セルパスを作れません: {err}");
                    event_loop.exit();
                    return;
                }
            };
            let mut passes = PassList::new(&gpu, config.width, config.height, 4);
            passes.add(BackgroundPass::new(wgpu::Color { r: 0.08, g: 0.10, b: 0.14, a: 1.0 }));
            passes.add(cells);
            passes.add(FullscreenPass::new(&gpu, include_str!("../src/present.wgsl")));
            self.passes = Some(passes);
            self.world = Some(world);
            self.gpu = Some(gpu);
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &mut self.gpu {
            gpu.suspend();
        }
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_ref() else { return };
        if window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu
                    && let Err(err) = gpu.resize(size.width, size.height)
                {
                    eprintln!("サーフェスのリサイズに失敗: {err}");
                    event_loop.exit();
                    return;
                }
                if let (Some(gpu), Some(passes)) = (self.gpu.as_ref(), self.passes.as_mut()) {
                    passes.resize(gpu, size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let (Some(gpu), Some(passes), Some(world)) =
                    (self.gpu.as_ref(), self.passes.as_mut(), self.world.as_mut())
                else {
                    return;
                };
                let Some(surface) = gpu.surface() else { return };
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => {
                        return;
                    }
                    error => {
                        eprintln!("フレームを取得できません: {error:?}");
                        event_loop.exit();
                        return;
                    }
                };
                let view = frame.texture.create_view(&Default::default());
                let positions = world.chunk_positions();
                if let Err(err) = passes.render(gpu, world, &positions, &view, [0.0, 0.0]) {
                    eprintln!("描画に失敗: {err}");
                    event_loop.exit();
                    return;
                }
                gpu.queue.present(frame);
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut Demo::default())?;
    Ok(())
}
