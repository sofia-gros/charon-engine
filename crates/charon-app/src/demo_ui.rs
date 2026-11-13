//! 仕様 09 §5: 参照画面の UI。画面に依存しない部分 (履歴・見本・クリップボード・輪郭) は純粋な関数にする。

use std::collections::VecDeque;

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::{MaterialTable, Phase};
use charon_core::rng::hash;
use charon_core::world::World;
use charon_game::EntityWorld;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use charon_render::LightQuality;

use crate::ui::Controls;

/// 計測グラフに残すフレーム数。
pub(super) const HISTORY: usize = 120;
/// クリップボードに入れられる矩形の一辺の最大 (セル)。
pub(super) const CLIP_MAX: i32 = 256;
/// 地形の輪郭として 1 フレームに描くセルの最大数。
pub(super) const EDGE_LIMIT: usize = 20_000;

/// `Main Menu` から App へ渡す操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DemoAction {
    /// 最初の世界 (seed 1) に戻る。
    Singleplayer,
    /// 新しい seed で作り直す。
    RandomSeed,
    Quit,
}

/// 1 フレームの計測値 (ミリ秒)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FrameStats {
    pub(super) frame_ms: f32,
    pub(super) tick_ms: f32,
    pub(super) phys_ms: f32,
}

impl FrameStats {
    pub(super) fn fps(&self) -> f32 {
        if self.frame_ms > 0.0 { 1000.0 / self.frame_ms } else { 0.0 }
    }
}

/// 直近 [`HISTORY`] フレームの計測値。
#[derive(Debug, Default)]
pub(super) struct PerfHistory {
    samples: VecDeque<FrameStats>,
}

impl PerfHistory {
    /// 有限でない値や負の値は 0 にして残す。古いものから捨てる。
    pub(super) fn push(&mut self, stats: FrameStats) {
        let clean = |value: f32| if value.is_finite() && value > 0.0 { value } else { 0.0 };
        if self.samples.len() == HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(FrameStats {
            frame_ms: clean(stats.frame_ms),
            tick_ms: clean(stats.tick_ms),
            phys_ms: clean(stats.phys_ms),
        });
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.samples.len()
    }

    pub(super) fn latest(&self) -> FrameStats {
        self.samples.back().copied().unwrap_or_default()
    }

    pub(super) fn series(&self, pick: impl Fn(&FrameStats) -> f32) -> Vec<f32> {
        self.samples.iter().map(pick).collect()
    }
}

/// `Draw` の色見本が選ぶもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SwatchTarget {
    Material(MaterialId),
    /// 先頭の ID。同じ族の 12 個が連続し、x 座標で色が変わる。
    Rainbow(MaterialId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Swatch {
    pub(super) label: String,
    pub(super) color: [u8; 4],
    pub(super) target: SwatchTarget,
}

/// 全材質の見本。`empty` と `boundary` は除き、虹色の族は 1 つの見本にまとめる。
pub(super) fn swatches(materials: &MaterialTable) -> Vec<Swatch> {
    let mut result = Vec::new();
    for index in 0..materials.len() {
        let id = MaterialId(index as u16);
        if id == MaterialId::EMPTY || id == MaterialId::BOUNDARY {
            continue;
        }
        let material = materials.get(id);
        if let Some(family) = material.name.strip_prefix("fse_demo:rainbow_") {
            // `sand_0` だけが族の先頭。他の色は見本にしない。
            if let Some(kind) = family.strip_suffix("_0") {
                result.push(Swatch {
                    label: format!("Rainbow {kind}"),
                    color: material.color,
                    target: SwatchTarget::Rainbow(id),
                });
            }
            continue;
        }
        result.push(Swatch {
            label: material.name.clone(),
            color: material.color,
            target: SwatchTarget::Material(id),
        });
    }
    result
}

/// コピーした矩形の材質。`EMPTY` は貼り付けで何も置かない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Clip {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) cells: Vec<MaterialId>,
}

/// 2 点で決まる矩形 (両端を含む) の材質をコピーする。大きすぎる・空だけなら `None`。
/// 境界と剛体のスタンプは空として扱う (地形として複製しない)。
pub(super) fn copy_rect(world: &World, a: CellPos, b: CellPos) -> Option<Clip> {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let width = i64::from(x1) - i64::from(x0) + 1;
    let height = i64::from(y1) - i64::from(y0) + 1;
    if width > i64::from(CLIP_MAX) || height > i64::from(CLIP_MAX) {
        return None;
    }
    let mut cells = Vec::with_capacity((width * height) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let cell = world.get(CellPos::new(x, y));
            cells.push(if cell.has(FLAG_BODY) || cell.material == MaterialId::BOUNDARY {
                MaterialId::EMPTY
            } else {
                cell.material
            });
        }
    }
    cells.iter().any(|&id| id != MaterialId::EMPTY).then_some(Clip {
        width: width as i32,
        height: height as i32,
        cells,
    })
}

