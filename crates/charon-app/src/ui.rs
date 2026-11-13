//! 仕様 06 §8: 材質一覧とブラシ・シミュレーション操作の UI。

use charon_core::cell::MaterialId;
use charon_core::material::MaterialTable;
use charon_core::world::World;
use charon_render::{GpuContext, LightQuality};
use charon_world::building::Layer;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::demo_ui::{DemoAction, DemoUi, FrameStats, Overlay};
use crate::settings::{Action, KeyBindings};

#[derive(Debug)]
pub(super) struct Controls {
    pub(super) selected: MaterialId,
    /// `Some(先頭の ID)` のとき、ブラシは材質の代わりに x 座標で色を変える虹色の材質群を置く (仕様 09 §2)。
    pub(super) rainbow: Option<MaterialId>,
    pub(super) build_layer: Option<Layer>,
    /// 左ボタンをブラシにする (`B` キー、`Draw` パネルの `Paint`)。偽のとき、プレイヤーがいれば左ボタンは武器 (仕様 10 §9.5)。
    pub(super) paint: bool,
    pub(super) brush_radius: u32,
    pub(super) paused: bool,
    pub(super) step_once: bool,
    pub(super) origin: [f32; 2],
    pub(super) zoom: u32,
    pub(super) light_quality: LightQuality,
    pub(super) show_debug: bool,
    /// 画面の部品 (パネル・仮想パッド) の大きさの倍率。
    pub(super) ui_scale: f32,
    /// 画像のスプライトの大きさの倍率 (ゲームの `SpriteAsset::scale` に掛ける)。
    pub(super) sprite_scale: f32,
    /// 音量 (0.0..=1.0): 全体・効果音・音楽 (仕様 12)。
    pub(super) master_volume: f32,
    pub(super) sfx_volume: f32,
    pub(super) music_volume: f32,
    /// ポーズのメニューが開いている (開いている間、世界は止まる、仕様 10 §14)。
    pub(super) pause_menu: bool,
    /// 持ち物の画面が開いている。
    pub(super) inventory_open: bool,
    /// キー割り当ての変更中の操作 (次に押したキーを割り当てる)。
    pub(super) rebinding: Option<Action>,
    /// キー割り当て (画面に出す・変更する。アプリの入力と同期する)。
    pub(super) bindings: KeyBindings,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) save: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) replay: bool,
    #[cfg(target_arch = "wasm32")]
    pub(super) web_save: bool,
    #[cfg(target_arch = "wasm32")]
    pub(super) web_status: String,
}

impl Controls {
    pub(super) fn new(selected: MaterialId) -> Self {
        Self {
            selected,
            rainbow: None,
            build_layer: None,
            paint: false,
            brush_radius: 3,
            paused: false,
            step_once: false,
            origin: [0.0, 0.0],
            zoom: 4,
            light_quality: LightQuality::High,
            show_debug: false,
            ui_scale: 1.0,
            sprite_scale: 1.0,
            master_volume: 1.0,
            sfx_volume: 1.0,
            music_volume: 0.7,
            pause_menu: false,
            inventory_open: false,
            rebinding: None,
            bindings: KeyBindings::default(),
            #[cfg(not(target_arch = "wasm32"))]
            save: false,
            #[cfg(not(target_arch = "wasm32"))]
            replay: false,
            #[cfg(target_arch = "wasm32")]
            web_save: false,
            #[cfg(target_arch = "wasm32")]
            web_status: super::NOT_SAVED.to_owned(),
        }
    }
}

pub(super) struct Gui {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    pub(super) controls: Controls,
    /// 参照画面の UI (仕様 09 §5)。`None` のときは従来の Sandbox パネル。
    pub(super) demo: Option<DemoUi>,
}

impl std::fmt::Debug for Gui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gui").field("controls", &self.controls).finish_non_exhaustive()
    }
}

impl Gui {
    pub(super) fn new(
        gpu: &GpuContext,
        window: &Window,
        selected: MaterialId,
        demo: Option<&MaterialTable>,
    ) -> Self {
        let context = egui::Context::default();
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let format = gpu
            .surface_config()
            .map_or(charon_render::PassList::scene_format(), |config| config.format);
        let renderer = egui_wgpu::Renderer::new(&gpu.device, format, Default::default());
        Self {
            context,
            state,
            renderer,
            controls: Controls::new(selected),
            demo: demo.map(DemoUi::new),
        }
    }

