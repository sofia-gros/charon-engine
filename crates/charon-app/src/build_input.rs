//! 仕様 18: カーソルと、ホットバーで選んだアイテムの使用 (ブロックを置く・壊す・消耗品)。タッチとマウスで共通。

use charon_core::coords::CellPos;
use charon_game::autoselect::{best_tool_for, is_active_chest, proximity_chest, smart_interact};
use charon_game::cursor::{
    Cursor, CursorState, CursorUse, REACH_CELLS, center_of, cursor_at, cursor_from_offset,
    cursor_state, smart_cursor, usage_of,
};
use charon_game::items::{ItemClass, ItemId, ItemKind};
use charon_game::targeting::{LockOn, LockPriority, ranked_targets};

use charon_core::world::World;
use charon_game::game::GameState;
use charon_game::{EntityId, EntityWorld};

use super::PadOutput;
use crate::settings::{AimMode, ItemCategory, TouchSettings};
use crate::touch_pad::{CursorView, TouchPad};

/// 使うボタンが、ブロックなどを使い終わったあと、次に使えるまでの tick。
const USE_INTERVAL: u64 = 6;
/// 消耗品の間隔 (tick)。
const CONSUME_INTERVAL: u64 = 40;
/// 使うボタンのつまみを、これ以上倒すまでは、ブロック・消耗品を使わない (千分率。仕様 28 §6)。
const CENTRE_TILT: i32 = 150;

/// 1 フレームの結果。
pub(super) struct BuildFrame {
    /// 武器 (杖) を撃ってよいか。ブロック・つるはし・消耗品を選んでいるときは偽。
    pub weapon: bool,
    /// 道具の使用先。持ち続けている間だけ、固定 tick ごとに採掘する。
    pub mine: Option<Cursor>,
    /// 設備・箱の操作は固定 tick へ渡す。
    pub fixture_command: Option<crate::ui::FixtureCommand>,
    /// 画面の操作 (使うボタン・サブ攻撃・世界の指) が、今、使っている。マウスは含まない。
    pub pad_use: bool,
    /// ロック中の敵への向き (1/16 セル単位)。武器はこの向きへ向ける (仕様 28 §8)。
    pub lock_aim: Option<(i32, i32)>,
}

/// 選んでいるアイテムが、武器として撃つものか (何も選んでいない・武器・セルのアイテム)。
fn weapon_like(item: Option<ItemId>, defs: &charon_game::defs::Definitions) -> bool {
    let Some(item) = item else { return true };
    match item.class() {
        ItemClass::Cells(_) => true,
        ItemClass::Block(_) => false,
        ItemClass::Def(_) => {
            matches!(
                defs.item_def(item).map(|d| &d.kind),
                Some(ItemKind::Weapon { .. } | ItemKind::Misc | ItemKind::Armor { .. }) | None
            )
        }
    }
}

/// 世界を直接さわっている指への、プレイヤーからの向き (1/16 セル単位、武器を向ける)。位置が不正なら `None`。
pub(super) fn world_touch_aim(
    entity: &charon_game::Entity,
    at: [f32; 2],
    zoom: f32,
    origin: [f32; 2],
) -> Option<(i32, i32)> {
    crate::demo_ui::pointer_cell(at, zoom, origin).map(|cell| crate::player::aim_at(entity, cell))
}

/// `build_frame` が触る項目 (App の別々の項目を借りる)。
pub(super) struct BuildCtx<'a> {
    pub world: &'a mut World,
    pub entities: &'a mut EntityWorld,
    pub game: &'a mut GameState,
    pub touch_pad: &'a mut TouchPad,
    pub player: Option<EntityId>,
    pub defs: Option<std::sync::Arc<charon_game::defs::Definitions>>,
    pub cursor_tilt: &'a mut (i32, i32),
    pub use_ready: &'a mut u64,
    /// 狙う方式・スマートカーソルの記憶と感度 (仕様 28 §6)。
    pub touch: &'a mut TouchSettings,
    /// ロックオンの状態 (仕様 28 §8)。
    pub lock: &'a mut LockOn,
}

