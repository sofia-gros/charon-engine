//! 仕様 18 §2・§6: カーソル (届く円・ブロックの格子・スマートカーソル・使えない場所) と、選んだアイテムを使う。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit::{box_art, material_of, place_block, world_from_ascii};
use charon_core::world::World;
use charon_game::blocks::BlockPos;
use charon_game::cursor::{
    Cursor, CursorState, CursorUse, REACH_CELLS, clamp_to_reach, cursor_from_offset, cursor_state,
    rim_offset, smart_cursor, usage_of,
};
use charon_game::defs::Definitions;
use charon_game::game::{BuildError, GameState};
use charon_game::items::ItemId;
use charon_game::loadout::{UseError, UseOutcome};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

fn arena() -> World {
    world_from_ascii(&box_art(64, 64), 1)
}

const CENTER: CellPos = CellPos { x: 20, y: 30 };

#[test]
fn use_button_moves_the_cursor_inside_the_reach_circle() {
    let r = REACH_CELLS;
    // 円の中は、そのまま。
    assert_eq!(clamp_to_reach((10, -5)), (10, -5));
    // 円の外は、同じ向きで縁へ。
    let (x, y) = clamp_to_reach((1000, 0));
    assert_eq!((x, y), (r, 0));
    let (x, y) = clamp_to_reach((-3000, 3000));
    let len = ((x * x + y * y) as f64).sqrt();
    assert!((len - f64::from(r)).abs() < 1.5, "縁: {len}");
    assert!(x < 0 && y > 0, "向きは保たれる");
    let free = cursor_from_offset(CENTER, (1000, 0), CursorUse::Free);
    assert_eq!(free.cell, CellPos::new(CENTER.x + r, CENTER.y));
}

#[test]
fn cursor_snaps_to_the_block_grid_for_block_items() {
    let free = cursor_from_offset(CENTER, (13, 7), CursorUse::Free);
    assert_eq!(free.cell, CellPos::new(33, 37), "武器・道具は揃えない");
    for usage in [CursorUse::Place, CursorUse::Mine] {
        let c = cursor_from_offset(CENTER, (13, 7), usage);
        assert_eq!(c.block, BlockPos { bx: 4, by: 4 });
        assert_eq!(c.cell, CellPos::new(4 * 8 + 4, 4 * 8 + 4), "ブロックの中心");
        // 同じブロックの中なら、どこを指しても同じブロック。
        assert_eq!(cursor_from_offset(CENTER, (12, 2), usage).block, c.block);
    }
}

#[test]
fn smart_cursor_jumps_to_the_rim_in_the_stick_direction() {
    let world = arena();
    assert_eq!(smart_cursor(&world, CENTER, (0, 0), CursorUse::Free), None);
    // ずれの大きさによらず、同じ点 (縁)。
    let small = smart_cursor(&world, CENTER, (3, 0), CursorUse::Free).unwrap();
    let large = smart_cursor(&world, CENTER, (900, 0), CursorUse::Free).unwrap();
    assert_eq!(small, large);
    assert_eq!(small.cell, CellPos::new(CENTER.x + REACH_CELLS, CENTER.y));
    // 向きを変えれば、縁の上を動く。
    let up = smart_cursor(&world, CENTER, (0, -5), CursorUse::Free).unwrap();
    assert_eq!(up.cell, CellPos::new(CENTER.x, CENTER.y - REACH_CELLS));
    let diag = rim_offset((7, 7)).unwrap();
    assert!(diag.0 > 30 && diag.0 == diag.1, "斜めの縁: {diag:?}");
}

#[test]
fn smart_cursor_targets_the_first_mineable_block_on_the_ray() {
    let mut world = arena();
    // 右へ: x = 40..47 (ブロック 5) と x = 48..55 (ブロック 6) が、y = 24..31 (ブロック行 3) にある。
    place_block(&mut world, 5, 3, '#');
    place_block(&mut world, 6, 3, '#');
    let mine = smart_cursor(&world, CENTER, (1, 0), CursorUse::Mine).unwrap();
    assert_eq!(mine.block, BlockPos { bx: 5, by: 3 }, "最初に当たるブロック");
    // 置くときは、その手前の空き。
    let put = smart_cursor(&world, CENTER, (1, 0), CursorUse::Place).unwrap();
    assert_eq!(put.block, BlockPos { bx: 4, by: 3 }, "手前の空きのブロック");
    // 届かない向き (左) は、円の縁。
    let none = smart_cursor(&world, CENTER, (-1, 0), CursorUse::Mine).unwrap();
    assert_eq!(none.cell.y.div_euclid(8), 3);
    assert_eq!(
        cursor_state(&world, CENTER, none, CursorUse::Mine),
        CursorState::Invalid,
        "壊すものがない"
    );
}