/// `at` を左上にクリップを貼る。空のセルは置かない。読み込み済みで境界・剛体でない位置だけを
/// `make_cell` と `set` で書き換え、書いたセル数を返す。
pub(super) fn paste(world: &mut World, clip: &Clip, at: CellPos) -> usize {
    let mut written = 0;
    for dy in 0..clip.height {
        for dx in 0..clip.width {
            let material = clip.cells[(dy * clip.width + dx) as usize];
            if material == MaterialId::EMPTY {
                continue;
            }
            let pos = CellPos::new(at.x.saturating_add(dx), at.y.saturating_add(dy));
            if world.chunk(pos.chunk()).is_none() {
                continue;
            }
            let current = world.get(pos);
            if current.has(FLAG_BODY)
                || current.material == MaterialId::BOUNDARY
                || current.material == material
            {
                continue;
            }
            let cell = world.make_cell(material, pos);
            world.set(pos, cell);
            written += 1;
        }
    }
    written
}

/// 範囲 (両端を含む) の固体セルのうち、上下左右のどれかが固体でないもの (地形の輪郭)。
/// 剛体のスタンプは地形ではないので除く。最大 `limit` 個。
pub(super) fn terrain_edges(
    world: &World,
    min: CellPos,
    max: CellPos,
    limit: usize,
) -> Vec<CellPos> {
    let solid = |x: i32, y: i32| {
        let cell = world.get(CellPos::new(x, y));
        !cell.has(FLAG_BODY) && world.materials().get(cell.material).phase == Phase::Solid
    };
    let mut edges = Vec::new();
    for y in min.y..=max.y {
        for x in min.x..=max.x {
            if solid(x, y)
                && !(solid(x - 1, y) && solid(x + 1, y) && solid(x, y - 1) && solid(x, y + 1))
            {
                if edges.len() == limit {
                    return edges;
                }
                edges.push(CellPos::new(x, y));
            }
        }
    }
    edges
}

/// `Random Seed` の次の seed。時計を使わず、今の seed と呼ばれた回数だけから決める。0 にはならない。
pub(super) fn next_seed(current: u64, count: u64) -> u64 {
    const SALT_SEED: u32 = 0x5345_4544;
    let high = hash(current, count, 1, 0, SALT_SEED);
    let low = hash(current, count, 2, 0, SALT_SEED);
    ((u64::from(high) << 32) | u64::from(low)).max(1)
}

/// 画面に重ねて描く対象。
pub(super) struct Overlay<'a> {
    pub(super) world: &'a World,
    pub(super) physics: &'a PhysicsEngine,
    pub(super) entities: &'a EntityWorld,
    /// 体力・杖・弾 (仕様 10 §6)。`Wand` パネルに使う。
    pub(super) game: &'a charon_game::game::GameState,
    pub(super) player: Option<charon_game::EntityId>,
    /// 画面の仮想スティックとボタン (仕様 10 §10)。出さないときは `None`。
    pub(super) pad: Option<&'a crate::touch_pad::TouchPad>,
}

/// 持ち物の画面 (仕様 10 §14): 持っている材質を多い順に、名前と数で。
fn draw_inventory(context: &egui::Context, overlay: &Overlay<'_>, controls: &mut Controls) {
    let mut open = true;
    egui::Window::new("Inventory").open(&mut open).default_pos([40.0, 300.0]).show(context, |ui| {
        let items: Vec<(u32, String)> = overlay
            .player
            .and_then(|id| overlay.game.inventories.get(&id))
            .map(|inventory| {
                let mut items: Vec<(u32, String)> = inventory
                    .iter()
                    .map(|(material, count)| {
                        (count, overlay.world.materials().get(material).name.clone())
                    })
                    .collect();
                items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                items
            })
            .unwrap_or_default();
        if items.is_empty() {
            ui.label("Empty — dig something with the laser.");
            return;
        }
        let total: u64 = items.iter().map(|(count, _)| u64::from(*count)).sum();
        ui.label(format!("{} kinds · {total} items", items.len()));
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            egui::Grid::new("charon inventory").num_columns(2).show(ui, |ui| {
                for (count, name) in &items {
                    ui.label(name);
                    ui.label(count.to_string());
                    ui.end_row();
                }
            });
        });
    });
    controls.inventory_open = open;
}

/// ポーズのメニュー (仕様 10 §14): 再開・設定 (UI の大きさ・キャラクターの大きさ・光)・キー割り当て・持ち物・終了。
fn draw_pause_menu(context: &egui::Context, controls: &mut Controls) -> Option<DemoAction> {
    let mut action = None;
    egui::Window::new("Pause")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Resume").clicked() {
                    controls.pause_menu = false;
                    controls.rebinding = None;
                }
                if ui.button("Inventory").clicked() {
                    controls.inventory_open = !controls.inventory_open;
                }
                if ui.button("Quit").clicked() {
                    action = Some(DemoAction::Quit);
                }
            });
            ui.separator();
            ui.add(egui::Slider::new(&mut controls.ui_scale, 0.5..=3.0).text("UI scale"));
            ui.add(
                egui::Slider::new(&mut controls.sprite_scale, 0.25..=3.0).text("Character scale"),
            );
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
            ui.add(egui::Slider::new(&mut controls.master_volume, 0.0..=1.0).text("Master volume"));
            ui.add(egui::Slider::new(&mut controls.sfx_volume, 0.0..=1.0).text("Effects volume"));
            ui.add(egui::Slider::new(&mut controls.music_volume, 0.0..=1.0).text("Music volume"));
            ui.separator();
            ui.label("Keys (click, then press a key; Esc cancels)");
            egui::Grid::new("charon keys").num_columns(2).show(ui, |ui| {
                for key_action in crate::settings::Action::ALL {
                    ui.label(key_action.label());
                    let waiting = controls.rebinding == Some(key_action);
                    let text = if waiting {
                        "press a key…".to_owned()
                    } else {
                        controls.bindings.describe(key_action)
                    };
                    if ui.button(text).clicked() {
                        controls.rebinding = if waiting { None } else { Some(key_action) };
                    }
                    ui.end_row();
                }
            });
            if ui.button("Reset keys").clicked() {
                controls.bindings.reset();
                controls.rebinding = None;
            }
        });
    action
}