/// ホットバーの選択・カーソルの計算と描画の通知・選んだアイテムの使用をする。`mouse_use` はマウスの左ボタン、
/// `mouse_cell` はマウスのカーソルのセル。
pub(super) fn build_frame(
    ctx: BuildCtx<'_>,
    pad: &PadOutput,
    mouse_use: bool,
    mouse_cell: Option<CellPos>,
    zoom: f32,
    origin: [f32; 2],
) -> BuildFrame {
    let (Some(id), Some(defs)) = (ctx.player, ctx.defs.clone()) else {
        ctx.touch_pad.set_cursor(None);
        return BuildFrame {
            weapon: true,
            mine: None,
            fixture_command: None,
            pad_use: false,
            lock_aim: None,
        };
    };
    if let Some(slot) = pad.select_slot
        && let Some(inventory) = ctx.game.inventories.get_mut(&id)
    {
        inventory.select(usize::from(slot));
    }
    let center_opt = center_of(ctx.entities, id);
    // オートセレクト (仕様 28 §8): 有効で、狙っているとき、対象のブロックに合う道具を一時的に選ぶ。ホットバーの選択は変えない。
    let auto_tool = (pad.auto_select && (pad.world_touch.is_some() || pad.use_item || pad.action))
        .then_some(center_opt)
        .flatten()
        .and_then(|center| {
            let cell = match pad.world_touch {
                Some(at) => crate::demo_ui::pointer_cell(at, zoom, origin),
                None => {
                    let tilt = ctx.touch.scale_tilt(pad.cursor_tilt.unwrap_or(*ctx.cursor_tilt));
                    let reach = i64::from(REACH_CELLS);
                    let off = |v: i32| (i64::from(v) * reach / 1000) as i32;
                    Some(
                        cursor_from_offset(center, (off(tilt.0), off(tilt.1)), CursorUse::Mine)
                            .cell,
                    )
                }
            }?;
            best_tool_for(ctx.world, ctx.game, id, &defs, cursor_at(cell, CursorUse::Mine).block)
        });
    if let Some(inventory) = ctx.game.inventories.get_mut(&id) {
        inventory.set_auto_item(auto_tool);
    }
    // ホットバーで選んでいるアイテム。持ち物にあるか、装備している (装備すると持ち物から移る) ものだけ。
    let pinned = ctx.game.inventories.get(&id).and_then(|inv| inv.hotbar(inv.selected()));
    let item = ctx.game.inventories.get(&id).and_then(|inv| inv.auto_item()).or_else(|| {
        pinned.filter(|&i| {
            ctx.game.inventories.get(&id).is_some_and(|inv| inv.item_count(i) > 0)
                || ctx
                    .game
                    .equipment
                    .get(&id)
                    .is_some_and(|e| e.iter().any(|(_, equipped)| equipped == i))
        })
    });
    // ホットバーで武器を選んだら、その武器を装備する (仕様 20 §1)。持ち物の中の別の武器と入れ替わる。
    if let Some(weapon) = item
        && matches!(defs.item_def(weapon).map(|d| &d.kind), Some(ItemKind::Weapon { .. }))
        && ctx.game.equipment.get(&id).and_then(|e| e.get(charon_game::items::EquipSlot::Weapon))
            != Some(weapon)
        && let Err(err) = ctx.game.equip(id, weapon, &defs)
    {
        eprintln!("武器を装備できません: {err}");
    }
    // 消耗品の枠をタップしたら、選ぶと同時にすぐ使う (仕様 28 §7)。間隔の間は選ぶだけ。クイック回復・クイックマナも同じ間隔。
    let tick = ctx.world.tick();
    let consumable =
        |i: ItemId| matches!(defs.item_def(i).map(|d| &d.kind), Some(ItemKind::Consumable { .. }));
    if pad.select_slot.is_some()
        && tick >= *ctx.use_ready
        && let Some(drink) = item.filter(|&i| consumable(i))
        && ctx.game.use_consumable(id, drink, &defs)
    {
        *ctx.use_ready = tick + CONSUME_INTERVAL;
    }
    let heal_item = ctx.game.quick_heal_item(id, &defs);
    let mana_item = ctx.game.quick_mana_item(id, &defs);
    ctx.touch_pad.set_quick(heal_item.is_some(), mana_item.is_some());
    for (pressed, quick) in [(pad.quick_heal, heal_item), (pad.quick_mana, mana_item)] {
        if pressed
            && tick >= *ctx.use_ready
            && let Some(drink) = quick
            && ctx.game.use_consumable(id, drink, &defs)
        {
            *ctx.use_ready = tick + CONSUME_INTERVAL;
        }
    }
    let fixture_item = item.filter(|&i| defs.fixture_for_item(i).is_some());
    let usage = if fixture_item.is_some() { CursorUse::Place } else { usage_of(item, &defs) };
    let weapon = fixture_item.is_none() && weapon_like(item, &defs);
    // 種類ごとの狙う方式とスマートカーソル (仕様 28 §6)。切り替えのボタンは、今の種類に効く。
    let category = match usage {
        CursorUse::Place => ItemCategory::Block,
        CursorUse::Mine => ItemCategory::Tool,
        CursorUse::Free if weapon => ItemCategory::Weapon,
        CursorUse::Free => ItemCategory::Other,
    };
    if pad.toggle_aim {
        ctx.touch.toggle_aim(category);
    }
    if pad.toggle_smart {
        ctx.touch.toggle_smart(category);
    }
    let aim_mode = ctx.touch.aim_mode(category);
    let smart = ctx.touch.smart(category);
    ctx.touch_pad.set_modes(aim_mode == AimMode::AimOnly, smart);
    let Some(center) = center_opt else {
        ctx.touch_pad.set_cursor(None);
        return BuildFrame {
            weapon,
            mine: None,
            fixture_command: None,
            pad_use: false,
            lock_aim: None,
        };
    };
    if let Some(tilt) = pad.cursor_tilt {
        *ctx.cursor_tilt = tilt;
    }
    // カーソル: タッチの画面ではスティックの倒れ (スマートなら向き) から、そうでなければマウスの位置から。
    let touch = ctx.touch_pad.visible();
    let cursor: Option<Cursor> = if let Some(at) = pad.world_touch {
        // 世界を直接さわっている指 (仕様 28 §5): 指の場所のセルを指す。届く範囲の外なら、枠は赤で何も起きない。
        crate::demo_ui::pointer_cell(at, zoom, origin).map(|cell| cursor_at(cell, usage))
    } else if touch {
        let tilt = ctx.touch.scale_tilt(*ctx.cursor_tilt);
        if smart {
            let dir = ctx.touch_pad.last_aim().unwrap_or(tilt);
            smart_cursor(ctx.world, center, dir, usage)
        } else {
            let reach = i64::from(REACH_CELLS);
            let off = |v: i32| (i64::from(v) * reach / 1000) as i32;
            Some(cursor_from_offset(center, (off(tilt.0), off(tilt.1)), usage))
        }
    } else {
        mouse_cell.map(|cell| {
            let offset = (cell.x.saturating_sub(center.x), cell.y.saturating_sub(center.y));
            cursor_from_offset(center, offset, usage)
        })
    };
    let state = cursor.map(|c| cursor_state(ctx.world, center, c, usage));
    // 描画用の枠 (ブロックを使うときだけ)。
    let view = match (cursor, state, usage) {
        (Some(c), Some(state), CursorUse::Place | CursorUse::Mine) => {
            let o = c.block.origin().unwrap_or(c.cell);
            let to =
                |x: i32, y: i32| [(x as f32 - origin[0]) * zoom, (y as f32 - origin[1]) * zoom];
            let a = to(o.x, o.y);
            let b = to(o.x + 8, o.y + 8);
            Some(CursorView { rect: [a[0], a[1], b[0], b[1]], state })
        }
        _ => None,
    };
    ctx.touch_pad.set_cursor(view);
    // ロックオン (仕様 28 §8): 押すとロック・次の候補・解除。ロック中は、武器をその敵へ向ける。
    let priority: LockPriority = ctx.touch.lock_on_priority.into();
    let cursor_cell = cursor.map(|c| c.cell);
    if pad.lock_on {
        ctx.lock.press(ctx.world, ctx.entities, ctx.game, center, cursor_cell, priority);
    }
    ctx.lock.update(ctx.world, ctx.entities, ctx.game, center, priority);
    let lock_aim = ctx
        .lock
        .target_cell(ctx.entities)
        .zip(ctx.entities.get(id))
        .map(|(cell, entity)| crate::player::aim_at(entity, cell));
    let candidate =
        ranked_targets(ctx.world, ctx.entities, ctx.game, center, cursor_cell, priority)
            .first()
            .copied();
    ctx.touch_pad.set_lock(ctx.lock.target, candidate);
    // インタラクト (仕様 28 §8): カーソルの箱 (スマート)、なければ、動かず狙わずカーソルが別の箱の上にないとき足元の箱 (近接)。
    let mut interact_cursor = None;
    if pad.interact {
        interact_cursor =
            cursor.and_then(|c| smart_interact(ctx.world, ctx.game, &defs, center, c));
        if interact_cursor.is_none() {
            let aiming = pad.cursor_tilt.is_some() || pad.world_touch.is_some() || pad.action;
            let over_chest =
                cursor.is_some_and(|c| is_active_chest(ctx.world, ctx.game, &defs, c.block));
            if !aiming && pad.horizontal == 0 && !over_chest {
                interact_cursor = proximity_chest(ctx.world, ctx.game, &defs, center);
            }
        }
    }
    // 使う。スティックは Aim & Use のとき (ブロック・消耗品は、つまみを倒してから)、サブ攻撃ボタンはいつでも、
    // 世界の指は触れている間、使う (仕様 28 §5・§6)。設備と箱は入力を記録し、次の固定 tick で検証して適用する。
    let tilted = pad.cursor_tilt.is_some_and(|(x, y)| x.abs().max(y.abs()) >= CENTRE_TILT);
    let careful = matches!(category, ItemCategory::Block | ItemCategory::Other);
    let stick_use = pad.use_item && aim_mode == AimMode::AimUse && (tilted || !careful);
    let pad_use = stick_use || pad.action || pad.world_touch.is_some();
    let want = pad_use || mouse_use;
    let mut fixture_command = None;
    if want
        && tick >= *ctx.use_ready
        && let Some(cursor) = cursor
    {
        if let Some(fixture_item) = fixture_item {
            if state == Some(CursorState::Ok) {
                fixture_command = Some(crate::ui::FixtureCommand::Place(cursor, fixture_item));
            }
            *ctx.use_ready = tick + USE_INTERVAL;
        } else if usage != CursorUse::Mine
            && ctx.game.fixtures.active(ctx.world, cursor.block)
            && ctx.game.fixtures.get(cursor.block).is_some_and(|fixture| {
                defs.item_id(&fixture.item_name, ctx.world.materials())
                    .and_then(|item| defs.fixture_for_item(item))
                    .is_some_and(|def| def.kind == charon_game::defs::FixtureKind::Chest)
            })
            && cursor_state(ctx.world, center, cursor, CursorUse::Free) == CursorState::Ok
        {
            fixture_command = Some(crate::ui::FixtureCommand::Open(cursor));
            *ctx.use_ready = tick + USE_INTERVAL;
        } else if item.is_some() {
            let consumable = !weapon && !matches!(usage, CursorUse::Place | CursorUse::Mine);
            if matches!(usage, CursorUse::Place) || consumable {
                let result = ctx.game.use_selected(ctx.world, ctx.entities, id, &defs, cursor);
                *ctx.use_ready = tick + if consumable { CONSUME_INTERVAL } else { USE_INTERVAL };
                if result.is_err() {
                    *ctx.use_ready = tick + USE_INTERVAL;
                }
            }
        }
    }
    if fixture_command.is_none()
        && let Some(chest) = interact_cursor
    {
        fixture_command = Some(crate::ui::FixtureCommand::Open(chest));
    }
    BuildFrame {
        weapon: weapon && !matches!(fixture_command, Some(crate::ui::FixtureCommand::Open(_))),
        mine: if want && usage == CursorUse::Mine && fixture_command.is_none() {
            cursor
        } else {
            None
        },
        fixture_command,
        pad_use,
        lock_aim,
    }
}

