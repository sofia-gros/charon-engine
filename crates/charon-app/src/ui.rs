//! 仕様 06 §8: 材質一覧とブラシ・シミュレーション操作の UI。

use charon_core::cell::MaterialId;
use charon_core::world::World;
use charon_render::{GpuContext, LightQuality};
use charon_world::building::Layer;
use winit::event::WindowEvent;
use winit::window::Window;

#[derive(Debug)]
pub(super) struct Controls {
    pub(super) selected: MaterialId,
    pub(super) build_layer: Option<Layer>,
    pub(super) brush_radius: u32,
    pub(super) paused: bool,
    pub(super) step_once: bool,
    pub(super) origin: [f32; 2],
    pub(super) zoom: u32,
    pub(super) light_quality: LightQuality,
    pub(super) show_debug: bool,
    pub(super) save: bool,
    pub(super) replay: bool,
}

impl Controls {
    pub(super) fn new(selected: MaterialId) -> Self {
        Self {
            selected,
            build_layer: None,
            brush_radius: 3,
            paused: false,
            step_once: false,
            origin: [0.0, 0.0],
            zoom: 4,
            light_quality: LightQuality::High,
            show_debug: false,
            save: false,
            replay: false,
        }
    }
}

pub(super) struct Gui {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    pub(super) controls: Controls,
}

impl std::fmt::Debug for Gui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gui").field("controls", &self.controls).finish_non_exhaustive()
    }
}

impl Gui {
    pub(super) fn new(gpu: &GpuContext, window: &Window, selected: MaterialId) -> Self {
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
        Self { context, state, renderer, controls: Controls::new(selected) }
    }

    pub(super) fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    pub(super) fn render(
        &mut self,
        gpu: &GpuContext,
        window: &Window,
        world: &World,
        target: &wgpu::TextureView,
        tick_ms: f32,
    ) {
        let input = self.state.take_egui_input(window);
        let controls = &mut self.controls;
        let mut output = self.context.run_pass(input, |context| {
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
                ui.horizontal(|ui| {
                    ui.label("Light");
                    for (quality, name) in [
                        (LightQuality::Off, "Off"),
                        (LightQuality::Low, "Low"),
                        (LightQuality::High, "High"),
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
                ui.horizontal(|ui| {
                    if ui.button("Save replay").clicked() {
                        controls.save = true;
                    }
                    if ui.button("Load replay").clicked() {
                        controls.replay = true;
                    }
                });
            });
            if controls.show_debug {
                draw_debug(context, world, controls);
            }
        });
        self.state.handle_platform_output(window, output.platform_output);
        let pixels_per_point = self.context.pixels_per_point();
        let jobs = self.context.tessellate(output.shapes, pixels_per_point);
        let size = window.inner_size();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point,
        };
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(&gpu.device, &gpu.queue, id, &delta);
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("charon egui") });
        let commands =
            self.renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("charon egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        gpu.queue.submit(commands.into_iter().chain([encoder.finish()]));
        for id in output.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
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
