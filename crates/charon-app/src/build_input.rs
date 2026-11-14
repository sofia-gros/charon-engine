//! 仕様 18: カーソルと、ホットバーで選んだアイテムの使用 (ブロックを置く・壊す・消耗品)。タッチとマウスで共通。

use charon_core::coords::CellPos;
use charon_game::cursor::{
    Cursor, CursorState, CursorUse, REACH_CELLS, center_of, cursor_from_offset, cursor_state,
    smart_cursor, usage_of,
};
use charon_game::items::{ItemClass, ItemId, ItemKind};

use charon_core::world::World;
use charon_game::game::GameState;
use charon_game::{EntityId, EntityWorld};

use super::PadOutput;
use crate::touch_pad::{CursorView, TouchPad};

/// 使うボタンが、ブロックなどを使い終わったあと、次に使えるまでの tick。
const USE_INTERVAL: u64 = 6;
/// 消耗品の間隔 (tick)。
const CONSUME_INTERVAL: u64 = 40;

/// 1 フレームの結果。
pub(super) struct BuildFrame {
    /// 武器 (杖) を撃ってよいか。ブロック・つるはし・消耗品を選んでいるときは偽。
    pub weapon: bool,
    /// 道具の使用先。持ち続けている間だけ、固定 tick ごとに採掘する。
    pub mine: Option<Cursor>,
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
        return BuildFrame { weapon: true, mine: None };
    };
    if let Some(slot) = pad.select_slot
        && let Some(inventory) = ctx.game.inventories.get_mut(&id)
    {
        inventory.select(usize::from(slot));
    }
    // ホットバーで選んでいるアイテム。持ち物にあるか、装備している (装備すると持ち物から移る) ものだけ。
    let pinned = ctx.game.inventories.get(&id).and_then(|inv| inv.hotbar(inv.selected()));
    let item = pinned.filter(|&i| {
        ctx.game.inventories.get(&id).is_some_and(|inv| inv.item_count(i) > 0)
            || ctx
                .game
                .equipment
                .get(&id)
                .is_some_and(|e| e.iter().any(|(_, equipped)| equipped == i))
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
    let usage = usage_of(item, &defs);
    let weapon = weapon_like(item, &defs);
    let Some(center) = center_of(ctx.entities, id) else {
        ctx.touch_pad.set_cursor(None);
        return BuildFrame { weapon, mine: None };
    };
    if let Some(tilt) = pad.cursor_tilt {
        *ctx.cursor_tilt = tilt;
    }
    // カーソル: タッチの画面ではスティックの倒れ (スマートなら向き) から、そうでなければマウスの位置から。
    let touch = ctx.touch_pad.visible();
    let cursor: Option<Cursor> = if touch {
        let tilt = *ctx.cursor_tilt;
        if ctx.touch_pad.smart() {
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
    // 使う。
    let want = pad.use_item || mouse_use;
    let tick = ctx.world.tick();
    if want
        && tick >= *ctx.use_ready
        && item.is_some()
        && let Some(cursor) = cursor
    {
        let consumable = !weapon && !matches!(usage, CursorUse::Place | CursorUse::Mine);
        if matches!(usage, CursorUse::Place) || consumable {
            let result = ctx.game.use_selected(ctx.world, ctx.entities, id, &defs, cursor);
            *ctx.use_ready = tick + if consumable { CONSUME_INTERVAL } else { USE_INTERVAL };
            if result.is_err() {
                // 置けない・届かない: 少し待って、また試す。
                *ctx.use_ready = tick + USE_INTERVAL;
            }
        }
    }
    let _ = CursorState::Ok;
    BuildFrame { weapon, mine: if want && usage == CursorUse::Mine { cursor } else { None } }
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
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