    pub(super) fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    pub(super) fn render(
        &mut self,
        gpu: &GpuContext,
        window: &Window,
        scene: &Overlay<'_>,
        target: &wgpu::TextureView,
        stats: FrameStats,
    ) -> Option<DemoAction> {
        let world = scene.world;
        let tick_ms = stats.tick_ms;
        // UI の大きさ (倍率)。egui の `zoom_factor` で、全部の部品が同じ倍率で拡大・縮小する。
        self.context.set_zoom_factor(self.controls.ui_scale.clamp(0.5, 3.0));
        let input = self.state.take_egui_input(window);
        let controls = &mut self.controls;
        let demo = &mut self.demo;
        let mut action = None;
        let output = self.context.run_pass(input, |context| {
            if let Some(demo) = demo.as_mut() {
                action = demo.draw(context, controls, scene, stats, world.tick());
                return;
            }
            egui::Window::new("Sandbox").default_pos([12.0, 12.0]).show(context, |ui| {
                ui.label("Materials");
                egui::ScrollArea::vertical().max_height(270.0).show(ui, |ui| {
                    for id in 0..world.materials().len() {
                        let material_id = MaterialId(id as u16);
                        if material_id == MaterialId::BOUNDARY {
                            continue;
                        }
                        let material = world.materials().get(material_id);
                        if ui
                            .selectable_label(controls.selected == material_id, &material.name)
                            .clicked()
                        {
                            controls.selected = material_id;
                            controls.rainbow = None;
                        }
                    }
                });
                ui.separator();
                ui.add(egui::Slider::new(&mut controls.brush_radius, 1..=24).text("Brush radius"));
                ui.horizontal(|ui| {
                    ui.label("Tool");
                    ui.selectable_value(&mut controls.build_layer, None, "Brush");
                    ui.selectable_value(
                        &mut controls.build_layer,
                        Some(Layer::Foreground),
                        "Build front",
                    );
                    ui.selectable_value(
                        &mut controls.build_layer,
                        Some(Layer::Background),
                        "Build back",
                    );
                });
                ui.label("Drag a rigid body; scroll while holding to rotate it.");
                if ui.button("Eraser").clicked() {
                    controls.selected = MaterialId::EMPTY;
                    controls.rainbow = None;
                }
                ui.horizontal(|ui| {
                    if ui.button(if controls.paused { "Play" } else { "Pause" }).clicked() {
                        controls.paused = !controls.paused;
                    }
                    if ui.button("Step").clicked() {
                        controls.step_once = true;
                        controls.paused = true;
                    }
                });
                ui.add(egui::Slider::new(&mut controls.zoom, 1..=8).text("Zoom"));
                ui.add(egui::Slider::new(&mut controls.ui_scale, 0.5..=3.0).text("UI scale"));
                ui.horizontal(|ui| {
                    ui.label("Light");
                    for (quality, name) in [
                        (LightQuality::Off, "Off"),
                        (LightQuality::Low, "Low"),
                        (LightQuality::High, "High"),
                        (LightQuality::XHigh, "xHigh"),
                    ] {
                        ui.selectable_value(&mut controls.light_quality, quality, name);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("X -").clicked() {
                        controls.origin[0] -= 16.0;
                    }
                    if ui.button("X +").clicked() {
                        controls.origin[0] += 16.0;
                    }
                    if ui.button("Y -").clicked() {
                        controls.origin[1] -= 16.0;
                    }
                    if ui.button("Y +").clicked() {
                        controls.origin[1] += 16.0;
                    }
                });
                ui.checkbox(&mut controls.show_debug, "Debug");
                if controls.show_debug {
                    ui.label(format!("Tick: {} · {:.2} ms", world.tick(), tick_ms));
                    ui.label(format!("Awake chunks: {}", world.awake_chunks()));
                }
                #[cfg(not(target_arch = "wasm32"))]
                ui.horizontal(|ui| {
                    if ui.button("Save replay").clicked() {
                        controls.save = true;
                    }
                    if ui.button("Load replay").clicked() {
                        controls.replay = true;
                    }
                });
                #[cfg(target_arch = "wasm32")]
                {
                    if ui.button("Save world").clicked() {
                        controls.web_save = true;
                    }
                    ui.label(&controls.web_status);
                }
            });
            if controls.show_debug {
                draw_debug(context, world, controls);
            }
        });
        self.state.handle_platform_output(window, output.platform_output);
        let size = window.inner_size();
        paint_egui(
            &mut self.renderer,
            &self.context,
            gpu,
            [size.width, size.height],
            target,
            output.shapes,
            output.textures_delta,
        );
        action
    }
}

/// egui の図形を `target` の上に重ねて描く。ウィンドウに依存しないので、画面なしの描画検査でも使う。
pub(super) fn paint_egui(
    renderer: &mut egui_wgpu::Renderer,
    context: &egui::Context,
    gpu: &GpuContext,
    size: [u32; 2],
    target: &wgpu::TextureView,
    shapes: Vec<egui::epaint::ClippedShape>,
    mut textures_delta: egui::TexturesDelta,
) {
    let pixels_per_point = context.pixels_per_point();
    let jobs = context.tessellate(shapes, pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point };
    for (id, deltas) in textures_delta.set.drain() {
        for delta in deltas {
            renderer.update_texture(&gpu.device, &gpu.queue, id, &delta);
        }
    }
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("charon egui") });
    let commands = renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &jobs, &screen);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("charon egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    gpu.queue.submit(commands.into_iter().chain([encoder.finish()]));
    for id in textures_delta.free.drain() {
        renderer.free_texture(&id);
    }
}
fn draw_debug(context: &egui::Context, world: &World, controls: &Controls) {
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("charon debug bounds"),
    ));
    let scale = controls.zoom as f32 / context.pixels_per_point();
    let screen = |x: f32, y: f32| {
        egui::pos2((x - controls.origin[0]) * scale, (y - controls.origin[1]) * scale)
    };
    for pos in world.chunk_positions() {
        let origin = pos.origin();
        let rect = egui::Rect::from_min_max(
            screen(origin.x as f32, origin.y as f32),
            screen((origin.x + 64) as f32, (origin.y + 64) as f32),
        );
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 208, 232)),
            egui::StrokeKind::Inside,
        );
        if let Some(dirty) = world.chunk(pos).and_then(|chunk| chunk.dirty.or(chunk.next_dirty)) {
            let dirty_rect = egui::Rect::from_min_max(
                screen(
                    (origin.x + i32::from(dirty.min_x)) as f32,
                    (origin.y + i32::from(dirty.min_y)) as f32,
                ),
                screen(
                    (origin.x + i32::from(dirty.max_x) + 1) as f32,
                    (origin.y + i32::from(dirty.max_y) + 1) as f32,
                ),
            );
            painter.rect_stroke(
                dirty_rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 166, 66)),
                egui::StrokeKind::Inside,
            );
        }
    }
}
