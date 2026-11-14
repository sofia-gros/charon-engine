//! 仕様 09 §5: 参照画面の UI。画面に依存しない部分 (履歴・見本・クリップボード・輪郭) は純粋な関数にする。

use std::collections::VecDeque;

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::MaterialTable;
use charon_core::rng::hash;
use charon_core::world::World;
use charon_game::EntityWorld;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use charon_render::LightQuality;

use crate::ui::{Controls, InvCommand};

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
        !cell.has(FLAG_BODY) && cell.is_solid(world.materials())
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
    /// アイテムの名前を引く定義 (ホットバーの表示用)。
    pub(super) defs: Option<&'a charon_game::defs::Definitions>,
}

/// 持ち物の画面 (仕様 18 §1、Terraria Android 型): 右上の半透明の格子 (10 列 × 5 行、1 行目 = ホットバー)・格子の下の
/// `Filter: ...` と装備・作れるもののリスト (左) と `Craft` ボタン (右端)。ゲームは止めない。操作は `controls.inv_commands`。
#[allow(deprecated)]
fn draw_inventory(context: &egui::Context, overlay: &Overlay<'_>, controls: &mut Controls) {
    use charon_game::items::{EquipSlot, ItemKind};
    let Some(inventory) = overlay.player.and_then(|id| overlay.game.inventories.get(&id)) else {
        egui::Area::new(egui::Id::unique("charon inventory none"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 70.0))
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                ui.label("No character.");
            });
        return;
    };
    let name_of = |item: charon_game::items::ItemId| item_label(overlay, item).0;
    let kind_of = |item: charon_game::items::ItemId| {
        overlay.defs.and_then(|d| d.item_def(item)).map(|d| &d.kind)
    };
    let filter_id = egui::Id::new("charon inv filter");
    let recipe_id = egui::Id::new("charon inv recipe");
    let filter: usize = context.data(|d| d.get_temp(filter_id)).unwrap_or(0);
    const FILTERS: [&str; 5] = ["All", "Weapons", "Armor", "Blocks", "Other"];
    let passes = |item: charon_game::items::ItemId| match filter {
        1 => matches!(kind_of(item), Some(ItemKind::Weapon { .. } | ItemKind::Tool { .. })),
        2 => matches!(kind_of(item), Some(ItemKind::Armor { .. })),
        3 => matches!(item.class(), charon_game::items::ItemClass::Block(_)),
        4 => {
            !matches!(
                kind_of(item),
                Some(ItemKind::Weapon { .. } | ItemKind::Tool { .. } | ItemKind::Armor { .. })
            ) && !matches!(item.class(), charon_game::items::ItemClass::Block(_))
        }
        _ => true,
    };
    let slot_size = 34.0;
    let (cols, rows) = (10usize, 5usize);
    egui::Area::new(egui::Id::new("charon inventory grid"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 70.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(140))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    // 1 行目: ホットバー。残りの行: 持ち物のアイテム (ホットバーにないもの、ID の昇順)。
                    let pinned: Vec<Option<charon_game::items::ItemId>> =
                        (0..cols).map(|s| inventory.hotbar(s)).collect();
                    let rest: Vec<(charon_game::items::ItemId, u32)> = inventory
                        .item_iter()
                        .filter(|(i, _)| !pinned.contains(&Some(*i)) && passes(*i))
                        .collect();
                    let mut rest = rest.into_iter();
                    for row in 0..rows {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                            #[allow(clippy::needless_range_loop)]
                            for col in 0..cols {
                                let (item, count, slot) = if row == 0 {
                                    let item = pinned[col];
                                    let count = item.map_or(0, |i| inventory.item_count(i));
                                    (item, count, Some(col))
                                } else {
                                    match rest.next() {
                                        Some((i, n)) => (Some(i), n, None),
                                        None => (None, 0, None),
                                    }
                                };
                                let selected = slot == Some(inventory.selected());
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(slot_size, slot_size),
                                    egui::Sense::click(),
                                );
                                let stroke = if selected {
                                    egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 220, 60))
                                } else {
                                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90))
                                };
                                ui.painter().rect(
                                    rect,
                                    3.0,
                                    egui::Color32::from_white_alpha(22),
                                    stroke,
                                    egui::StrokeKind::Inside,
                                );
                                if let Some(item) = item {
                                    let (name, color) = item_label(overlay, item);
                                    ui.painter().rect_filled(rect.shrink(6.0), 3.0, color);
                                    ui.painter().text(
                                        rect.center_bottom() - egui::vec2(0.0, 2.0),
                                        egui::Align2::CENTER_BOTTOM,
                                        short_name(&name),
                                        egui::FontId::proportional(9.0),
                                        egui::Color32::WHITE,
                                    );
                                    if count > 0 {
                                        ui.painter().text(
                                            rect.right_top() + egui::vec2(-3.0, 2.0),
                                            egui::Align2::RIGHT_TOP,
                                            count.to_string(),
                                            egui::FontId::proportional(11.0),
                                            egui::Color32::WHITE,
                                        );
                                    }
                                    let response = response.on_hover_text(name);
                                    if response.clicked() {
                                        match slot {
                                            Some(s) => {
                                                controls.inv_commands.push(InvCommand::Select(s))
                                            }
                                            None => {
                                                let equippable = overlay
                                                    .defs
                                                    .and_then(|d| d.item_def(item))
                                                    .is_some_and(|d| d.equip_slot().is_some());
                                                if equippable {
                                                    controls
                                                        .inv_commands
                                                        .push(InvCommand::Equip(item));
                                                } else {
                                                    controls.inv_commands.push(InvCommand::Pin(
                                                        inventory.selected(),
                                                        item,
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                } else if row == 0 && response.clicked() {
                                    controls.inv_commands.push(InvCommand::Select(col));
                                }
                            }
                        });
                    }
                    // 格子の下: 絞り込みと装備。
                    ui.horizontal(|ui| {
                        if ui
                            .button(format!("Filter: {}", FILTERS[filter % FILTERS.len()]))
                            .clicked()
                        {
                            context.data_mut(|d| {
                                d.insert_temp(filter_id, (filter + 1) % FILTERS.len())
                            });
                        }
                        let equipment =
                            overlay.player.and_then(|id| overlay.game.equipment.get(&id));
                        for slot in EquipSlot::ALL {
                            let item = equipment.and_then(|e| e.get(slot));
                            let text = item.map_or_else(|| "-".to_owned(), name_of);
                            if ui
                                .small_button(short_name(&text))
                                .on_hover_text(format!("{slot:?}: {text}"))
                                .clicked()
                                && item.is_some()
                            {
                                controls.inv_commands.push(InvCommand::Unequip(slot));
                            }
                        }
                    });
                    // 作れるもの (材料のあるものを上に) と Craft ボタン。
                    if let Some(defs) = overlay.defs {
                        let mut recipes: Vec<(usize, bool)> = defs
                            .recipes()
                            .iter()
                            .enumerate()
                            .map(|(i, r)| {
                                (i, r.inputs.iter().all(|&(it, n)| inventory.item_count(it) >= n))
                            })
                            .collect();
                        recipes.sort_by_key(|&(i, ok)| (!ok, i));
                        let chosen: usize = context.data(|d| d.get_temp(recipe_id)).unwrap_or(0);
                        ui.horizontal(|ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("charon inv recipes v2")
                                .max_height(110.0)
                                .max_width(230.0)
                                .show(ui, |ui| {
                                    ui.set_width(220.0);
                                    for &(i, ok) in &recipes {
                                        let r = &defs.recipes()[i];
                                        let text =
                                            egui::RichText::new(name_of(r.output.0)).color(if ok {
                                                egui::Color32::WHITE
                                            } else {
                                                egui::Color32::from_white_alpha(100)
                                            });
                                        if ui.selectable_label(chosen == i, text).clicked() {
                                            context.data_mut(|d| d.insert_temp(recipe_id, i));
                                        }
                                    }
                                });
                            ui.vertical(|ui| {
                                if let Some(r) = defs.recipes().get(chosen) {
                                    let needs: Vec<String> = r
                                        .inputs
                                        .iter()
                                        .map(|&(i, n)| format!("{} {n}", name_of(i)))
                                        .collect();
                                    ui.label(egui::RichText::new(needs.join("\n")).size(11.0));
                                    let ok = r
                                        .inputs
                                        .iter()
                                        .all(|&(it, n)| inventory.item_count(it) >= n);
                                    if ui.add_enabled(ok, egui::Button::new("Craft")).clicked() {
                                        controls
                                            .inv_commands
                                            .push(InvCommand::Craft(r.name.clone()));
                                    }
                                }
                                // セルが 64 個たまった材質は、ブロックにできる。
                                for (item, count) in inventory.item_iter() {
                                    if let charon_game::items::ItemClass::Cells(m) = item.class()
                                        && count >= charon_game::blocks::BLOCK_CELLS as u32
                                        && matches!(
                                            overlay.world.materials().get(m).phase,
                                            charon_core::material::Phase::Solid
                                                | charon_core::material::Phase::Powder
                                        )
                                        && ui
                                            .small_button(format!("{} → Block", name_of(item)))
                                            .clicked()
                                    {
                                        controls.inv_commands.push(InvCommand::CraftBlock(m));
                                    }
                                }
                            });
                        });
                    }
                    if ui.small_button("Close").clicked() {
                        controls.inventory_open = false;
                    }
                });
        });
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
            sim_distance_slider(ui, &mut controls.sim_distance);
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

