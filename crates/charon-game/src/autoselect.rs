//! 仕様 28 §8: オートセレクト (対象に合う道具を自動で選ぶ) と、箱のインタラクト (スマート・近接)。整数だけ。

use charon_core::cell::FLAG_BLOCK;
use charon_core::coords::CellPos;
use charon_core::material::HarvestTool;
use charon_core::world::World;

use crate::EntityId;
use crate::blocks::BlockPos;
use crate::cursor::{Cursor, CursorState, CursorUse, cursor_state};
use crate::defs::{Definitions, FixtureKind};
use crate::game::GameState;
use crate::items::{ItemId, ItemKind};

/// `block` (完全なブロック) を壊すのに使う道具のうち、持ち物で力が最大のもの。同じ力なら `ItemId` の小さいほう。
///
/// 壊せないもの (空き・不完全なブロック・設備・読み込まれていない場所)、合う道具がないときは `None`。
/// 材質の優先の道具 (`HarvestTool`) で、つるはしの力か斧の力かが決まる ([`GameState::mine_selected_tick`] と同じ規則)。
pub fn best_tool_for(
    world: &World,
    game: &GameState,
    id: EntityId,
    defs: &Definitions,
    block: BlockPos,
) -> Option<ItemId> {
    let cells = block.cells();
    let first = *cells.first()?;
    world.chunk(first.chunk())?;
    let material = world.get(first).material;
    if game.fixtures.get(block).is_some()
        || !cells.iter().all(|&pos| {
            let cell = world.get(pos);
            cell.has(FLAG_BLOCK) && cell.material == material
        })
    {
        return None;
    }
    let harvest = world.materials().get(material).harvest_tool;
    let inventory = game.inventories.get(&id)?;
    inventory
        .item_iter()
        .filter(|&(_, count)| count > 0)
        .filter_map(|(item, _)| match defs.item_def(item).map(|d| &d.kind) {
            Some(ItemKind::Tool { pick, axe }) => {
                let power = match harvest {
                    HarvestTool::Axe => *axe,
                    HarvestTool::Pick => *pick,
                };
                (power > 0).then_some((power, item))
            }
            _ => None,
        })
        .max_by_key(|&(power, item)| (power, std::cmp::Reverse(item)))
        .map(|(_, item)| item)
}

/// `block` が、使える (石の箱として有効な) 箱か。
pub fn is_active_chest(
    world: &World,
    game: &GameState,
    defs: &Definitions,
    block: BlockPos,
) -> bool {
    game.fixtures.active(world, block)
        && game.fixtures.get(block).is_some_and(|fixture| {
            defs.item_id(&fixture.item_name, world.materials())
                .and_then(|item| defs.fixture_for_item(item))
                .is_some_and(|def| def.kind == FixtureKind::Chest)
        })
}

/// スマートインタラクト: カーソルのブロックが有効な箱で、届くなら、そのカーソル。
pub fn smart_interact(
    world: &World,
    game: &GameState,
    defs: &Definitions,
    center: CellPos,
    cursor: Cursor,
) -> Option<Cursor> {
    (is_active_chest(world, game, defs, cursor.block)
        && cursor_state(world, center, cursor, CursorUse::Free) == CursorState::Ok)
        .then_some(cursor)
}

/// 近接インタラクト: プレイヤーの足元 (中心のブロックと、左右・下の隣の合わせて 6 つ) の有効な箱のうち、中心に最も近いもの。
/// 同じ近さなら (by, bx) が小さいほう。
pub fn proximity_chest(
    world: &World,
    game: &GameState,
    defs: &Definitions,
    center: CellPos,
) -> Option<Cursor> {
    let here = BlockPos::containing(center);
    let mut best: Option<(i64, (i32, i32), Cursor)> = None;
    for dy in 0..=1 {
        for dx in -1..=1 {
            let block = BlockPos { bx: here.bx.saturating_add(dx), by: here.by.saturating_add(dy) };
            let Some(origin) = block.origin() else { continue };
            if !is_active_chest(world, game, defs, block) {
                continue;
            }
            let cell = CellPos::new(origin.x + 4, origin.y + 4);
            let (ddx, ddy) = (i64::from(cell.x - center.x), i64::from(cell.y - center.y));
            let key = (ddx * ddx + ddy * ddy, (block.by, block.bx));
            if best.as_ref().is_none_or(|(d, order, _)| (key.0, key.1) < (*d, *order)) {
                best = Some((key.0, key.1, Cursor { cell, block }));
            }
        }
    }
    best.map(|(_, _, cursor)| cursor)
}
