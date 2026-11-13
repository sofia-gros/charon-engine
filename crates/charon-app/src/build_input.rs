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
        return BuildFrame { weapon: true };
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
        return BuildFrame { weapon };
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
        if matches!(usage, CursorUse::Place | CursorUse::Mine) || consumable {
            let result = ctx.game.use_selected(ctx.world, ctx.entities, id, &defs, cursor);
            *ctx.use_ready = tick + if consumable { CONSUME_INTERVAL } else { USE_INTERVAL };
            if result.is_err() {
                // 置けない・届かない: 少し待って、また試す。
                *ctx.use_ready = tick + USE_INTERVAL;
            }
        }
    }
    let _ = CursorState::Ok;
    BuildFrame { weapon }
}

/// 近くのブロックのアイテム (作業台の判定用、仕様 19 §4)。中心から 3 ブロックまでを、ブロックごとに 1 セル調べる。
pub(super) fn nearby_blocks(world: &World, center: CellPos) -> Vec<ItemId> {
    let (bx, by) = (center.x >> 3, center.y >> 3);
    let mut found = Vec::new();
    for dy in -3..=3 {
        for dx in -3..=3 {
            let cell = CellPos::new((bx + dx) * 8, (by + dy) * 8);
            let c = world.get(cell);
            if c.has(charon_core::cell::FLAG_BLOCK) {
                let item = ItemId::block(c.material);
                if !found.contains(&item) {
                    found.push(item);
                }
            }
        }
    }
    found
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
    let nearby = center_of(entities, id).map(|c| nearby_blocks(world, c)).unwrap_or_default();
    for command in commands {
        match command {
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
    use charon_core::testkit::{box_art, place_block, world_from_ascii};
    use charon_game::defs::Definitions;
    use charon_game::items::EquipSlot;
    use charon_game::vitals::{MAX_HP, Vitals};

    use super::*;
    use crate::ui::InvCommand;

    fn setup() -> (World, EntityWorld, GameState, Definitions, EntityId) {
        let materials = Arc::new(MaterialTable::builtin().unwrap_or_else(|e| panic!("{e}")));
        let defs = Definitions::engine_defaults(&materials).unwrap_or_else(|e| panic!("{e}"));
        let world = world_from_ascii(&box_art(64, 64), 1);
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

    /// 近くのブロックだけが、作業台の判定に使われる (遠いものは含まない)。
    #[test]
    fn nearby_blocks_lists_only_close_ones() {
        let (mut world, _entities, _game, _defs, _id) = setup();
        place_block(&mut world, 5, 5, '#');
        place_block(&mut world, 7, 5, 's');
        let near = nearby_blocks(&world, CellPos::new(44, 44));
        assert_eq!(near.len(), 2, "{near:?}");
        let far = nearby_blocks(&world, CellPos::new(8, 8));
        assert!(far.is_empty(), "遠いブロックは含まない: {far:?}");
    }
}