/// 仮想スティックとボタン・ホットバー・カーソルを、半透明で描く (ピクセル → egui の点へ直す)。仕様 18 §1。
fn draw_touch_pad(
    context: &egui::Context,
    pad: &crate::touch_pad::TouchPad,
    overlay: &Overlay<'_>,
    controls: &Controls,
) {
    use crate::touch_pad::HOTBAR_VISIBLE;
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("charon touch pad"),
    ));
    let scale = 1.0 / context.pixels_per_point();
    let at = |p: [f32; 2]| egui::pos2(p[0] * scale, p[1] * scale);
    let layout = pad.layout();
    let unit = layout.unit * scale;
    let line = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(110));
    let fill = egui::Color32::from_white_alpha(28);
    let knob = egui::Color32::from_white_alpha(120);
    let label = |center: [f32; 2], text: &str, size: f32| {
        painter.text(
            at(center),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(unit * size),
            egui::Color32::from_white_alpha(200),
        );
    };
    // 左スティック (固定): 中心に十字の目印。触れているときは、指の位置につまみ。
    painter.circle(at(layout.stick), layout.stick_radius * scale, fill, line);
    label(layout.stick, "+", 1.0);
    if let Some((center, now)) = pad.stick_touch() {
        let (dx, dy) = (now[0] - center[0], now[1] - center[1]);
        let length = dx.hypot(dy).max(1.0);
        let reach = length.min(layout.radius());
        painter.circle_filled(
            at([center[0] + dx / length * reach, center[1] + dy / length * reach]),
            unit * 0.9,
            knob,
        );
    }
    // 使うボタン: つまみになる。
    let using = pad.held("use");
    painter.circle(
        at(layout.use_button),
        layout.use_radius * scale,
        if using { knob } else { fill },
        line,
    );
    if let Some(tip) = pad.use_knob() {
        painter.circle_filled(at(tip), unit * 1.0, knob);
    } else {
        label(layout.use_button, "+", 1.4);
    }
    // フックのスティック: 傾けている向きを矢印で。
    painter.circle(
        at(layout.hook),
        layout.hook_radius * scale,
        if pad.held("hook") { knob } else { fill },
        line,
    );
    label(layout.hook, "G", 0.9);
    // 仕様 18 §2.1: フックの傾きは、使うボタンのカーソルとは独立してプレイヤーから示す。
    if let (Some((dx, dy)), Some(player)) =
        (pad.hook_aim(), overlay.player.and_then(|id| overlay.entities.get(id)))
    {
        let center = egui::pos2(
            ((player.x16 as f32 / 16.0 + f32::from(player.width) / 2.0) - controls.origin[0])
                * controls.zoom
                * scale,
            ((player.y16 as f32 / 16.0 + f32::from(player.height) / 2.0) - controls.origin[1])
                * controls.zoom
                * scale,
        );
        let direction = egui::vec2(dx as f32, dy as f32).normalized();
        let tip = center + direction * (layout.unit * 0.8 * scale);
        let side = egui::vec2(-direction.y, direction.x);
        let arrow = egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 230, 90));
        painter.line_segment([center, tip], arrow);
        painter.line_segment([tip, tip - direction * 9.0 + side * 6.0], arrow);
        painter.line_segment([tip, tip - direction * 9.0 - side * 6.0], arrow);
    }
    // ボタン。
    for (center, text, held, radius) in [
        (layout.jump, "^^", pad.held("jump"), layout.jump_radius),
        (layout.zoom_in, "+", pad.held("zoom_in"), 0.8 * layout.unit),
        (layout.zoom_out, "-", pad.held("zoom_out"), 0.8 * layout.unit),
        (layout.inventory, "[]", overlay_inventory_open(overlay), 0.9 * layout.unit),
        (layout.smart, "S", pad.smart(), 0.9 * layout.unit),
    ] {
        painter.circle(at(center), radius * scale, if held { knob } else { fill }, line);
        label(center, text, 0.6);
    }
    draw_hotbar(context, &painter, scale, &layout, overlay, HOTBAR_VISIBLE, false, [0.0, 0.0]);
    // カーソル (ブロックの格子に揃う枠。置けない・届かないときは赤)。
    if let Some(view) = pad.cursor_view() {
        let color = match view.state {
            charon_game::cursor::CursorState::Ok => egui::Color32::from_rgb(255, 230, 90),
            _ => egui::Color32::from_rgb(235, 70, 70),
        };
        let rect = egui::Rect::from_min_max(
            at([view.rect[0], view.rect[1]]),
            at([view.rect[2], view.rect[3]]),
        );
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(2.5, color), egui::StrokeKind::Outside);
    }
}