/// 仮想スティックとボタンを、半透明で描く (ピクセル → egui の点へ直す)。
fn draw_touch_pad(context: &egui::Context, pad: &crate::touch_pad::TouchPad) {
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("charon touch pad"),
    ));
    let scale = 1.0 / context.pixels_per_point();
    let at = |p: [f32; 2]| egui::pos2(p[0] * scale, p[1] * scale);
    let layout = pad.layout();
    let (unit, radius) = (layout.unit * scale, layout.radius() * scale);
    let line = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(110));
    let fill = egui::Color32::from_white_alpha(28);
    let knob = egui::Color32::from_white_alpha(120);
    // スティック: 触れているときは、触れた場所を中心にして、指の位置に丸を描く。
    for (touch, home, label) in
        [(pad.stick_touch(), layout.stick_home, "MOVE"), (pad.aim_touch(), layout.aim_home, "AIM")]
    {
        let center = touch.map_or(home, |(start, _)| start);
        painter.circle(at(center), radius, fill, line);
        if touch.is_none() {
            painter.text(
                at(center),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(unit * 0.6),
                egui::Color32::from_white_alpha(150),
            );
        }
        if let Some((start, now)) = touch {
            let (dx, dy) = (now[0] - start[0], now[1] - start[1]);
            let length = dx.hypot(dy).max(1.0);
            let reach = length.min(layout.radius());
            let tip = [start[0] + dx / length * reach, start[1] + dy / length * reach];
            painter.circle_filled(at(tip), unit * 0.9, knob);
        }
    }
    for (center, label, held) in [
        (layout.jump, "JUMP", pad.held("jump")),
        (layout.hook, "HOOK", pad.held("hook")),
        (layout.dig, "DIG", pad.held("dig")),
        (layout.paint, "PAINT", pad.held("paint")),
    ] {
        let color = if held { knob } else { fill };
        painter.circle(at(center), unit, color, line);
        painter.text(
            at(center),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(unit * 0.5),
            egui::Color32::from_white_alpha(200),
        );
    }
}

/// 参照画面の UI の状態。
#[derive(Debug)]
pub(super) struct DemoUi {
    pub(super) history: PerfHistory,
    pub(super) show_chunks: bool,
    pub(super) show_dirty: bool,
    pub(super) show_collision: bool,
    pub(super) show_bodies: bool,
    pub(super) clip: Option<Clip>,
    /// `Ctrl` + ドラッグで選んだ矩形の 2 点。
    pub(super) selection: Option<[CellPos; 2]>,
    pub(super) paste_armed: bool,
    swatches: Vec<Swatch>,
    status: String,
}

impl DemoUi {
    pub(super) fn new(materials: &MaterialTable) -> Self {
        Self {
            history: PerfHistory::default(),
            show_chunks: false,
            show_dirty: false,
            show_collision: false,
            show_bodies: false,
            clip: None,
            selection: None,
            paste_armed: false,
            swatches: swatches(materials),
            status: String::new(),
        }
    }