/// スロット間の移動 (仕様 19 §3.1)。検証してから変更し、失敗したら持ち物・印・装備を変えない。
fn move_inventory_slot(
    game: &mut GameState,
    id: EntityId,
    defs: &charon_game::defs::Definitions,
    source: crate::ui::InvSlot,
    target: crate::ui::InvSlot,
) -> bool {
    use crate::ui::InvSlot;
    use charon_game::inventory::HOTBAR_SLOTS;
    let Some(inventory) = game.inventories.get(&id) else { return false };
    match (source, target) {
        (InvSlot::Bag(Some(item)), InvSlot::Hotbar(slot)) => {
            game.inventories.get_mut(&id).is_some_and(|inv| inv.pin(slot, item))
        }
        (InvSlot::Hotbar(a), InvSlot::Hotbar(b)) => {
            game.inventories.get_mut(&id).is_some_and(|inv| inv.swap_hotbar(a, b))
        }
        (InvSlot::Hotbar(slot), InvSlot::Bag(_)) if slot < HOTBAR_SLOTS => {
            game.inventories.get_mut(&id).map(|inv| inv.unpin(slot)).is_some()
        }
        (InvSlot::Bag(Some(item)), InvSlot::Equipment(slot)) => {
            if defs.item_def(item).and_then(|def| def.equip_slot()) != Some(slot) {
                return false;
            }
            game.equip(id, item, defs).is_ok()
        }
        (InvSlot::Hotbar(index), InvSlot::Equipment(slot)) => {
            let Some(item) = inventory.hotbar_active(index) else { return false };
            if defs.item_def(item).and_then(|def| def.equip_slot()) != Some(slot) {
                return false;
            }
            game.equip(id, item, defs).is_ok()
        }
        (InvSlot::Equipment(slot), InvSlot::Bag(_)) => {
            let Some(item) = game.equipment.get(&id).and_then(|eq| eq.get(slot)) else {
                return false;
            };
            inventory.can_hold(item, 1) && game.unequip(id, slot).is_ok()
        }
        (InvSlot::Equipment(source_slot), InvSlot::Hotbar(target_slot)) => {
            if target_slot >= HOTBAR_SLOTS {
                return false;
            }
            let Some(item) = game.equipment.get(&id).and_then(|eq| eq.get(source_slot)) else {
                return false;
            };
            if !inventory.can_hold(item, 1) || game.unequip(id, source_slot).is_err() {
                return false;
            }
            // 枠の範囲と持ち物は上で確かめてあるため、ここでは失敗しない。
            game.inventories.get_mut(&id).is_some_and(|inv| inv.pin(target_slot, item))
        }
        _ => false,
    }
}

/// 持ち物の画面からの操作を、ゲームに反映する (仕様 19)。失敗した操作は何も変えない。
pub(super) fn apply_inventory_commands(
    game: &mut GameState,
    world: &World,
    entities: &EntityWorld,
    id: EntityId,
    defs: &charon_game::defs::Definitions,
    commands: Vec<crate::ui::InvCommand>,
) {
    use crate::ui::InvCommand;
    let nearby = center_of(entities, id)
        .map(|center| game.fixtures.nearby_stations(world, center, defs))
        .unwrap_or_default();
    for command in commands {
        match command {
            InvCommand::Move(source, target) => {
                move_inventory_slot(game, id, defs, source, target);
            }
            InvCommand::Select(slot) => {
                if let Some(inv) = game.inventories.get_mut(&id) {
                    inv.select(slot);
                }
            }
            InvCommand::Pin(slot, item) => {
                if let Some(inv) = game.inventories.get_mut(&id) {
                    inv.pin(slot, item);
                }
            }
            InvCommand::Equip(item) => {
                if let Err(err) = game.equip(id, item, defs) {
                    eprintln!("装備できません: {err}");
                }
            }
            InvCommand::Unequip(slot) => {
                if let Err(err) = game.unequip(id, slot) {
                    eprintln!("外せません: {err}");
                }
            }
            InvCommand::Craft(recipe) => {
                if let Err(err) = game.craft(id, &recipe, &nearby, defs) {
                    eprintln!("作れません: {err}");
                }
            }
            InvCommand::CraftBlock(material) => {
                game.craft_block(world, id, material);
            }
            InvCommand::Fixture(_) => {} // apply_ui_commands が可変の世界で処理する。
        }
    }
}

/// 画面の持ち物と設備を、キューに積んだ順序のまま固定 tick で適用する。
pub(super) fn apply_ui_commands(
    game: &mut GameState,
    world: &mut World,
    entities: &mut EntityWorld,
    id: EntityId,
    defs: &charon_game::defs::Definitions,
    open_chest: &mut Option<charon_game::blocks::BlockPos>,
    commands: Vec<crate::ui::InvCommand>,
) {
    for command in commands {
        if let crate::ui::InvCommand::Fixture(fixture) = command {
            apply_fixture_commands(game, world, entities, id, defs, open_chest, vec![fixture]);
        } else {
            apply_inventory_commands(game, world, entities, id, defs, vec![command]);
        }
    }
}