fn overlay_inventory_open(_overlay: &Overlay<'_>) -> bool {
    false
}

/// アイテムの表示名と、枠の色 (素材なしの仮の絵)。
fn item_label(overlay: &Overlay<'_>, item: charon_game::items::ItemId) -> (String, egui::Color32) {
    use charon_game::items::ItemClass;
    match item.class() {
        ItemClass::Cells(m) => {
            let name = overlay.world.materials().get(m).name.clone();
            (name, egui::Color32::from_rgb(120, 110, 90))
        }
        ItemClass::Block(m) => {
            let name = format!("{} block", overlay.world.materials().get(m).name);
            (name, egui::Color32::from_rgb(90, 100, 130))
        }
        ItemClass::Def(index) => {
            let name = overlay
                .defs
                .and_then(|d| d.item_names().nth(index).map(str::to_owned))
                .unwrap_or_else(|| format!("item {index}"));
            (name, egui::Color32::from_rgb(110, 90, 130))
        }
    }
}

/// 枠に収まる短い名前。
fn short_name(name: &str) -> String {
    name.chars().take(6).collect()
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
    /// 5 つのパネル (`PANELS`) が開いているか。閉じたものは、画面の左端の細いタブ (`|▸`) になり、触れると開く。
    pub(super) panels: [bool; PANELS.len()],
    /// 最初のフレームで、開閉の初期値を決めたか。
    panels_ready: bool,
}