#[test]
fn unplaceable_or_out_of_reach_cursor_does_nothing() {
    let mut world = arena();
    place_block(&mut world, 5, 3, '#');
    let ok = Cursor::at(CellPos::new(34, 30));
    assert_eq!(cursor_state(&world, CENTER, ok, CursorUse::Place), CursorState::Ok);
    let taken = Cursor::at(CellPos::new(42, 26));
    assert_eq!(
        cursor_state(&world, CENTER, taken, CursorUse::Place),
        CursorState::Invalid,
        "すでにブロックがある"
    );
    assert_eq!(cursor_state(&world, CENTER, taken, CursorUse::Mine), CursorState::Ok);
    let far = Cursor::at(CellPos::new(CENTER.x + REACH_CELLS + 5, CENTER.y));
    assert_eq!(cursor_state(&world, CENTER, far, CursorUse::Place), CursorState::OutOfReach);
    assert_eq!(cursor_state(&world, CENTER, far, CursorUse::Free), CursorState::OutOfReach);
}

struct Stage {
    world: World,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    player: EntityId,
}

fn stage() -> Stage {
    let materials = Arc::new(MaterialTable::builtin().unwrap());
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let mut world = world_from_ascii(&box_art(64, 64), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    let mut entities = EntityWorld::new();
    let player = entities.spawn_player(10 * 16, 20 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    Stage { world, entities, game, defs, player }
}

fn center(s: &Stage) -> CellPos {
    charon_game::cursor::center_of(&s.entities, s.player).unwrap()
}

#[test]
fn use_selected_places_breaks_and_consumes() {
    let mut s = stage();
    let stone = material_of(&s.world, '#');
    let (p, defs) = (s.player, s.defs.clone());
    let block = ItemId::block(stone);
    let pick = defs.item_id("wood_pickaxe", s.world.materials()).unwrap();
    let potion = defs.item_id("heal_potion", s.world.materials()).unwrap();
    let inv = s.game.inventories.get_mut(&p).unwrap();
    inv.add_item(block, 2);
    inv.add_item(pick, 1);
    inv.add_item(potion, 2);
    assert!(inv.pin(0, block) && inv.pin(1, pick) && inv.pin(2, potion));
    assert_eq!(usage_of(Some(block), &defs), CursorUse::Place);
    assert_eq!(usage_of(Some(pick), &defs), CursorUse::Mine);
    assert_eq!(usage_of(Some(potion), &defs), CursorUse::Free);
    assert_eq!(usage_of(None, &defs), CursorUse::Free);

    let c = center(&s);
    let target = cursor_from_offset(c, (20, 12), CursorUse::Place);
    // ブロックを置く。
    s.game.inventories.get_mut(&p).unwrap().select(0);
    let out = s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target).unwrap();
    assert_eq!(out, UseOutcome::Placed);
    assert_eq!(s.game.inventories[&p].block_count(stone), 1);
    // 同じ場所には、もう置けない (何も変わらない)。
    let before = (s.world.state_hash(), s.game.inventories[&p].clone());
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target),
        Err(UseError::Invalid)
    );
    assert_eq!((s.world.state_hash(), s.game.inventories[&p].clone()), before);
    // 届かない場所には置けない。
    let far = Cursor::at(CellPos::new(c.x + REACH_CELLS + 20, c.y));
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, far),
        Err(UseError::OutOfReach)
    );
    // つるはしで壊す: ブロックのアイテムがもどる。
    s.game.inventories.get_mut(&p).unwrap().select(1);
    let mine = cursor_from_offset(c, (20, 12), CursorUse::Mine);
    let out = s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, mine).unwrap();
    assert_eq!(out, UseOutcome::Broken);
    assert_eq!(s.game.inventories[&p].block_count(stone), 2);
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, mine),
        Err(UseError::Invalid),
        "壊すものがない"
    );
    // 消耗品: 体力が回復して、1 個減る。
    s.game.vitals.get_mut(p).unwrap().damage(60);
    s.game.inventories.get_mut(&p).unwrap().select(2);
    let hp = s.game.vitals.get(p).unwrap().hp;
    let out = s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target).unwrap();
    assert_eq!(out, UseOutcome::Consumed);
    assert_eq!(s.game.vitals.get(p).unwrap().hp, (hp + 50).min(MAX_HP));
    assert_eq!(s.game.inventories[&p].item_count(potion), 1);
    // 何も選んでいなければ、何も起きない。
    s.game.inventories.get_mut(&p).unwrap().select(9);
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target),
        Ok(UseOutcome::Nothing)
    );
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, EntityId(99), &defs, target),
        Err(UseError::NoCharacter)
    );
    let _ = BuildError::NoBlock;
}

#[test]
fn placing_without_the_block_in_hand_fails_cleanly() {
    let mut s = stage();
    let stone = material_of(&s.world, '#');
    let (p, defs) = (s.player, s.defs.clone());
    let block = ItemId::block(stone);
    let inv = s.game.inventories.get_mut(&p).unwrap();
    inv.add_item(block, 1);
    inv.pin(0, block);
    inv.select(0);
    let c = center(&s);
    let target = cursor_from_offset(c, (20, 12), CursorUse::Place);
    s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target).unwrap();
    // 使い切ったら、ホットバーの枠は空と同じ (何も起きない)。
    let target2 = cursor_from_offset(c, (-20, 12), CursorUse::Place);
    assert_eq!(
        s.game.use_selected(&mut s.world, &mut s.entities, p, &defs, target2),
        Ok(UseOutcome::Nothing)
    );
}