/// UI の設備コマンドを固定 tick で順に実行する。失敗時は画面状態もゲーム状態も変えない。
pub(super) fn apply_fixture_commands(
    game: &mut GameState,
    world: &mut World,
    entities: &mut EntityWorld,
    id: EntityId,
    defs: &charon_game::defs::Definitions,
    open_chest: &mut Option<charon_game::blocks::BlockPos>,
    commands: Vec<crate::ui::FixtureCommand>,
) {
    use crate::ui::FixtureCommand;
    use charon_game::defs::FixtureKind;
    let in_reach = |world: &World, entities: &EntityWorld, cursor: Cursor, usage| {
        center_of(entities, id)
            .is_some_and(|center| cursor_state(world, center, cursor, usage) == CursorState::Ok)
    };
    for command in commands {
        match command {
            FixtureCommand::Place(cursor, item) => {
                if in_reach(world, entities, cursor, CursorUse::Place)
                    && let Err(err) =
                        game.place_fixture(world, entities, id, cursor.block, item, defs)
                {
                    eprintln!("設備を置けません: {err:?}");
                }
            }
            FixtureCommand::Open(cursor) => {
                let pos = cursor.block;
                if in_reach(world, entities, cursor, CursorUse::Free)
                    && game.fixtures.active(world, pos)
                    && game.fixtures.get(pos).is_some_and(|fixture| {
                        defs.item_id(&fixture.item_name, world.materials())
                            .and_then(|item| defs.fixture_for_item(item))
                            .is_some_and(|def| def.kind == FixtureKind::Chest)
                    })
                {
                    *open_chest = Some(pos);
                }
            }
            FixtureCommand::Close => *open_chest = None,
            FixtureCommand::Deposit(pos, item, amount)
            | FixtureCommand::Withdraw(pos, item, amount)
                if *open_chest == Some(pos) =>
            {
                let center = pos.origin().map(|p| CellPos::new(p.x + 4, p.y + 4));
                if center.is_some_and(|cell| {
                    in_reach(world, entities, Cursor::at(cell), CursorUse::Free)
                }) {
                    let result = if matches!(command, FixtureCommand::Deposit(..)) {
                        game.transfer_to_chest(world, id, pos, item, amount, defs)
                    } else {
                        game.transfer_from_chest(world, id, pos, item, amount, defs)
                    };
                    if let Err(err) = result {
                        eprintln!("箱の出し入れに失敗: {err:?}");
                    }
                }
            }
            FixtureCommand::Deposit(..) | FixtureCommand::Withdraw(..) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use charon_core::cell::MaterialId;
    use charon_core::material::MaterialTable;
    use charon_core::testkit::{box_art, place, place_block, world_from_ascii};
    use charon_game::blocks::BlockPos;
    use charon_game::defs::Definitions;
    use charon_game::items::EquipSlot;
    use charon_game::vitals::{MAX_HP, Vitals};

    use super::*;
    use crate::ui::InvCommand;

    fn setup() -> (World, EntityWorld, GameState, Definitions, EntityId) {
        setup_with_seed(1)
    }

    fn setup_with_seed(seed: u64) -> (World, EntityWorld, GameState, Definitions, EntityId) {
        let materials = Arc::new(MaterialTable::builtin().unwrap_or_else(|e| panic!("{e}")));
        let defs = Definitions::engine_defaults(&materials).unwrap_or_else(|e| panic!("{e}"));
        let world = world_from_ascii(&box_art(64, 64), seed);
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(10 * 16, 20 * 16, &world).unwrap_or_else(|e| panic!("{e}"));
        let mut game = GameState::from_definitions(&defs);
        let vitals = Vitals::new(MAX_HP).unwrap_or_else(|e| panic!("{e}"));
        game.add_character(id, vitals, defs.wand("basic").unwrap_or_else(|e| panic!("{e}")));
        game.give_inventory(id).unwrap_or_else(|e| panic!("{e}"));
        (world, entities, game, defs, id)
    }

    /// 仕様 24 §7: カーソルの設備配置と箱の UI コマンドは同じ固定 tick の順に反映する。
    #[test]
    fn fixture_ui_routes_craft_and_transfer() {
        use crate::ui::FixtureCommand;
        for seed in 1..=8 {
            let (mut world, mut entities, mut game, defs, id) = setup_with_seed(seed);
            let center = center_of(&entities, id).unwrap();
            let block = BlockPos::containing(center);
            let bench_pos = BlockPos { bx: block.bx + 2, by: block.by };
            let chest_pos = BlockPos { bx: block.bx + 3, by: block.by };
            let bench = defs.item_id("workbench", world.materials()).unwrap();
            let chest = defs.item_id("storage_box", world.materials()).unwrap();
            let wood = world.materials().id("wood").unwrap();
            let inv = game.inventories.get_mut(&id).unwrap();
            inv.add_item(bench, 1);
            inv.add(wood, 25);
            inv.pin(0, bench);
            let mut pad = TouchPad::default();
            let mut tilt = (0, 0);
            let mut ready = 0;
            let bench_cell = bench_pos.origin().unwrap();
            let frame = build_frame(
                BuildCtx {
                    world: &mut world,
                    entities: &mut entities,
                    game: &mut game,
                    touch_pad: &mut pad,
                    player: Some(id),
                    defs: Some(Arc::new(defs.clone())),
                    cursor_tilt: &mut tilt,
                    use_ready: &mut ready,
                    touch: &mut TouchSettings::default(),
                    lock: &mut LockOn::default(),
                },
                &PadOutput::default(),
                true,
                Some(CellPos::new(bench_cell.x + 4, bench_cell.y + 4)),
                1.0,
                [0.0, 0.0],
            );
            assert_eq!(
                frame.fixture_command,
                Some(FixtureCommand::Place(
                    Cursor::at(CellPos::new(bench_cell.x + 4, bench_cell.y + 4)),
                    bench
                )),
                "seed={seed}"
            );
            let mut open = None;
            apply_inventory_commands(
                &mut game,
                &world,
                &entities,
                id,
                &defs,
                vec![crate::ui::InvCommand::Craft("storage_box".into())],
            );
            assert_eq!(game.inventories[&id].item_count(chest), 0, "seed={seed}");
            apply_ui_commands(
                &mut game,
                &mut world,
                &mut entities,
                id,
                &defs,
                &mut open,
                vec![
                    InvCommand::Fixture(frame.fixture_command.unwrap()),
                    InvCommand::Craft("storage_box".into()),
                ],
            );
            assert!(game.fixtures.active(&world, bench_pos));
            assert_eq!(game.inventories[&id].item_count(chest), 1, "seed={seed}");
            let chest_cell = chest_pos.origin().unwrap();
            let cursor = Cursor::at(CellPos::new(chest_cell.x + 4, chest_cell.y + 4));
            apply_fixture_commands(
                &mut game,
                &mut world,
                &mut entities,
                id,
                &defs,
                &mut open,
                vec![FixtureCommand::Place(cursor, chest), FixtureCommand::Open(cursor)],
            );
            assert_eq!(open, Some(chest_pos));
            let wood_item = ItemId::cells(wood);
            apply_fixture_commands(
                &mut game,
                &mut world,
                &mut entities,
                id,
                &defs,
                &mut open,
                vec![FixtureCommand::Deposit(chest_pos, wood_item, 5)],
            );
            assert_eq!(game.fixtures.get(chest_pos).unwrap().contents["cells:wood"], 5);
            let before = game.state_hash();
            apply_fixture_commands(
                &mut game,
                &mut world,
                &mut entities,
                id,
                &defs,
                &mut open,
                vec![FixtureCommand::Withdraw(chest_pos, wood_item, 6)],
            );
            assert_eq!(game.state_hash(), before, "seed={seed}: 失敗時は不変");
            apply_fixture_commands(
                &mut game,
                &mut world,
                &mut entities,
                id,
                &defs,
                &mut open,
                vec![FixtureCommand::Withdraw(chest_pos, wood_item, 2), FixtureCommand::Close],
            );
            assert_eq!(open, None);
            assert_eq!(game.fixtures.get(chest_pos).unwrap().contents["cells:wood"], 3);
        }
    }

    /// 持ち物の画面の操作 (ホットバー・クラフト・装備) がゲームに反映される。
    #[test]
    fn inventory_commands_craft_pin_equip_and_select() {
        let (world, entities, mut game, defs, id) = setup();
        let materials = world.materials();
        let wood = materials.id("wood").unwrap_or_else(|| panic!("wood"));
        game.inventories.get_mut(&id).map(|inv| inv.add(wood, 90));
        let helmet = defs.item_id("wood_helmet", materials).unwrap_or_else(|| panic!("helmet"));
        let pick = defs.item_id("wood_pickaxe", materials).unwrap_or_else(|| panic!("pick"));
        apply_inventory_commands(
            &mut game,
            &world,
            &entities,
            id,
            &defs,
            vec![
                InvCommand::Craft("wood_helmet".to_owned()),
                InvCommand::Craft("wood_pickaxe".to_owned()),
                InvCommand::Craft("wood_pickaxe".to_owned()),
                InvCommand::Equip(helmet),
                InvCommand::Pin(3, pick),
                InvCommand::Select(3),
                InvCommand::CraftBlock(wood),
            ],
        );
        let inv = &game.inventories[&id];
        assert_eq!(game.equipment[&id].get(EquipSlot::Head), Some(helmet));
        assert_eq!(
            inv.item_count(pick),
            1,
            "ヘルメットのあと、40 個でつるはし 1 つ。2 つ目は材料が足りない"
        );
        assert_eq!(inv.count(wood), 0);
        assert_eq!(inv.hotbar(3), Some(pick));
        assert_eq!(inv.selected_item(), Some(pick));
        assert_eq!(inv.block_count(wood), 0, "セルが足りないので、ブロックにならない");
    }

    /// 仕様 19 §3.1: ホットバーの印を交換し、装備を往復しても個数と選択枠は変わらない。
    #[test]
    fn inventory_slot_moves_swap_and_keep_counts() {
        use crate::ui::InvSlot::{Bag, Equipment, Hotbar};
        for seed in 1..=8 {
            let (world, entities, mut game, defs, id) = setup_with_seed(seed);
            let materials = world.materials();
            let pick = defs.item_id("wood_pickaxe", materials).unwrap();
            let helmet = defs.item_id("wood_helmet", materials).unwrap();
            let stone = ItemId::cells(materials.id("stone").unwrap());
            let inv = game.inventories.get_mut(&id).unwrap();
            inv.add_item(pick, 2);
            inv.add_item(helmet, 1);
            inv.add_item(stone, 7);
            inv.pin(0, pick);
            inv.pin(1, stone);
            inv.select(0);
            let apply = |game: &mut GameState, source, target| {
                apply_inventory_commands(
                    game,
                    &world,
                    &entities,
                    id,
                    &defs,
                    vec![InvCommand::Move(source, target)],
                );
            };
            apply(&mut game, Hotbar(0), Hotbar(1));
            assert_eq!(
                (game.inventories[&id].hotbar(0), game.inventories[&id].hotbar(1)),
                (Some(stone), Some(pick)),
                "seed={seed}"
            );
            apply(&mut game, Bag(Some(helmet)), Equipment(EquipSlot::Head));
            assert_eq!(game.equipment[&id].get(EquipSlot::Head), Some(helmet));
            assert_eq!(game.inventories[&id].item_count(helmet), 0);
            apply(&mut game, Equipment(EquipSlot::Head), Hotbar(3));
            apply(&mut game, Hotbar(0), Bag(None));
            let inv = &game.inventories[&id];
            assert_eq!(
                (inv.hotbar(0), inv.hotbar(1), inv.hotbar(3)),
                (None, Some(pick), Some(helmet)),
                "seed={seed}"
            );
            assert_eq!(
                (inv.item_count(pick), inv.item_count(helmet), inv.item_count(stone)),
                (2, 1, 7),
                "seed={seed}"
            );
            assert_eq!(inv.selected(), 0, "seed={seed}");
            assert_eq!(game.equipment[&id].get(EquipSlot::Head), None);
        }
    }

    /// 不正な枠、未所持、装備できない種類、持ち切れない交換は全状態を維持する。
    #[test]
    fn inventory_slot_failures_are_atomic() {
        use crate::ui::InvSlot::{Bag, Equipment, Hotbar};
        for seed in 1..=8 {
            let (world, entities, mut game, defs, id) = setup_with_seed(seed);
            let helmet = defs.item_id("wood_helmet", world.materials()).unwrap();
            let pick = defs.item_id("wood_pickaxe", world.materials()).unwrap();
            game.inventories.get_mut(&id).unwrap().add_item(helmet, 1);
            game.equip(id, helmet, &defs).unwrap();
            game.inventories
                .get_mut(&id)
                .unwrap()
                .add_item(helmet, charon_game::inventory::STACK_MAX);
            let before_inventory = game.inventories[&id].clone();
            let before_equipment = game.equipment[&id].clone();
            for (source, target) in [
                (Hotbar(0), Hotbar(10)),
                (Bag(Some(pick)), Hotbar(1)),
                (Bag(Some(helmet)), Equipment(EquipSlot::Weapon)),
                (Equipment(EquipSlot::Head), Bag(None)),
                (Equipment(EquipSlot::Head), Hotbar(2)),
            ] {
                apply_inventory_commands(
                    &mut game,
                    &world,
                    &entities,
                    id,
                    &defs,
                    vec![InvCommand::Move(source, target)],
                );
                assert_eq!(
                    game.inventories[&id], before_inventory,
                    "seed={seed}: {source:?} → {target:?}"
                );
                assert_eq!(
                    game.equipment[&id], before_equipment,
                    "seed={seed}: {source:?} → {target:?}"
                );
            }
        }
    }

    /// UI のドラッグ元と離した枠から 1 件だけ操作を作り、枠外と同じ枠は無視する。
    #[test]
    fn inventory_slot_drop_rejects_invalid_targets() {
        use crate::ui::{InvSlot, slot_drop_command};
        let a = InvSlot::Hotbar(0);
        let b = InvSlot::Hotbar(1);
        assert_eq!(slot_drop_command(a, Some(b)), Some(InvCommand::Move(a, b)));
        assert_eq!(slot_drop_command(a, None), None);
        assert_eq!(slot_drop_command(a, Some(a)), None);
        assert_eq!(slot_drop_command(InvSlot::Bag(None), Some(b)), None);
    }

    /// 印の入れ替えと装備のあと、名前で保存・復帰しても配置を保つ。
    #[test]
    fn inventory_slot_save_roundtrip() {
        use crate::ui::InvSlot::{Bag, Equipment, Hotbar};
        for seed in 1..=8 {
            let (world, entities, mut game, defs, id) = setup_with_seed(seed);
            let pick = defs.item_id("wood_pickaxe", world.materials()).unwrap();
            let helmet = defs.item_id("wood_helmet", world.materials()).unwrap();
            let inv = game.inventories.get_mut(&id).unwrap();
            inv.add_item(pick, 2);
            inv.add_item(helmet, 1);
            inv.pin(0, pick);
            apply_inventory_commands(
                &mut game,
                &world,
                &entities,
                id,
                &defs,
                vec![
                    InvCommand::Move(Hotbar(0), Hotbar(5)),
                    InvCommand::Move(Bag(Some(helmet)), Equipment(EquipSlot::Head)),
                ],
            );
            let save =
                charon_game::save::GameSave::capture(&game, &entities, id, world.materials(), 0)
                    .with_items(&game, id, &defs, world.materials());
            let text = save.to_ron().unwrap();
            let loaded = charon_game::save::GameSave::from_ron(&text).unwrap();
            let (restored_world, mut restored_entities, mut restored_game, _, restored_id) =
                setup_with_seed(seed);
            loaded.apply(&mut restored_game, &mut restored_entities, &restored_world, restored_id);
            loaded.apply_items(&mut restored_game, restored_id, &defs, restored_world.materials());
            assert_eq!(
                restored_game.inventories[&restored_id].hotbar(5),
                Some(pick),
                "seed={seed}"
            );
            assert_eq!(restored_game.inventories[&restored_id].item_count(pick), 2, "seed={seed}");
            assert_eq!(
                restored_game.equipment[&restored_id].get(EquipSlot::Head),
                Some(helmet),
                "seed={seed}"
            );
        }
    }

    /// 入力フレームは道具を即時使用せず、6 tick の待ちにかかわらず固定 tick の採掘先を渡す。
    #[test]
    fn tool_input_routes_to_fixed_ticks_without_instant_break() {
        let (mut world, mut entities, mut game, defs, id) = setup();
        let wood = world.materials().id("wood").unwrap();
        let pick = defs.item_id("wood_pickaxe", world.materials()).unwrap();
        game.inventories.get_mut(&id).unwrap().add_item(pick, 1);
        game.inventories.get_mut(&id).unwrap().pin(0, pick);
        place_block(&mut world, 5, 5, 'W');
        let mut touch_pad = TouchPad::default();
        let mut tilt = (0, 0);
        let mut use_ready = u64::MAX;
        let frame = build_frame(
            BuildCtx {
                world: &mut world,
                entities: &mut entities,
                game: &mut game,
                touch_pad: &mut touch_pad,
                player: Some(id),
                defs: Some(Arc::new(defs.clone())),
                cursor_tilt: &mut tilt,
                use_ready: &mut use_ready,
                touch: &mut TouchSettings::default(),
                lock: &mut LockOn::default(),
            },
            &PadOutput::default(),
            true,
            Some(CellPos::new(44, 44)),
            1.0,
            [0.0, 0.0],
        );
        assert!(!frame.weapon);
        assert_eq!(
            frame.mine.map(|cursor| cursor.block),
            Some(charon_game::blocks::BlockPos { bx: 5, by: 5 })
        );
        assert_eq!(game.inventories[&id].block_count(wood), 0, "入力フレームでは壊さない");
        assert_eq!(use_ready, u64::MAX, "道具は 6 tick 間隔の制限を受けない");
        for _ in 0..37 {
            assert_eq!(
                game.mine_selected_tick(
                    &mut world,
                    &entities,
                    id,
                    &defs,
                    frame.mine.unwrap(),
                    true
                ),
                Ok(charon_game::loadout::UseOutcome::Nothing)
            );
        }
        assert_eq!(
            game.mine_selected_tick(&mut world, &entities, id, &defs, frame.mine.unwrap(), true),
            Ok(charon_game::loadout::UseOutcome::Broken)
        );
        assert_eq!(game.inventories[&id].block_count(wood), 1);
    }

    /// 世界の指のテスト用の一式。
    struct Rig {
        world: World,
        entities: EntityWorld,
        game: GameState,
        defs: Definitions,
        id: EntityId,
        pad: TouchPad,
        ready: u64,
        settings: TouchSettings,
        tilt: (i32, i32),
        lock: LockOn,
    }

    impl Rig {
        fn new(seed: u64) -> Self {
            let (world, entities, game, defs, id) = setup_with_seed(seed);
            let mut pad = TouchPad::default();
            pad.show();
            Self {
                world,
                entities,
                game,
                defs,
                id,
                pad,
                ready: 0,
                settings: TouchSettings::default(),
                tilt: (0, 0),
                lock: LockOn::default(),
            }
        }

        /// 持ち物の `name` を 1 個持たせ、0 番の枠に印して選ぶ。
        fn give_item(&mut self, name: &str) -> ItemId {
            let item = self.defs.item_id(name, self.world.materials()).unwrap();
            let inv = self.game.inventories.get_mut(&self.id).unwrap();
            inv.add_item(item, 1);
            inv.pin(0, item);
            inv.select(0);
            item
        }

        /// 使うボタンのスティックを `tilt` (千分率) へ倒した入力。
        fn stick(tilt: (i32, i32)) -> PadOutput {
            PadOutput { use_item: true, cursor_tilt: Some(tilt), ..PadOutput::default() }
        }

        /// 材質 `name` のブロックを `count` 個持たせ、0 番の枠に印して選ぶ。
        fn give_blocks(&mut self, name: &str, count: u32) -> MaterialId {
            let material = self.world.materials().id(name).unwrap();
            let inv = self.game.inventories.get_mut(&self.id).unwrap();
            inv.add_block(material, count);
            inv.pin(0, ItemId::block(material));
            inv.select(0);
            material
        }

        fn center(&self) -> CellPos {
            center_of(&self.entities, self.id).unwrap()
        }

        /// 入力 1 フレームを流す (ズーム 1、原点 (0, 0) なので、画素 = セル)。
        fn frame(&mut self, pad: &PadOutput) -> BuildFrame {
            build_frame(
                BuildCtx {
                    world: &mut self.world,
                    entities: &mut self.entities,
                    game: &mut self.game,
                    touch_pad: &mut self.pad,
                    player: Some(self.id),
                    defs: Some(Arc::new(self.defs.clone())),
                    cursor_tilt: &mut self.tilt,
                    use_ready: &mut self.ready,
                    touch: &mut self.settings,
                    lock: &mut self.lock,
                },
                pad,
                false,
                None,
                1.0,
                [0.0, 0.0],
            )
        }

        /// 世界の指 1 本の入力フレームを流す。
        fn touch(&mut self, at: [f32; 2]) -> BuildFrame {
            self.frame(&PadOutput { world_touch: Some(at), ..PadOutput::default() })
        }

        /// 箱の設備を `block` に置く (持ち物に 1 個足してから、固定 tick の操作として置く)。
        fn place_chest(&mut self, block: BlockPos) {
            let chest = self.defs.item_id("storage_box", self.world.materials()).unwrap();
            self.game.inventories.get_mut(&self.id).unwrap().add_item(chest, 1);
            let origin = block.origin().unwrap();
            let cursor = Cursor::at(CellPos::new(origin.x + 4, origin.y + 4));
            let mut open = None;
            apply_fixture_commands(
                &mut self.game,
                &mut self.world,
                &mut self.entities,
                self.id,
                &self.defs,
                &mut open,
                vec![crate::ui::FixtureCommand::Place(cursor, chest)],
            );
            assert!(self.game.fixtures.active(&self.world, block), "箱が置かれた");
        }

        /// プレイヤーの中心から `(dx, dy)` セルの位置に敵 (骸骨) を置く。
        fn skeleton(&mut self, dx: i32, dy: i32) -> EntityId {
            let c = self.center();
            self.game
                .spawn_enemy(
                    &self.defs,
                    &mut self.entities,
                    &self.world,
                    "skeleton",
                    (c.x + dx, c.y + dy),
                )
                .unwrap()
        }

        fn blocks(&self, material: MaterialId) -> u32 {
            self.game.inventories[&self.id].block_count(material)
        }

        fn has_block(&self, block: BlockPos) -> bool {
            self.world.get(block.origin().unwrap()).has(charon_core::cell::FLAG_BLOCK)
        }
    }

    /// ブロックを選んで世界をタップすると、触れたセルのブロックの格子に置かれる (仕様 28 §5)。
    #[test]
    fn world_tap_places_a_block_at_the_touched_cell() {
        for seed in 1..=8 {
            let mut rig = Rig::new(seed);
            let stone = rig.give_blocks("stone", 3);
            let block = BlockPos::containing(rig.center());
            let target = BlockPos { bx: block.bx + 2, by: block.by };
            let origin = target.origin().unwrap();
            // ブロックの中の、中心でない場所をさわっても、そのブロックに置かれる。
            rig.touch([(origin.x + 1) as f32 + 0.5, (origin.y + 6) as f32 + 0.5]);
            assert_eq!(rig.blocks(stone), 2, "seed={seed}");
            assert!(rig.has_block(target), "seed={seed}");
            assert_eq!(
                rig.pad.cursor_view().map(|view| view.state),
                Some(CursorState::Ok),
                "seed={seed}: 枠が出る"
            );
        }
    }

    /// 押したまま動かすと、カーソルが指を追い、通ったブロックに続けて置く。間隔 (6 tick) は守る。
    #[test]
    fn world_drag_follows_the_finger_and_places_continuously() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 5);
        let block = BlockPos::containing(rig.center());
        let first = BlockPos { bx: block.bx + 2, by: block.by };
        let second = BlockPos { bx: block.bx + 3, by: block.by };
        let cell = |p: BlockPos| {
            let o = p.origin().unwrap();
            [(o.x + 4) as f32 + 0.5, (o.y + 4) as f32 + 0.5]
        };
        rig.touch(cell(first));
        rig.touch(cell(second));
        assert!(rig.has_block(first));
        assert!(!rig.has_block(second), "間隔の間は置かない");
        charon_core::testkit::run(&mut rig.world, USE_INTERVAL);
        rig.touch(cell(second));
        assert!(rig.has_block(second));
        assert_eq!(rig.blocks(stone), 3);
    }

    /// 届く範囲の外では、カーソルは円の縁に寄らず、枠は赤で、何も置かれない。
    #[test]
    fn world_touch_out_of_reach_does_nothing() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 2);
        let center = rig.center();
        let far = BlockPos::containing(CellPos::new(center.x + REACH_CELLS + 16, center.y));
        let origin = far.origin().unwrap();
        let before = rig.game.state_hash();
        rig.touch([(origin.x + 4) as f32 + 0.5, (origin.y + 4) as f32 + 0.5]);
        assert_eq!(rig.blocks(stone), 2);
        assert!(!rig.has_block(far));
        assert_eq!(rig.pad.cursor_view().map(|view| view.state), Some(CursorState::OutOfReach));
        assert_eq!(rig.game.state_hash(), before, "失敗したら何も変えない");
    }

    /// ブロックの既定は Aim Only: スティックはカーソルを動かすだけで、置かない (仕様 28 §6)。
    #[test]
    fn aim_only_moves_the_cursor_without_using() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 3);
        let frame = rig.frame(&Rig::stick((500, 0)));
        assert!(!frame.pad_use, "スティックだけでは使わない");
        assert_eq!(rig.blocks(stone), 3);
        assert!(rig.pad.cursor_view().is_some(), "カーソルは動く (枠が出る)");
        assert!(rig.pad.aim_only(), "描画に、狙うだけと知らせる");
    }

    /// Aim Only では、サブ攻撃ボタンを押している間、カーソルの場所に使う。スティックを離してもカーソルは残る。
    #[test]
    fn action_button_uses_at_the_cursor_in_aim_only() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 3);
        rig.frame(&Rig::stick((500, 0)));
        let frame = rig.frame(&PadOutput { action: true, ..Rig::stick((500, 0)) });
        assert!(frame.pad_use);
        assert_eq!(rig.blocks(stone), 2, "置いた");
        // スティックを離しても、カーソルは動かした場所に残り、サブ攻撃で続けて使える。
        charon_core::testkit::run(&mut rig.world, USE_INTERVAL);
        let frame = rig.frame(&PadOutput { action: true, ..PadOutput::default() });
        assert!(frame.pad_use);
        assert_eq!(rig.tilt, (500, 0), "倒れを覚えている");
    }

    /// 武器の既定は Aim & Use: スティックで撃ち続ける。
    #[test]
    fn aim_and_use_keeps_the_weapon_firing_with_the_stick() {
        let mut rig = Rig::new(1);
        rig.give_item("starter_wand");
        let frame = rig.frame(&Rig::stick((0, 0)));
        assert!(frame.pad_use && frame.weapon, "中心をたたいただけでも、武器は撃つ");
        assert!(!rig.pad.aim_only());
        let frame = rig.frame(&PadOutput::default());
        assert!(!frame.pad_use, "離したら撃たない");
    }

    /// 切り替えて Aim & Use にしたブロックは、つまみを倒すまで置かない。
    #[test]
    fn centre_tap_does_not_place_blocks() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 3);
        rig.frame(&PadOutput { toggle_aim: true, ..PadOutput::default() });
        assert!(!rig.pad.aim_only(), "Aim & Use に切り替わった");
        let frame = rig.frame(&Rig::stick((0, 0)));
        assert!(!frame.pad_use && rig.blocks(stone) == 3, "中心だけでは置かない");
        let frame = rig.frame(&Rig::stick((149, 0)));
        assert!(!frame.pad_use, "150‰ 未満");
        let frame = rig.frame(&Rig::stick((200, 0)));
        assert!(frame.pad_use);
        assert_eq!(rig.blocks(stone), 2);
    }

    /// 切り替えは、今の種類だけに効く。種類ごとに覚える。
    #[test]
    fn aim_mode_is_remembered_by_item_category() {
        let mut rig = Rig::new(1);
        rig.give_blocks("stone", 1);
        rig.frame(&PadOutput { toggle_aim: true, ..PadOutput::default() });
        rig.frame(&PadOutput { toggle_smart: true, ..PadOutput::default() });
        assert!(!rig.pad.aim_only() && rig.pad.smart(), "ブロックは Aim & Use・スマート");
        rig.give_item("wood_pickaxe");
        rig.frame(&PadOutput::default());
        assert!(!rig.pad.aim_only(), "つるはしは元のまま Aim & Use");
        assert!(rig.pad.smart(), "つるはしは元のままスマート");
        rig.frame(&PadOutput { toggle_smart: true, ..PadOutput::default() });
        assert!(!rig.pad.smart(), "つるはしのスマートだけ切れる");
        rig.give_blocks("stone", 1);
        rig.frame(&PadOutput::default());
        assert!(rig.pad.smart(), "ブロックのスマートは残っている");
    }

    /// 設定「Aim Mode Toggle」「Smart Cursor Toggle」の 4 つの動き。
    #[test]
    fn aim_and_smart_toggle_modes_follow_settings() {
        use crate::settings::{AimModeToggle, SmartCursorToggle};
        let toggles = PadOutput { toggle_aim: true, toggle_smart: true, ..PadOutput::default() };
        // 全部の種類を同じにする。
        let mut rig = Rig::new(1);
        rig.settings.aim_mode_toggle = AimModeToggle::ForAllItems;
        rig.settings.smart_cursor_toggle = SmartCursorToggle::ForAllItems;
        rig.give_blocks("stone", 1);
        rig.frame(&toggles);
        rig.give_item("starter_wand");
        rig.frame(&PadOutput::default());
        for category in crate::settings::ItemCategory::ALL {
            assert_eq!(
                rig.settings.aim_mode(category),
                rig.settings.aim_mode(crate::settings::ItemCategory::Block),
                "{category:?}"
            );
            assert_eq!(
                rig.settings.smart(category),
                rig.settings.smart(crate::settings::ItemCategory::Block)
            );
        }
        // 常に Aim & Use / 常に Off: ボタンは何もしない。
        let mut rig = Rig::new(1);
        rig.settings.aim_mode_toggle = AimModeToggle::AlwaysAimUse;
        rig.settings.smart_cursor_toggle = SmartCursorToggle::AlwaysOff;
        rig.give_blocks("stone", 1);
        rig.frame(&toggles);
        assert!(
            !rig.pad.aim_only() && !rig.pad.smart(),
            "ブロックでも Aim & Use、スマートは切れたまま"
        );
        // 常に Aim Only / 常に On。
        let mut rig = Rig::new(1);
        rig.settings.aim_mode_toggle = AimModeToggle::AlwaysAimOnly;
        rig.settings.smart_cursor_toggle = SmartCursorToggle::AlwaysOn;
        rig.give_item("starter_wand");
        rig.frame(&toggles);
        assert!(rig.pad.aim_only() && rig.pad.smart(), "武器でも Aim Only・スマート");
        let frame = rig.frame(&Rig::stick((900, 0)));
        assert!(!frame.pad_use, "Aim Only の武器は、スティックでは撃たない");
        let frame = rig.frame(&PadOutput { action: true, ..PadOutput::default() });
        assert!(frame.pad_use && frame.weapon, "サブ攻撃で撃つ");
    }

    /// 感度は、スティックの倒れでカーソルが動く距離を変える。
    #[test]
    fn aim_sensitivity_scales_the_tilt() {
        let mut places = Vec::new();
        for sensitivity in [50, 100, 150] {
            let mut rig = Rig::new(1);
            rig.settings.aim_sensitivity = sensitivity;
            rig.give_blocks("stone", 1);
            rig.frame(&Rig::stick((400, 0)));
            let view = rig.pad.cursor_view().unwrap();
            places.push(view.rect[0]);
        }
        assert!(places[0] < places[1] && places[1] < places[2], "{places:?}");
    }

    /// 消耗品の枠をタップすると、選ぶと同時にすぐ使う。道具・武器は選ぶだけ。間隔の間は選ぶだけ (仕様 28 §7)。
    #[test]
    fn hotbar_tap_uses_a_consumable_at_once() {
        let mut rig = Rig::new(1);
        let potion = rig.defs.item_id("heal_potion", rig.world.materials()).unwrap();
        let sword = rig.defs.item_id("wood_sword", rig.world.materials()).unwrap();
        let inv = rig.game.inventories.get_mut(&rig.id).unwrap();
        inv.add_item(potion, 3);
        inv.add_item(sword, 1);
        inv.pin(1, potion);
        inv.pin(2, sword);
        rig.game.vitals.get_mut(rig.id).unwrap().hp = 20;
        let tap = |slot| PadOutput { select_slot: Some(slot), ..PadOutput::default() };
        rig.frame(&tap(2));
        assert_eq!(rig.game.vitals.get(rig.id).unwrap().hp, 20, "武器は選ぶだけ");
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 3);
        rig.frame(&tap(1));
        assert_eq!(rig.game.vitals.get(rig.id).unwrap().hp, 70, "回復薬はすぐ使う");
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 2);
        assert_eq!(rig.game.inventories[&rig.id].selected(), 1, "選ばれてもいる");
        // 間隔の間は、タップしても選ぶだけ。間隔が過ぎたら、また使う。
        rig.frame(&tap(2));
        rig.frame(&tap(1));
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 2, "間隔の間は使わない");
        charon_core::testkit::run(&mut rig.world, CONSUME_INTERVAL);
        rig.frame(&tap(1));
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 1);
        assert_eq!(rig.game.vitals.get(rig.id).unwrap().hp, 100, "上限で止まる");
    }

    /// クイック回復・クイックマナのボタンは、使えるときだけ使い、使えるかを描画に知らせる。
    #[test]
    fn quick_buttons_drink_only_when_useful() {
        let mut rig = Rig::new(1);
        let potion = rig.defs.item_id("heal_potion", rig.world.materials()).unwrap();
        let mana = rig.defs.item_id("mana_potion", rig.world.materials()).unwrap();
        let inv = rig.game.inventories.get_mut(&rig.id).unwrap();
        inv.add_item(potion, 2);
        inv.add_item(mana, 2);
        let press = PadOutput { quick_heal: true, quick_mana: true, ..PadOutput::default() };
        rig.frame(&press);
        assert_eq!(rig.pad.quick_ready(), [false, false], "満タンなので使えない");
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 2);
        rig.game.vitals.get_mut(rig.id).unwrap().hp = 10;
        rig.game.wands.get_mut(&rig.id).unwrap().mana = 0;
        rig.frame(&PadOutput::default());
        assert_eq!(rig.pad.quick_ready(), [true, true], "ボタンが使える");
        rig.frame(&press);
        assert_eq!(rig.game.inventories[&rig.id].item_count(potion), 1, "回復薬を 1 個");
        assert_eq!(rig.game.inventories[&rig.id].item_count(mana), 2, "同じ間隔の中: マナはまだ");
        charon_core::testkit::run(&mut rig.world, CONSUME_INTERVAL);
        rig.frame(&PadOutput { quick_mana: true, ..PadOutput::default() });
        assert_eq!(rig.game.inventories[&rig.id].item_count(mana), 1);
    }

    /// インタラクトのボタン: カーソルの箱は開ける (つるはしを持っていても)。届かなければ開けない (仕様 28 §8)。
    #[test]
    fn smart_interact_opens_the_chest_under_the_cursor() {
        let mut rig = Rig::new(1);
        rig.give_item("wood_pickaxe");
        let near = BlockPos {
            bx: BlockPos::containing(rig.center()).bx + 2,
            ..BlockPos::containing(rig.center())
        };
        rig.place_chest(near);
        let origin = near.origin().unwrap();
        let on_chest = [(origin.x + 4) as f32 + 0.5, (origin.y + 4) as f32 + 0.5];
        // ボタンがなければ、つるはしでさわっても開けない。
        let frame = rig.frame(&PadOutput { world_touch: Some(on_chest), ..PadOutput::default() });
        assert_eq!(frame.fixture_command, None);
        let frame = rig.frame(&PadOutput {
            world_touch: Some(on_chest),
            interact: true,
            ..PadOutput::default()
        });
        assert_eq!(
            frame.fixture_command,
            Some(crate::ui::FixtureCommand::Open(Cursor::at(CellPos::new(
                origin.x + 4,
                origin.y + 4
            ))))
        );
        // 届かない場所の箱は開けない (世界の右端に箱を置いてから、プレイヤーを左端へ移す)。
        let here = BlockPos::containing(rig.center());
        let far = BlockPos { bx: 7, by: here.by };
        rig.place_chest(far);
        rig.entities.get_mut(rig.id).unwrap().x16 = 2 * 16;
        assert!(rig.center().x + REACH_CELLS < far.origin().unwrap().x + 4);
        let origin = far.origin().unwrap();
        let frame = rig.frame(&PadOutput {
            world_touch: Some([(origin.x + 4) as f32 + 0.5, (origin.y + 4) as f32 + 0.5]),
            interact: true,
            ..PadOutput::default()
        });
        assert_eq!(frame.fixture_command, None, "届かない");
    }

    /// 動かず狙わず、カーソルも別の箱の上にないとき、足元の箱を開ける。動いている・狙っているときは開けない。
    #[test]
    fn proximity_interact_opens_the_chest_at_the_feet() {
        let mut rig = Rig::new(1);
        rig.give_item("wood_pickaxe");
        let here = BlockPos::containing(rig.center());
        let feet = BlockPos { bx: here.bx + 1, by: here.by };
        rig.place_chest(feet);
        let press = PadOutput { interact: true, ..PadOutput::default() };
        let origin = feet.origin().unwrap();
        // 足元の箱 (カーソルは自分の位置のまま)。
        let frame = rig.frame(&press);
        let opened = frame.fixture_command.expect("足元の箱を開ける");
        assert_eq!(
            opened,
            crate::ui::FixtureCommand::Open(Cursor::at(CellPos::new(origin.x + 4, origin.y + 4)))
        );
        // 動いている・狙っている (スティックを倒している) ときは開けない。
        assert_eq!(
            rig.frame(&PadOutput { horizontal: 1, ..press }).fixture_command,
            None,
            "動いている"
        );
        let aiming = PadOutput { cursor_tilt: Some((900, -900)), ..press };
        assert_eq!(rig.frame(&aiming).fixture_command, None, "狙っている");
        // 箱から離れていれば開けない (左へ 3 ブロック)。
        let mut far = Rig::new(1);
        far.give_item("wood_pickaxe");
        let here = BlockPos::containing(far.center());
        far.place_chest(BlockPos { bx: here.bx + 4, by: here.by });
        assert_eq!(far.frame(&press).fixture_command, None, "足元ではない");
    }

    /// オートセレクト: 有効で、対象が石ならつるはし、木なら斧を一時的に選ぶ。ホットバーの選択 (剣) は変わらず、解除すると戻る。
    #[test]
    fn auto_select_swaps_the_tool_while_pointing() {
        let mut rig = Rig::new(1);
        let sword = rig.give_item("wood_sword");
        let pick = rig.defs.item_id("stone_pickaxe", rig.world.materials()).unwrap();
        let axe = rig.defs.item_id("wood_axe", rig.world.materials()).unwrap();
        let inv = rig.game.inventories.get_mut(&rig.id).unwrap();
        inv.add_item(pick, 1);
        inv.add_item(axe, 1);
        let here = BlockPos::containing(rig.center());
        let (stone_at, wood_at) =
            (BlockPos { bx: here.bx + 2, by: here.by }, BlockPos { bx: here.bx + 3, by: here.by });
        charon_core::testkit::place_block(&mut rig.world, stone_at.bx, stone_at.by, '#');
        charon_core::testkit::place_block(&mut rig.world, wood_at.bx, wood_at.by, 'W');
        let px = |p: BlockPos| {
            let o = p.origin().unwrap();
            [(o.x + 4) as f32 + 0.5, (o.y + 4) as f32 + 0.5]
        };
        // 一時的に選ばれたアイテム。剣は装備に移るので、選んでいる枠のアイテムは見ない。
        let selected = |rig: &Rig| rig.game.inventories[&rig.id].auto_item();
        // 有効でないときは、剣のまま (オートセレクトは何も選ばない)。
        let frame =
            rig.frame(&PadOutput { world_touch: Some(px(stone_at)), ..PadOutput::default() });
        assert_eq!(selected(&rig), None);
        assert!(frame.mine.is_none() && frame.weapon);
        // 有効: 石にはつるはし。採掘の対象になる。ホットバーの選択は変わらない。
        let auto =
            |at| PadOutput { world_touch: Some(at), auto_select: true, ..PadOutput::default() };
        let frame = rig.frame(&auto(px(stone_at)));
        assert_eq!(selected(&rig), Some(pick));
        assert_eq!(frame.mine.map(|c| c.block), Some(stone_at));
        assert!(!frame.weapon, "道具なので、杖は撃たない");
        assert_eq!(rig.game.inventories[&rig.id].selected(), 0);
        // 木には斧。
        rig.frame(&auto(px(wood_at)));
        assert_eq!(selected(&rig), Some(axe));
        // 何も指していない (指を離した) ときは、元に戻る。
        let frame = rig.frame(&PadOutput { auto_select: true, ..PadOutput::default() });
        assert_eq!(selected(&rig), None, "指していないときは選ばない");
        assert!(frame.weapon);
        // 空きを指しているときも、元のまま。
        rig.frame(&auto(px(BlockPos { bx: here.bx + 5, by: here.by })));
        assert_eq!(selected(&rig), None);
        assert_eq!(
            rig.game.inventories[&rig.id].hotbar(0),
            Some(sword),
            "ホットバーの印は剣のまま"
        );
    }

    /// ロック中も、ブロックを置ける。ロックは壊れない。
    #[test]
    fn lock_on_still_allows_placing_blocks() {
        let mut rig = Rig::new(1);
        let stone = rig.give_blocks("stone", 3);
        let enemy = rig.skeleton(20, -8);
        rig.frame(&PadOutput { lock_on: true, ..PadOutput::default() });
        assert_eq!(rig.lock.target, Some(enemy));
        assert_eq!(rig.pad.lock_view().0, Some(enemy));
        let block = BlockPos::containing(rig.center());
        let target = BlockPos { bx: block.bx + 2, by: block.by + 1 };
        let origin = target.origin().unwrap();
        rig.touch([(origin.x + 4) as f32 + 0.5, (origin.y + 4) as f32 + 0.5]);
        assert_eq!(rig.blocks(stone), 2, "置けた");
        assert!(rig.has_block(target));
        assert_eq!(rig.lock.target, Some(enemy), "ロックは続く");
    }

    /// ロック中は、狙いがその敵へ向く。ロックしていなければ向きは出ない。もう一度押すと (ほかに敵がいなければ) 解除する。
    #[test]
    fn lock_on_aims_weapons_at_the_target() {
        let mut rig = Rig::new(1);
        rig.give_item("starter_wand");
        let frame = rig.frame(&PadOutput::default());
        assert_eq!(frame.lock_aim, None);
        rig.skeleton(20, -8);
        let frame = rig.frame(&PadOutput { lock_on: true, ..PadOutput::default() });
        let aim = frame.lock_aim.expect("ロック中は向きが出る");
        assert!(aim.0 > 100, "右の敵へ: {aim:?}");
        // 敵が左下へ移ったら、向きも変わる。
        let enemy = rig.lock.target.unwrap();
        let entity = rig.entities.get_mut(enemy).unwrap();
        entity.x16 -= 16 * 60;
        entity.y16 += 16 * 10;
        let frame = rig.frame(&PadOutput::default());
        let aim = frame.lock_aim.expect("追う");
        assert!(aim.0 < 0 && aim.1 > 0, "左下へ: {aim:?}");
        // 押すと、ほかに敵がいないので解除する。
        let frame = rig.frame(&PadOutput { lock_on: true, ..PadOutput::default() });
        assert_eq!(frame.lock_aim, None);
        assert_eq!(rig.lock.target, None);
    }

    /// 武器は、プレイヤーから指の場所への向きへ向ける。不正な位置は無視する。
    #[test]
    fn world_touch_aims_weapons_toward_the_finger() {
        let (_world, entities, _game, _defs, id) = setup();
        let player = entities.get(id).unwrap();
        let center = center_of(&entities, id).unwrap();
        let right = world_touch_aim(
            player,
            [(center.x + 30) as f32 + 0.5, center.y as f32 + 0.5],
            1.0,
            [0.0, 0.0],
        )
        .unwrap();
        assert!(right.0 > 400 && right.1.abs() < 40, "{right:?}");
        let up_left = world_touch_aim(
            player,
            [(center.x - 20) as f32 + 0.5, (center.y - 20) as f32 + 0.5],
            1.0,
            [0.0, 0.0],
        )
        .unwrap();
        assert!(up_left.0 < -200 && up_left.1 < -200, "{up_left:?}");
        // ズームと原点を使って、画素からセルへ直す。
        let zoomed = world_touch_aim(
            player,
            [(center.x - 5) as f32 * 4.0 + 2.0, center.y as f32 * 4.0],
            4.0,
            [5.0, 0.0],
        )
        .unwrap();
        assert!(zoomed.0 < 0 || zoomed.0.abs() < 40, "{zoomed:?}");
        assert_eq!(world_touch_aim(player, [f32::NAN, 0.0], 1.0, [0.0, 0.0]), None);
    }

    /// App のクラフト入力はプレイヤー中心から有効な Station を探す。
    #[test]
    fn inventory_commands_require_active_station() {
        for seed in 1..=8 {
            let (mut world, mut entities, mut game, defs, id) = setup_with_seed(seed);
            let center = center_of(&entities, id).unwrap();
            let block = BlockPos::containing(center);
            let pos = BlockPos { bx: block.bx + 3, by: block.by };
            let bench = defs.item_id("workbench", world.materials()).unwrap();
            let furnace = defs.item_id("furnace", world.materials()).unwrap();
            let wood = world.materials().id("wood").unwrap();
            let stone = world.materials().id("stone").unwrap();
            let inv = game.inventories.get_mut(&id).unwrap();
            inv.add_item(bench, 1);
            inv.add(wood, 8);
            inv.add(stone, 40);
            let command = || vec![InvCommand::Craft("furnace".to_owned())];
            apply_inventory_commands(&mut game, &world, &entities, id, &defs, command());
            assert_eq!(game.inventories[&id].item_count(furnace), 0);
            game.place_fixture(&mut world, &mut entities, id, pos, bench, &defs).unwrap();
            apply_inventory_commands(&mut game, &world, &entities, id, &defs, command());
            assert_eq!(game.inventories[&id].item_count(furnace), 1, "seed={seed}");
        }
    }

    /// 完全な Station のみを拾い、普通のブロック・Chest・傷んだ設備は設備としない。
    #[test]
    fn nearby_stations_list_only_active_close_fixtures() {
        let (mut world, mut entities, mut game, defs, id) = setup();
        let bench = defs.item_id("workbench", world.materials()).unwrap();
        let chest = defs.item_id("storage_box", world.materials()).unwrap();
        game.inventories.get_mut(&id).unwrap().add_item(bench, 1);
        game.inventories.get_mut(&id).unwrap().add_item(chest, 1);
        let bench_pos = BlockPos { bx: 5, by: 5 };
        game.place_fixture(&mut world, &mut entities, id, bench_pos, bench, &defs).unwrap();
        game.place_fixture(&mut world, &mut entities, id, BlockPos { bx: 6, by: 5 }, chest, &defs)
            .unwrap();
        place_block(&mut world, 4, 5, '#');
        let center = CellPos::new(44, 44);
        assert_eq!(game.fixtures.nearby_stations(&world, center, &defs), vec![bench]);
        assert!(game.fixtures.nearby_stations(&world, CellPos::new(8, 8), &defs).is_empty());
        place(&mut world, 40, 40, '.');
        assert!(game.fixtures.nearby_stations(&world, center, &defs).is_empty());
    }
}