/// パネルの名前 (開閉の添字の順)。左端のタブにも、この短い名前を出す。
const PANELS: [(&str, &str); 5] = [
    ("Debug", "Dbg"),
    ("Main Menu", "Menu"),
    ("Draw", "Draw"),
    ("Clipboard", "Clip"),
    ("Registries", "Reg"),
];
const PANEL_DEBUG: usize = 0;

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
            panels: [false; PANELS.len()],
            panels_ready: false,
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
        // パネルの開閉の初期値: タッチの画面 (仮想パッドが出る) では、画面を隠さないようにすべて閉じる。
        // デスクトップでは、`Main Menu` と `Draw` を開く。
        if !self.panels_ready {
            self.panels_ready = true;
            self.panels = if overlay.pad.is_some() {
                [false; PANELS.len()]
            } else {
                [false, true, true, false, false]
            };
        }
        let mut open = self.panels;
        // 計測 (FPS・グラフ・版の表示) は、デスクトップか、`Debug` パネルを開いたときだけ。タッチの画面では邪魔になる。
        let show_stats = overlay.pad.is_none() || open[PANEL_DEBUG];
        draw_hearts_and_stars(context, overlay);
        draw_hud_bars(context, overlay);
        draw_boss_bar(context, overlay);
        draw_death_overlay(context, overlay);
        draw_hints(context, overlay, world_ticks);
        if overlay.pad.is_none() {
            draw_desktop_hotbar(context, overlay);
        }
        if controls.inventory_open {
            draw_inventory(context, overlay, controls);
        }
        if controls.pause_menu
            && let Some(chosen) = draw_pause_menu(context, controls)
        {
            action = Some(chosen);
        }
        if let Some(pad) = overlay.pad {
            draw_touch_pad(context, pad, overlay, controls);
        }

        if show_stats {
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
        }

        egui::Window::new("Debug")
            .default_pos([40.0, 14.0])
            .open(&mut open[0])
            .collapsible(false)
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
                ui.add(egui::Slider::new(&mut controls.zoom, 1.0..=8.0).text("Zoom"));
                ui.add(egui::Slider::new(&mut controls.ui_scale, 0.5..=3.0).text("UI scale"));
                ui.add(
                    egui::Slider::new(&mut controls.sprite_scale, 0.25..=3.0)
                        .text("Character scale"),
                );
                sim_distance_slider(ui, &mut controls.sim_distance);
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
                ui.separator();
                player_info(ui, overlay);
            });

        egui::Window::new("Main Menu")
            .default_pos([92.0, 14.0])
            .open(&mut open[1])
            .collapsible(false)
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
            .open(&mut open[2])
            .collapsible(false)
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
            .open(&mut open[3])
            .collapsible(false)
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
            .open(&mut open[4])
            .collapsible(false)
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

        // 閉じたパネルは、画面の左端の細いタブになる。触れると開く。
        self.panels = open;
        draw_dock(context, &mut self.panels);

        if show_stats {
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
        }

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
        let scale = controls.zoom / context.pixels_per_point();
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
pub(super) fn pointer_cell(pos: [f32; 2], zoom: f32, origin: [f32; 2]) -> Option<CellPos> {
    if !pos[0].is_finite() || !pos[1].is_finite() {
        return None;
    }
    let zoom = zoom.max(1.0);
    Some(CellPos::new(
        (origin[0] + pos[0] / zoom).floor() as i32,
        (origin[1] + pos[1] / zoom).floor() as i32,
    ))
}

/// 画面の右上の、小さなバーだけの表示 (背景・文字なし)。上から体力 (赤)・マナ (青)・燃料 (黄)、息は減っているときだけ (水色)。
/// 状態異常は、バーの下に小さな四角 (燃える = 橙、濡れる = 水色、毒 = 緑) で出す。杖を持つプレイヤーがいるときだけ。
/// 文字の情報 (道具・持ち物・次の呪文など) は、`Debug` パネルの `player_info` に出す。
/// ホットバー (横一列): `visible` 個の枠 (タッチは `...` も)。選んでいる枠は黄色い枠。`numbers` なら、枠に数字 (キー) を出す。
#[allow(clippy::too_many_arguments)]
fn draw_hotbar(
    context: &egui::Context,
    painter: &egui::Painter,
    scale: f32,
    layout: &crate::touch_pad::Layout,
    overlay: &Overlay<'_>,
    visible: usize,
    numbers: bool,
    offset: [f32; 2],
) {
    let at = |p: [f32; 2]| egui::pos2((p[0] + offset[0]) * scale, (p[1] + offset[1]) * scale);
    let unit = layout.unit * scale;
    let line = egui::Stroke::new(2.0, egui::Color32::from_white_alpha(110));
    let fill = egui::Color32::from_white_alpha(28);
    let Some(inventory) = overlay.player.and_then(|id| overlay.game.inventories.get(&id)) else {
        return;
    };
    let more = !numbers;
    for slot in 0..(visible + usize::from(more)) {
        let [l, t, r, b] = layout.slot_rect(slot);
        let rect = egui::Rect::from_min_max(at([l, t]), at([r, b]));
        if more && slot == visible {
            painter.rect(rect, 4.0, fill, line, egui::StrokeKind::Inside);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "...",
                egui::FontId::proportional(unit * 0.6),
                egui::Color32::from_white_alpha(200),
            );
            continue;
        }
        let selected = inventory.selected() == slot;
        let stroke = if selected {
            egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 220, 60))
        } else {
            line
        };
        painter.rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
        if numbers {
            painter.text(
                rect.left_top() + egui::vec2(unit * 0.1, unit * 0.05),
                egui::Align2::LEFT_TOP,
                ((slot + 1) % 10).to_string(),
                egui::FontId::proportional(unit * 0.32),
                egui::Color32::from_white_alpha(170),
            );
        }
        let equipped = |i| {
            overlay
                .player
                .and_then(|id| overlay.game.equipment.get(&id))
                .is_some_and(|e| e.iter().any(|(_, x)| x == i))
        };
        if let Some(item) =
            inventory.hotbar(slot).filter(|&i| inventory.item_count(i) > 0 || equipped(i))
        {
            let (name, color) = item_label(overlay, item);
            painter.rect_filled(rect.shrink(unit * 0.25), 3.0, color);
            painter.text(
                rect.center_bottom() - egui::vec2(0.0, unit * 0.1),
                egui::Align2::CENTER_BOTTOM,
                short_name(&name),
                egui::FontId::proportional(unit * 0.3),
                egui::Color32::WHITE,
            );
            if inventory.item_count(item) > 0 {
                painter.text(
                    rect.right_top() + egui::vec2(-unit * 0.1, unit * 0.05),
                    egui::Align2::RIGHT_TOP,
                    inventory.item_count(item).to_string(),
                    egui::FontId::proportional(unit * 0.34),
                    egui::Color32::WHITE,
                );
            }
        }
    }
    // 選んでいるアイテムの名前と数 (画面の上)。
    if let Some(item) = inventory.selected_item() {
        let (name, _) = item_label(overlay, item);
        painter.text(
            egui::pos2(context.content_rect().center().x, unit * 0.6),
            egui::Align2::CENTER_CENTER,
            format!("{name} ({})", inventory.item_count(item)),
            egui::FontId::proportional(unit * 0.6),
            egui::Color32::from_white_alpha(230),
        );
    }
}