    /// 1 フレーム分のパネルと重ね描きを出す。`Main Menu` が押されたら操作を返す。
    pub(super) fn draw(
        &mut self,
        context: &egui::Context,
        controls: &mut Controls,
        overlay: &Overlay<'_>,
        stats: FrameStats,
        world_ticks: u64,
    ) -> Option<DemoAction> {
        self.history.push(stats);
        let mut action = None;
        draw_wand_panel(context, overlay);
        if controls.inventory_open {
            draw_inventory(context, overlay, controls);
        }
        if controls.pause_menu
            && let Some(chosen) = draw_pause_menu(context, controls)
        {
            action = Some(chosen);
        }
        if let Some(pad) = overlay.pad {
            draw_touch_pad(context, pad);
        }

        egui::Area::new(egui::Id::unique("charon fps")).fixed_pos(egui::pos2(2.0, 1.0)).show(
            context,
            |ui| {
                ui.label(
                    egui::RichText::new(format!("{:.0} FPS", stats.fps()))
                        .size(9.0)
                        .color(egui::Color32::from_rgb(230, 70, 70)),
                );
            },
        );

        egui::Window::new("Debug")
            .default_pos([6.0, 14.0])
            .default_open(false)
            .resizable(false)
            .show(context, |ui| {
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
                ui.add(
                    egui::Slider::new(&mut controls.sprite_scale, 0.25..=3.0)
                        .text("Character scale"),
                );
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
                ui.checkbox(&mut self.show_chunks, "Chunk bounds");
                ui.checkbox(&mut self.show_dirty, "Dirty rects");
                ui.checkbox(&mut self.show_collision, "Collision outline");
                ui.checkbox(&mut self.show_bodies, "Bodies / player");
                ui.label(format!(
                    "Tick: {} · awake chunks: {}",
                    world_ticks,
                    overlay.world.awake_chunks()
                ));
                ui.label("F: free camera · Ctrl+drag: select");
            });

        egui::Window::new("Main Menu")
            .default_pos([92.0, 14.0])
            .default_open(true)
            .resizable(false)
            .show(context, |ui| {
                if ui.button("Singleplayer").clicked() {
                    action = Some(DemoAction::Singleplayer);
                }
                if ui.button("Random Seed").clicked() {
                    action = Some(DemoAction::RandomSeed);
                }
                if ui.button("Quit").clicked() {
                    action = Some(DemoAction::Quit);
                }
            });

        egui::Window::new("Draw")
            .default_pos([204.0, 14.0])
            .default_open(true)
            .resizable(false)
            .show(context, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                ui.set_max_width(200.0);
                ui.horizontal_wrapped(|ui| {
                    for swatch in &self.swatches {
                        let selected = match swatch.target {
                            SwatchTarget::Material(id) => {
                                controls.rainbow.is_none() && controls.selected == id
                            }
                            SwatchTarget::Rainbow(first) => controls.rainbow == Some(first),
                        };
                        let (response, painter) =
                            ui.allocate_painter(egui::vec2(24.0, 24.0), egui::Sense::click());
                        let [r, g, b, a] = swatch.color;
                        painter.rect_filled(
                            response.rect,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(r, g, b, a.max(120)),
                        );
                        if let SwatchTarget::Rainbow(first) = swatch.target {
                            // 虹色は 12 本の縦縞で示す。
                            let strip = response.rect.width() / 12.0;
                            for i in 0..12u16 {
                                let [r, g, b, _] = overlay
                                    .world
                                    .materials()
                                    .get(MaterialId(first.0.saturating_add(i)))
                                    .color;
                                let x = response.rect.left() + strip * f32::from(i);
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(x, response.rect.top()),
                                        egui::pos2(x + strip + 0.5, response.rect.bottom()),
                                    ),
                                    0.0,
                                    egui::Color32::from_rgb(r, g, b),
                                );
                            }
                        }
                        painter.rect_stroke(
                            response.rect,
                            0.0,
                            egui::Stroke::new(
                                if selected { 2.0 } else { 1.0 },
                                if selected {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_gray(70)
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        if response.on_hover_text(&swatch.label).clicked() {
                            controls.build_layer = None;
                            match swatch.target {
                                SwatchTarget::Material(id) => {
                                    controls.selected = id;
                                    controls.rainbow = None;
                                }
                                SwatchTarget::Rainbow(first) => controls.rainbow = Some(first),
                            }
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut controls.brush_radius, 1..=24).text("Brush"));
                ui.checkbox(&mut controls.paint, "Paint with left button (B)");
                if ui.button("Eraser").clicked() {
                    controls.selected = MaterialId::EMPTY;
                    controls.rainbow = None;
                    controls.build_layer = None;
                }
            });

        egui::Window::new("Clipboard")
            .default_pos([566.0, 14.0])
            .default_open(false)
            .resizable(false)
            .show(context, |ui| {
                match (&self.clip, self.selection) {
                    (Some(clip), _) => {
                        ui.label(format!("{} × {} cells copied", clip.width, clip.height));
                    }
                    (None, Some(_)) => {
                        ui.label("Selection ready.");
                    }
                    (None, None) => {
                        ui.label("Nothing here...");
                    }
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.selection.is_some(), egui::Button::new("Copy")).clicked()
                        && let Some([a, b]) = self.selection
                    {
                        match copy_rect(overlay.world, a, b) {
                            Some(clip) => {
                                self.clip = Some(clip);
                                self.status.clear();
                            }
                            None => self.status = "Nothing to copy (empty or too large)".to_owned(),
                        }
                    }
                    if ui
                        .add_enabled(
                            self.clip.is_some(),
                            egui::Button::new("Paste").selected(self.paste_armed),
                        )
                        .clicked()
                    {
                        self.paste_armed = !self.paste_armed;
                    }
                    if ui.button("Clear").clicked() {
                        self.clip = None;
                        self.selection = None;
                        self.paste_armed = false;
                    }
                });
                if self.paste_armed {
                    ui.label("Click in the world to paste.");
                }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });

        egui::Window::new("Registries")
            .default_pos([670.0, 14.0])
            .default_open(false)
            .resizable(false)
            .show(context, |ui| {
                let materials = overlay.world.materials();
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for index in 0..materials.len() {
                        let id = MaterialId(index as u16);
                        if id == MaterialId::BOUNDARY {
                            continue;
                        }
                        let material = materials.get(id);
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            let [r, g, b, _] = material.color;
                            ui.painter().rect_filled(rect, 0.0, egui::Color32::from_rgb(r, g, b));
                            ui.label(format!(
                                "{} · {:?} · density {}",
                                material.name, material.phase, material.density
                            ));
                        });
                    }
                });
            });

        self.draw_performance(context);

        egui::Area::new(egui::Id::unique("charon build"))
            .anchor(egui::Align2::LEFT_BOTTOM, [4.0, -4.0])
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Development Build\nCharon Engine v{}",
                        charon_core::VERSION
                    ))
                    .size(9.0)
                    .color(egui::Color32::from_gray(190)),
                );
            });

        self.draw_overlays(context, controls, overlay);
        action
    }

    /// 右下の計測グラフ。CPU 使用率とメモリは標準ライブラリだけでは取れないので出さない。
    fn draw_performance(&self, context: &egui::Context) {
        let latest = self.history.latest();
        egui::Window::new("Performance")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-6.0, -6.0])
            .show(context, |ui| {
                ui.set_width(140.0);
                ui.label(
                    egui::RichText::new(format!(
                        "mspf: {:.2}  fps: {:.0}",
                        latest.frame_ms,
                        latest.fps()
                    ))
                    .size(10.0),
                );
                for (label, value, series, color) in [
                    (
                        "mspf",
                        latest.frame_ms,
                        self.history.series(|s| s.frame_ms),
                        egui::Color32::from_rgb(120, 200, 255),
                    ),
                    (
                        "tick mspt",
                        latest.tick_ms,
                        self.history.series(|s| s.tick_ms),
                        egui::Color32::from_rgb(240, 110, 150),
                    ),
                    (
                        "phys mspt",
                        latest.phys_ms,
                        self.history.series(|s| s.phys_ms),
                        egui::Color32::from_rgb(150, 230, 120),
                    ),
                ] {
                    ui.label(egui::RichText::new(format!("{label}: {value:.2}")).size(10.0));
                    sparkline(ui, &series, color);
                }
            });
    }

    fn draw_overlays(&self, context: &egui::Context, controls: &Controls, overlay: &Overlay<'_>) {
        let painter = context.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::unique("charon demo overlay"),
        ));
        let scale = controls.zoom as f32 / context.pixels_per_point();
        let screen = |x: f32, y: f32| {
            egui::pos2((x - controls.origin[0]) * scale, (y - controls.origin[1]) * scale)
        };
        let screen_rect = context.content_rect();
        let cells_wide = (screen_rect.width() / scale).ceil() as i32 + 2;
        let cells_high = (screen_rect.height() / scale).ceil() as i32 + 2;
        let min = CellPos::new(
            controls.origin[0].floor() as i32 - 1,
            controls.origin[1].floor() as i32 - 1,
        );
        let max = CellPos::new(min.x.saturating_add(cells_wide), min.y.saturating_add(cells_high));

        if self.show_collision {
            let fill = egui::Color32::from_rgba_unmultiplied(70, 230, 90, 200);
            for pos in terrain_edges(overlay.world, min, max, EDGE_LIMIT) {
                let rect = egui::Rect::from_min_size(
                    screen(pos.x as f32, pos.y as f32),
                    egui::vec2(scale.max(1.0), scale.max(1.0)),
                );
                painter.rect_filled(rect, 0.0, fill);
            }
        }
        if self.show_chunks || self.show_dirty {
            for pos in overlay.world.chunk_positions() {
                let origin = pos.origin();
                if self.show_chunks {
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
                }
                if self.show_dirty
                    && let Some(dirty) =
                        overlay.world.chunk(pos).and_then(|chunk| chunk.dirty.or(chunk.next_dirty))
                {
                    let rect = egui::Rect::from_min_max(
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
                        rect,
                        0.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 166, 66)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        if self.show_bodies {
            let teal = egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 200, 200));
            for body in &overlay.physics.bodies {
                let Some(rigid) = overlay.physics.world.bodies.get(body.handle) else { continue };
                let center = rigid.translation() * CELLS_PER_METER;
                let (c, r) = (screen(center.x, center.y), 4.0 * scale);
                painter.line_segment([c - egui::vec2(r, r), c + egui::vec2(r, r)], teal);
                painter.line_segment([c - egui::vec2(r, -r), c + egui::vec2(r, -r)], teal);
            }
            for entity in overlay.entities.iter() {
                let (x, y) = (entity.x16 as f32 / 16.0, entity.y16 as f32 / 16.0);
                let rect = egui::Rect::from_min_max(
                    screen(x, y),
                    screen(x + f32::from(entity.width), y + f32::from(entity.height)),
                );
                painter.rect_stroke(
                    rect.expand(1.0),
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(235, 60, 60)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        if let Some([a, b]) = self.selection {
            let (x0, x1) = (a.x.min(b.x), a.x.max(b.x) + 1);
            let (y0, y1) = (a.y.min(b.y), a.y.max(b.y) + 1);
            painter.rect_stroke(
                egui::Rect::from_min_max(
                    screen(x0 as f32, y0 as f32),
                    screen(x1 as f32, y1 as f32),
                ),
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 230, 90)),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// 折れ線の小さなグラフ。最大値で縦を正規化する (最大が 1 ms 未満でも 1 ms を下限にする)。
fn sparkline(ui: &mut egui::Ui, values: &[f32], color: egui::Color32) {
    let (response, painter) = ui.allocate_painter(egui::vec2(140.0, 26.0), egui::Sense::hover());
    painter.rect_filled(response.rect, 0.0, egui::Color32::from_black_alpha(90));
    if values.len() < 2 {
        return;
    }
    let maximum = values.iter().copied().fold(1.0_f32, f32::max);
    let rect = response.rect;
    let points: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(i, &value)| {
            egui::pos2(
                rect.left() + rect.width() * i as f32 / (HISTORY - 1) as f32,
                rect.bottom() - rect.height() * (value / maximum).clamp(0.0, 1.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, color)));
}

/// ポインターの画面位置 (ピクセル) をセルにする。有限でなければ `None`。
pub(super) fn pointer_cell(pos: [f32; 2], zoom: u32, origin: [f32; 2]) -> Option<CellPos> {
    if !pos[0].is_finite() || !pos[1].is_finite() {
        return None;
    }
    let zoom = zoom.max(1) as f32;
    Some(CellPos::new(
        (origin[0] + pos[0] / zoom).floor() as i32,
        (origin[1] + pos[1] / zoom).floor() as i32,
    ))
}

/// 仕様 10 §6.3: 体力・息・マナのバー、次の呪文の名前、飛んでいる弾の数。杖を持つプレイヤーがいるときだけ。
fn draw_wand_panel(context: &egui::Context, overlay: &Overlay<'_>) {
    let Some(id) = overlay.player else { return };
    let (Some(vitals), Some(wand)) = (overlay.game.vitals.get(id), overlay.game.wands.get(&id))
    else {
        return;
    };
    let next = wand
        .spells
        .get(wand.cursor())
        .and_then(|&spell| overlay.game.spells.get(spell))
        .map_or("-", |spell| spell.name.as_str());
    let fraction =
        |value: f32, max: f32| if max > 0.0 { (value / max).clamp(0.0, 1.0) } else { 0.0 };
    egui::Area::new(egui::Id::unique("charon wand"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 6.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let bar = |ui: &mut egui::Ui,
                           label: &str,
                           f: f32,
                           color: egui::Color32,
                           text: String| {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.add(
                            egui::ProgressBar::new(f).fill(color).desired_width(140.0).text(text),
                        );
                    });
                };
                bar(
                    ui,
                    "HP",
                    fraction(vitals.hp as f32, vitals.max_hp as f32),
                    egui::Color32::from_rgb(200, 60, 60),
                    format!("{}/{}", vitals.hp, vitals.max_hp),
                );
                bar(
                    ui,
                    "Breath",
                    fraction(f32::from(vitals.breath), f32::from(charon_game::vitals::BREATH_MAX)),
                    egui::Color32::from_rgb(70, 140, 220),
                    format!("{}", vitals.breath / 60),
                );
                bar(
                    ui,
                    "Mana",
                    fraction(wand.mana as f32, wand.mana_max as f32),
                    egui::Color32::from_rgb(90, 90, 230),
                    format!("{}/{}", wand.mana, wand.mana_max),
                );
                if let Some(hook) = overlay.game.hooks.get(&id) {
                    let state = match hook.rope().map(|r| r.phase()) {
                        Some(charon_physics::rope::RopePhase::Flying) => "flying",
                        Some(charon_physics::rope::RopePhase::Anchored) => "anchored",
                        None if hook.cooldown() > 0 => "cooling",
                        None => "ready",
                    };
                    ui.label(format!("Tool (right): {} · {state}", hook.def.name));
                }
                if let Some(inventory) = overlay.game.inventories.get(&id) {
                    // 持ち物 (仕様 10 §13): 多い順に 4 種類まで。
                    let mut items: Vec<(u32, &str)> = inventory
                        .iter()
                        .map(|(material, count)| {
                            (count, overlay.world.materials().get(material).name.as_str())
                        })
                        .collect();
                    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
                    let shown: Vec<String> = items
                        .iter()
                        .take(4)
                        .map(|(count, name)| format!("{name} {count}"))
                        .collect();
                    let text = if shown.is_empty() { "-".to_owned() } else { shown.join(" · ") };
                    ui.label(format!("Items (dig): {text}"));
                }
                // 状態異常 (仕様 10 §17)。
                {
                    use charon_game::status::StatusKind;
                    let active: Vec<&str> = [
                        (StatusKind::Burning, "burning"),
                        (StatusKind::Wet, "wet"),
                        (StatusKind::Poison, "poison"),
                    ]
                    .into_iter()
                    .filter(|(kind, _)| overlay.game.statuses.has(id, *kind))
                    .map(|(_, name)| name)
                    .collect();
                    if !active.is_empty() {
                        ui.label(format!("Status: {}", active.join(" · ")));
                    }
                }
                ui.label(format!(
                    "Weapon (left) next: {next} · Shots: {} · Enemies: {}{}",
                    overlay.game.projectiles.len(),
                    overlay.game.enemies.len(),
                    if vitals.alive() { "" } else { " · DEAD" }
                ));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::testkit::{box_art, fill, material_of, to_ascii, world_from_ascii};

    #[test]
    fn perf_history_is_bounded() {
        let mut history = PerfHistory::default();
        assert_eq!(history.latest(), FrameStats::default());
        for i in 0..(HISTORY + 30) {
            history.push(FrameStats { frame_ms: i as f32, tick_ms: 1.0, phys_ms: 0.5 });
        }
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history.latest().frame_ms, (HISTORY + 29) as f32);
        // 古いものから捨てる: 先頭は 30 番目の値。
        assert_eq!(history.series(|s| s.frame_ms)[0], 30.0);
        // 異常な値は 0 にして残す。fps は frame_ms から求める。
        history.push(FrameStats { frame_ms: f32::NAN, tick_ms: -3.0, phys_ms: f32::INFINITY });
        assert_eq!(history.latest(), FrameStats::default());
        assert_eq!(history.latest().fps(), 0.0);
        assert_eq!(FrameStats { frame_ms: 2.0, ..FrameStats::default() }.fps(), 500.0);
    }

    #[test]
    fn rainbow_swatches_are_grouped() {
        let materials = charon_core::material::MaterialTable::from_mods(&[(
            "fse_demo",
            include_str!("../../../assets/mods/fse_demo/materials.ron"),
        )])
        .unwrap();
        let list = swatches(&materials);
        let rainbow: Vec<_> =
            list.iter().filter(|s| matches!(s.target, SwatchTarget::Rainbow(_))).collect();
        assert_eq!(rainbow.len(), 2, "砂と岩で 1 つずつ");
        assert!(rainbow.iter().any(|s| s.label == "Rainbow sand"));
        assert!(rainbow.iter().any(|s| s.label == "Rainbow rock"));
        // 虹色の個別の色は見本にならず、`empty` と `boundary` も見本にならない。
        assert!(!list.iter().any(|s| s.label.starts_with("fse_demo:rainbow_")));
        assert!(!list.iter().any(|s| s.target == SwatchTarget::Material(MaterialId::EMPTY)));
        assert!(!list.iter().any(|s| s.target == SwatchTarget::Material(MaterialId::BOUNDARY)));
        // 17 組み込み材質から 2 つ除いた 15 + 他の MOD 材質 8 + 虹色 2。
        assert_eq!(list.len(), 15 + 8 + 2);
        let first = materials.id("fse_demo:rainbow_sand_0").unwrap();
        assert!(list.iter().any(|s| s.target == SwatchTarget::Rainbow(first)));
    }

    #[test]
    fn clipboard_copy_paste_roundtrip() {
        let mut world = world_from_ascii(
            "
            ######################
            #....................#
            #.ss..www............#
            #.ss..www............#
            #....................#
            ######################
            ",
            1,
        );
        let before = to_ascii(&world, 0, 0, 22, 6);
        let clip = copy_rect(&world, CellPos::new(8, 3), CellPos::new(2, 2)).unwrap();
        assert_eq!((clip.width, clip.height), (7, 2), "2 点の順序に依存しない");
        let written = paste(&mut world, &clip, CellPos::new(12, 3));
        assert_eq!(written, 10);
        let after = to_ascii(&world, 0, 0, 22, 6);
        assert_ne!(before, after);
        // 貼った場所に同じ並びが出る。空は何も置かないので、既存のセルは消えない。
        assert_eq!(to_ascii(&world, 12, 3, 19, 5), "ss..www\nss..www");
        // 同じ貼り付けをもう一度しても変わらない (書いたセルが 0)。
        assert_eq!(paste(&mut world, &clip, CellPos::new(12, 3)), 0);
        // 空だけの矩形と大きすぎる矩形はコピーできない。
        assert!(copy_rect(&world, CellPos::new(2, 4), CellPos::new(5, 4)).is_none());
        assert!(copy_rect(&world, CellPos::new(0, 0), CellPos::new(300, 3)).is_none());
        // 未読み込みのチャンクと境界には貼らない。
        assert_eq!(paste(&mut world, &clip, CellPos::new(400, 3)), 0);
        let stone = material_of(&world, '#');
        assert!(
            clip.cells.iter().all(|&id| id != stone),
            "壁は選択範囲に含めていない: {:?}",
            clip.cells.len()
        );
    }

    #[test]
    fn terrain_edges_follow_solid_boundaries() {
        let mut world = world_from_ascii(&box_art(20, 10), 1);
        fill(&mut world, 5, 3, 9, 6, '#');
        let edges = terrain_edges(&world, CellPos::new(4, 2), CellPos::new(10, 7), 1000);
        // 5×4 の塊のうち、内側の 3×2 を除く外周 (20 − 6 = 14) だけが輪郭。
        assert_eq!(edges.len(), 14);
        assert!(!edges.contains(&CellPos::new(7, 4)));
        assert!(edges.contains(&CellPos::new(5, 3)) && edges.contains(&CellPos::new(9, 6)));
        // 上限で打ち切る。砂は固体ではないので輪郭にならない。
        assert_eq!(terrain_edges(&world, CellPos::new(4, 2), CellPos::new(10, 7), 5).len(), 5);
        fill(&mut world, 12, 3, 14, 4, 's');
        assert_eq!(terrain_edges(&world, CellPos::new(11, 2), CellPos::new(15, 5), 1000).len(), 0);
    }

    #[test]
    fn random_seed_is_deterministic_and_changes() {
        for seed in 1..=8u64 {
            assert_eq!(next_seed(seed, 0), next_seed(seed, 0));
            let mut seen = std::collections::BTreeSet::from([seed]);
            for count in 0..32 {
                let next = next_seed(seed, count);
                assert_ne!(next, 0);
                seen.insert(next);
            }
            assert!(seen.len() >= 30, "seed {seed}: 重複が多い ({})", seen.len());
        }
    }

    #[test]
    fn pointer_cell_handles_zoom_and_invalid_input() {
        assert_eq!(pointer_cell([40.0, 48.0], 4, [0.0, 0.0]), Some(CellPos::new(10, 12)));
        assert_eq!(pointer_cell([0.0, 0.0], 4, [-8.0, 3.5]), Some(CellPos::new(-8, 3)));
        assert_eq!(pointer_cell([f32::NAN, 0.0], 4, [0.0, 0.0]), None);
        assert_eq!(pointer_cell([1.0, 1.0], 0, [0.0, 0.0]), Some(CellPos::new(1, 1)));
    }

    /// ポーズのメニューと持ち物の画面 (仕様 10 §14): 開くと図形が増え、閉じたままなら増えない。
    #[test]
    fn pause_menu_and_inventory_draw() {
        let world = world_from_ascii(&box_art(40, 20), 1);
        let physics = PhysicsEngine::new();
        let entities = EntityWorld::new();
        let game = charon_game::game::GameState::new(Default::default());
        let overlay = Overlay {
            world: &world,
            physics: &physics,
            entities: &entities,
            game: &game,
            player: None,
            pad: None,
        };
        let shapes = |pause: bool, inventory: bool, rebinding: bool| -> usize {
            let mut demo = DemoUi::new(world.materials());
            let mut controls = Controls::new(material_of(&world, 's'));
            controls.pause_menu = pause;
            controls.inventory_open = inventory;
            if rebinding {
                controls.rebinding = Some(crate::settings::Action::Dig);
            }
            let context = egui::Context::default();
            let mut count = 0;
            for frame in 0..3u32 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 768.0),
                    )),
                    time: Some(f64::from(frame) / 60.0),
                    ..Default::default()
                };
                let mut action = None;
                let mut output = context.run_pass(input, |context| {
                    action = demo.draw(
                        context,
                        &mut controls,
                        &overlay,
                        FrameStats { frame_ms: 2.0, tick_ms: 1.0, phys_ms: 0.1 },
                        0,
                    );
                });
                assert_eq!(action, None, "押さなければ何も起きない");
                count = output.shapes.len();
                output.textures_delta.clear();
            }
            assert_eq!(controls.pause_menu, pause);
            count
        };
        let closed = shapes(false, false, false);
        let inventory = shapes(false, true, false);
        let pause = shapes(true, false, false);
        let rebinding = shapes(true, false, true);
        assert!(inventory > closed, "持ち物の画面が出る ({closed} → {inventory})");
        assert!(pause > closed + 10, "ポーズのメニューが出る ({closed} → {pause})");
        assert_ne!(rebinding, 0);
    }

    #[test]
    fn demo_ui_draws_a_frame() {
        let world = world_from_ascii(&box_art(40, 20), 1);
        let physics = PhysicsEngine::new();
        let entities = EntityWorld::new();
        let game = charon_game::game::GameState::new(Default::default());
        let overlay = Overlay {
            world: &world,
            physics: &physics,
            entities: &entities,
            game: &game,
            player: None,
            pad: None,
        };
        let mut demo = DemoUi::new(world.materials());
        demo.show_collision = true;
        demo.show_chunks = true;
        demo.show_dirty = true;
        demo.show_bodies = true;
        demo.selection = Some([CellPos::new(2, 2), CellPos::new(8, 6)]);
        let mut controls = Controls::new(material_of(&world, 's'));
        let context = egui::Context::default();
        for frame in 0..3u32 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 768.0),
                )),
                time: Some(f64::from(frame) / 60.0),
                ..Default::default()
            };
            let mut action = None;
            let mut output = context.run_pass(input, |context| {
                action = demo.draw(
                    context,
                    &mut controls,
                    &overlay,
                    FrameStats { frame_ms: 2.0, tick_ms: 1.0, phys_ms: 0.1 },
                    u64::from(frame),
                );
            });
            assert_eq!(action, None);
            // 各パネルと重ね描き (外周の石の輪郭・チャンク枠・選択) が描かれる。
            assert!(output.shapes.len() > 20, "frame {frame}: {} 個の図形", output.shapes.len());
            output.textures_delta.clear();
        }
        assert_eq!(demo.history.len(), 3);
    }
}