/// タッチのない画面 (デスクトップ) のホットバー: 左上に 10 枠、数字キー 1〜0 で選ぶ。
fn draw_desktop_hotbar(context: &egui::Context, overlay: &Overlay<'_>) {
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("charon desktop hotbar"),
    ));
    let scale = 1.0 / context.pixels_per_point();
    let size = context.content_rect().size() * context.pixels_per_point();
    let layout = crate::touch_pad::Layout::new([size.x, size.y], 0.6);
    // 画面の下の中央 (左のパネルと重ならない)。
    let slots = charon_game::inventory::HOTBAR_SLOTS as f32;
    let total = slots * 1.65 * layout.unit;
    let offset = [(size.x - total) / 2.0 - 0.4 * layout.unit, size.y - 2.4 * layout.unit];
    draw_hotbar(
        context,
        &painter,
        scale,
        &layout,
        overlay,
        charon_game::inventory::HOTBAR_SLOTS,
        true,
        offset,
    );
}

/// 倒れたときの表示 (仕様 23 §1): 画面の中央に、復活までの秒数。
fn draw_death_overlay(context: &egui::Context, overlay: &Overlay<'_>) {
    let Some(id) = overlay.player else { return };
    let Some(ticks) = overlay.game.death_ticks(id) else { return };
    let left = charon_game::respawn::RESPAWN_TICKS.saturating_sub(ticks) / 60 + 1;
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("charon death"),
    ));
    let center = context.content_rect().center();
    painter.rect_filled(context.content_rect(), 0.0, egui::Color32::from_black_alpha(90));
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        "You died",
        egui::FontId::proportional(40.0),
        egui::Color32::from_rgb(230, 70, 70),
    );
    painter.text(
        center + egui::vec2(0.0, 40.0),
        egui::Align2::CENTER_CENTER,
        format!("Respawning in {left}"),
        egui::FontId::proportional(18.0),
        egui::Color32::from_white_alpha(220),
    );
}

/// 最初のヒント (仕様 23 §2): 世界の始まりの 25 秒だけ、画面の下に操作を出す。
fn draw_hints(context: &egui::Context, overlay: &Overlay<'_>, world_ticks: u64) {
    if world_ticks > HINT_TICKS || overlay.player.is_none() {
        return;
    }
    let text = if overlay.pad.is_some() {
        "Left stick: walk  |  ^^: jump  |  Big button: use the selected item (tilt to aim)  |  Hotbar: tap  |  []: inventory & crafting"
    } else {
        "A / D: walk  |  W: jump  |  Left click: use the selected item  |  1-0: hotbar  |  I: inventory & crafting  |  Esc: menu"
    };
    egui::Area::new(egui::Id::unique("charon hints"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -72.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(context, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(130))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_max_width(context.content_rect().width() * 0.92);
                    ui.label(egui::RichText::new(text).size(13.0).color(egui::Color32::WHITE));
                });
        });
}

/// ヒントを出す、世界の始まりからの tick (25 秒)。
const HINT_TICKS: u64 = 60 * 25;

/// ボスの体力のバー (画面の上の中央、仕様 22 §4)。ボスがいるときだけ。
fn draw_boss_bar(context: &egui::Context, overlay: &Overlay<'_>) {
    let boss = overlay.game.enemies.iter().find(|(id, enemy)| {
        enemy.def.boss && overlay.game.vitals.get(**id).is_some_and(|v| v.alive())
    });
    let Some((id, enemy)) = boss else { return };
    let Some(vitals) = overlay.game.vitals.get(*id) else { return };
    let fraction = if vitals.max_hp > 0 {
        (vitals.hp as f32 / vitals.max_hp as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    egui::Area::new(egui::Id::unique("charon boss bar"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 36.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(context, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(320.0, 14.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(150));
            let fill = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(rect.width() * fraction, rect.height()),
            );
            painter.rect_filled(fill, 3.0, egui::Color32::from_rgb(200, 50, 70));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &enemy.def.name,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
        });
}

/// 体力のハート (右上、10 個ずつ 1〜2 段、1 個 = 体力 20) とマナの星 (右端に縦、1 個 = マナ 20)。仕様 18 §1。
/// 半分以上残っていれば塗り、足りない分は暗くする。ハートは 2 つの円と 1 つの三角形、星は 10 個の三角形で描く (凸の図形だけ)。
fn draw_hearts_and_stars(context: &egui::Context, overlay: &Overlay<'_>) {
    let Some(id) = overlay.player else { return };
    let (Some(vitals), Some(wand)) = (overlay.game.vitals.get(id), overlay.game.wands.get(&id))
    else {
        return;
    };
    let count = |max: i64| usize::try_from((max.max(0) + 19) / 20).unwrap_or(0);
    let hearts = count(i64::from(vitals.max_hp)).min(20);
    let stars = count(i64::from(wand.mana_max)).min(10);
    let (size, gap) = (18.0, 3.0);
    egui::Area::new(egui::Id::unique("charon hearts"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(context, |ui| {
            let rows = hearts.div_ceil(10).max(1);
            let width = 10.0 * (size + gap) + size + 2.0 * gap;
            let tall = (stars as f32 * (size + gap)).max(rows as f32 * (size + gap));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, tall), egui::Sense::hover());
            let painter = ui.painter();
            let dim = egui::Color32::from_black_alpha(120);
            for i in 0..hearts {
                let left = (i64::from(vitals.hp) - i as i64 * 20).clamp(0, 20);
                let color = if left >= 10 { egui::Color32::from_rgb(220, 50, 60) } else { dim };
                let (col, row) = (i % 10, i / 10);
                let center = egui::pos2(
                    rect.left() + (col as f32 + 0.5) * (size + gap),
                    rect.top() + (row as f32 + 0.5) * (size + gap),
                );
                let r = size * 0.27;
                painter.circle_filled(center + egui::vec2(-r, -r * 0.5), r * 1.05, color);
                painter.circle_filled(center + egui::vec2(r, -r * 0.5), r * 1.05, color);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        center + egui::vec2(-r * 2.0, -r * 0.2),
                        center + egui::vec2(r * 2.0, -r * 0.2),
                        center + egui::vec2(0.0, r * 2.4),
                    ],
                    color,
                    egui::Stroke::NONE,
                ));
            }
            for i in 0..stars {
                let left = (i64::from(wand.mana.min(1_000_000)) - i as i64 * 20).clamp(0, 20);
                let color = if left >= 10 { egui::Color32::from_rgb(90, 130, 255) } else { dim };
                let center = egui::pos2(
                    rect.right() - (size + gap) * 0.5,
                    rect.top() + (i as f32 + 0.5) * (size + gap),
                );
                let (outer, inner) = (size * 0.5, size * 0.22);
                let point = |k: usize| {
                    let angle =
                        std::f32::consts::PI * (k as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
                    let radius = if k.is_multiple_of(2) { outer } else { inner };
                    center + egui::vec2(angle.cos(), angle.sin()) * radius
                };
                for k in 0..10 {
                    painter.add(egui::Shape::convex_polygon(
                        vec![center, point(k), point((k + 1) % 10)],
                        color,
                        egui::Stroke::NONE,
                    ));
                }
            }
        });
}

fn draw_hud_bars(context: &egui::Context, overlay: &Overlay<'_>) {
    use charon_game::status::StatusKind;
    let Some(id) = overlay.player else { return };
    let (Some(vitals), Some(wand)) = (overlay.game.vitals.get(id), overlay.game.wands.get(&id))
    else {
        return;
    };
    let fraction =
        |value: f32, max: f32| if max > 0.0 { (value / max).clamp(0.0, 1.0) } else { 0.0 };
    let _ = wand;
    let mut bars: Vec<(f32, egui::Color32)> = Vec::new();
    if let Some(lev) = overlay.game.levitation.get(&id) {
        bars.push((
            fraction(f32::from(lev.fuel), f32::from(lev.def.fuel_max)),
            egui::Color32::from_rgb(230, 180, 60),
        ));
    }
    if vitals.breath < charon_game::vitals::BREATH_MAX {
        bars.push((
            fraction(f32::from(vitals.breath), f32::from(charon_game::vitals::BREATH_MAX)),
            egui::Color32::from_rgb(90, 200, 230),
        ));
    }
    let chips: Vec<egui::Color32> = [
        (StatusKind::Burning, egui::Color32::from_rgb(240, 130, 40)),
        (StatusKind::Wet, egui::Color32::from_rgb(90, 170, 240)),
        (StatusKind::Poison, egui::Color32::from_rgb(110, 210, 90)),
    ]
    .into_iter()
    .filter(|(kind, _)| overlay.game.statuses.has(id, *kind))
    .map(|(_, color)| color)
    .collect();
    egui::Area::new(egui::Id::unique("charon hud"))
        .anchor(
            egui::Align2::RIGHT_TOP,
            egui::vec2(-40.0, 12.0 + 24.0 * (f32::from(vitals.max_hp > 200) + 1.0)),
        )
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(context, |ui| {
            let (width, height, gap) = (90.0, 7.0, 3.0);
            let total =
                bars.len() as f32 * (height + gap) + if chips.is_empty() { 0.0 } else { 9.0 };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, total), egui::Sense::hover());
            let painter = ui.painter();
            for (row, (value, color)) in bars.iter().enumerate() {
                let top = rect.top() + row as f32 * (height + gap);
                let track = egui::Rect::from_min_size(
                    egui::pos2(rect.left(), top),
                    egui::vec2(width, height),
                );
                painter.rect_filled(track, 2.0, egui::Color32::from_black_alpha(110));
                let fill = egui::Rect::from_min_size(track.min, egui::vec2(width * value, height));
                painter.rect_filled(fill, 2.0, *color);
            }
            let chip_top = rect.top() + bars.len() as f32 * (height + gap);
            for (i, color) in chips.iter().enumerate() {
                let at = egui::pos2(rect.right() - 6.0 - i as f32 * 9.0, chip_top + 3.0);
                painter.rect_filled(
                    egui::Rect::from_center_size(at, egui::vec2(6.0, 6.0)),
                    1.0,
                    *color,
                );
            }
        });
}

/// `Debug` パネルに出す、プレイヤーの文字の情報: 体力・息・マナの数、道具、持ち物、状態異常、次の呪文。
fn player_info(ui: &mut egui::Ui, overlay: &Overlay<'_>) {
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
    ui.label(format!(
        "HP {}/{} · Breath {} · Mana {}/{}",
        vitals.hp,
        vitals.max_hp,
        vitals.breath / 60,
        wand.mana,
        wand.mana_max
    ));
    if let Some(lev) = overlay.game.levitation.get(&id) {
        ui.label(format!("Fuel (hold jump to hover): {}", lev.fuel / 60));
    }
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
            .map(|(material, count)| (count, overlay.world.materials().get(material).name.as_str()))
            .collect();
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        let shown: Vec<String> =
            items.iter().take(4).map(|(count, name)| format!("{name} {count}")).collect();
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
}

/// 更新距離 (チャンク) のスライダー: 0 は `Auto` (画面のまわりだけ)。カメラからこの距離の外のチャンクは、
/// コールドフリーズする (砂・水・煙・熱の更新を一切しない、仕様 02 §9.4)。
fn sim_distance_slider(ui: &mut egui::Ui, distance: &mut u32) {
    let response =
        ui.add(
            egui::Slider::new(distance, 0..=charon_world::streaming::MAX_SIM_DISTANCE)
                .text("Sim distance (chunks)")
                .custom_formatter(|value, _| {
                    if value < 0.5 { "Auto".to_owned() } else { format!("{value:.0}") }
                })
                .custom_parser(|text| {
                    let text = text.trim();
                    if text.eq_ignore_ascii_case("auto") { Some(0.0) } else { text.parse().ok() }
                }),
        );
    response.on_hover_text("Chunks farther than this from the camera are frozen (not simulated).");
}

/// 閉じたパネルを、画面の左端に縦に並べた細いタブ (`|▸ 名前`) にする。触れると、そのパネルが開く (仕様 10 §14)。
/// 開いているパネルは、タイトルの `x` で閉じる (= このタブになる)。
fn draw_dock(context: &egui::Context, open: &mut [bool; PANELS.len()]) {
    if open.iter().all(|&o| o) {
        return;
    }
    egui::Area::new(egui::Id::unique("charon dock"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 44.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
            for (index, (_, short)) in PANELS.iter().enumerate() {
                if open[index] {
                    continue;
                }
                let tab = egui::Button::new(
                    egui::RichText::new(format!("|> {short}"))
                        .size(11.0)
                        .color(egui::Color32::from_white_alpha(230)),
                )
                .fill(egui::Color32::from_black_alpha(120))
                .corner_radius(egui::CornerRadius { nw: 0, ne: 6, sw: 0, se: 6 })
                .min_size(egui::vec2(44.0, 30.0));
                if ui.add(tab).clicked() {
                    open[index] = true;
                }
            }
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
        // 21 組み込み材質から 2 つ除いた 19 + 他の MOD 材質 8 + 虹色 2。
        assert_eq!(list.len(), 19 + 8 + 2);
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
        assert_eq!(pointer_cell([40.0, 48.0], 4.0, [0.0, 0.0]), Some(CellPos::new(10, 12)));
        assert_eq!(pointer_cell([0.0, 0.0], 4.0, [-8.0, 3.5]), Some(CellPos::new(-8, 3)));
        assert_eq!(pointer_cell([f32::NAN, 0.0], 4.0, [0.0, 0.0]), None);
        assert_eq!(pointer_cell([1.0, 1.0], 0.0, [0.0, 0.0]), Some(CellPos::new(1, 1)));
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
            defs: None,
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

    /// 仕様 23: 最初のヒント・倒れたときの表示・デスクトップのホットバーが出る。
    #[test]
    fn hints_death_overlay_and_desktop_hotbar_draw() {
        use charon_game::defs::Definitions;
        use charon_game::game::GameState;
        use charon_game::vitals::{MAX_HP, Vitals};
        let materials = std::sync::Arc::new(MaterialTable::builtin().unwrap());
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let world = world_from_ascii(&box_art(40, 40), 1);
        let physics = PhysicsEngine::new();
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(8 * 16, 4 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        let pick = defs.item_id("wood_pickaxe", world.materials()).unwrap();
        let inventory = game.inventories.get_mut(&player).unwrap();
        inventory.add_item(pick, 1);
        inventory.pin(0, pick);
        let shapes =
            |game: &GameState, entities: &EntityWorld, with_player: bool, ticks: u64| -> usize {
                let overlay = Overlay {
                    world: &world,
                    physics: &physics,
                    entities,
                    game,
                    player: with_player.then_some(player),
                    pad: None,
                    defs: Some(&defs),
                };
                let mut demo = DemoUi::new(world.materials());
                let mut controls = Controls::new(material_of(&world, 's'));
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
                    let mut output = context.run_pass(input, |context| {
                        demo.draw(
                            context,
                            &mut controls,
                            &overlay,
                            FrameStats { frame_ms: 2.0, tick_ms: 1.0, phys_ms: 0.1 },
                            ticks,
                        );
                    });
                    count = output.shapes.len();
                    output.textures_delta.clear();
                }
                count
            };
        let without = shapes(&game, &entities, false, 100_000);
        let hotbar = shapes(&game, &entities, true, 100_000);
        assert!(hotbar > without + 10, "デスクトップのホットバーが出る ({without} → {hotbar})");
        let hints = shapes(&game, &entities, true, 0);
        assert!(hints > hotbar, "最初のヒントが出る ({hotbar} → {hints})");
        // 倒れると、復活までの表示が出る。
        game.vitals.get_mut(player).unwrap().hp = 0;
        game.step_respawn(&world, &mut entities, [8, 4]);
        assert!(game.death_ticks(player).is_some());
        let dead = shapes(&game, &entities, true, 100_000);
        assert!(dead > hotbar, "倒れたときの表示が出る ({hotbar} → {dead})");
    }

    /// パネル (仕様 10 §14): タッチの画面では最初はすべて閉じて、左端の細いタブになり、触れると開く。
    /// デスクトップでは `Main Menu` と `Draw` が開いている。
    #[test]
    fn panels_dock_to_the_left_and_open_by_touch() {
        let world = world_from_ascii(&box_art(40, 20), 1);
        let physics = PhysicsEngine::new();
        let entities = EntityWorld::new();
        let game = charon_game::game::GameState::new(Default::default());
        let pad = crate::touch_pad::TouchPad::default();
        let overlay = |pad| Overlay {
            world: &world,
            physics: &physics,
            entities: &entities,
            game: &game,
            player: None,
            pad,
            defs: None,
        };
        let frames =
            |demo: &mut DemoUi, overlay: &Overlay<'_>, taps: &[(u32, egui::Pos2, bool)]| {
                let mut controls = Controls::new(material_of(&world, 's'));
                let context = egui::Context::default();
                let mut last = 0;
                for frame in 0..6u32 {
                    let mut events = Vec::new();
                    for &(at, pos, pressed) in taps {
                        if at == frame {
                            events.push(egui::Event::PointerMoved(pos));
                            events.push(egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            });
                        }
                    }
                    let input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1024.0, 768.0),
                        )),
                        time: Some(f64::from(frame) / 60.0),
                        events,
                        ..Default::default()
                    };
                    let mut output = context.run_pass(input, |context| {
                        demo.draw(
                            context,
                            &mut controls,
                            overlay,
                            FrameStats { frame_ms: 2.0, tick_ms: 1.0, phys_ms: 0.1 },
                            0,
                        );
                    });
                    last = output.shapes.len();
                    output.textures_delta.clear();
                }
                last
            };
        // デスクトップ (パッドなし): `Main Menu` と `Draw` が開いている。
        let mut desktop = DemoUi::new(world.materials());
        frames(&mut desktop, &overlay(None), &[]);
        assert_eq!(desktop.panels, [false, true, true, false, false]);
        // タッチ (パッドあり): すべて閉じている。
        let mut touch = DemoUi::new(world.materials());
        let closed_shapes = frames(&mut touch, &overlay(Some(&pad)), &[]);
        assert_eq!(touch.panels, [false; 5], "画面を隠さない");
        // 左端の最初のタブ (`Dbg`、アンカー (0, 44)、最小 44×30) に触れると、`Debug` が開く。
        let tab = egui::pos2(20.0, 44.0 + 15.0);
        let release = [(2, tab, true), (3, tab, false)];
        let opened_shapes = frames(&mut touch, &overlay(Some(&pad)), &release);
        assert_eq!(touch.panels, [true, false, false, false, false], "触れたタブのパネルが開く");
        assert_ne!(closed_shapes, opened_shapes, "パネルが描かれる");
        // タブの外 (右の方) に触れても開かない。
        let mut other = DemoUi::new(world.materials());
        let far = egui::pos2(700.0, 400.0);
        frames(&mut other, &overlay(Some(&pad)), &[(2, far, true), (3, far, false)]);
        assert_eq!(other.panels, [false; 5]);
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
            defs: None,
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
