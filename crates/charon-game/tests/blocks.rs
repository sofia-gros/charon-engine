//! 仕様 17 §3・§4: ブロックを置く (押し出し)・壊す。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use charon_core::cell::{Cell, FLAG_BLOCK, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{
    box_art, count, count_block_cells, fill, material_of, place, place_block, world_from_ascii,
};
use charon_core::world::World;
use charon_game::blocks::{BlockPos, BreakError, PlaceError, break_block, place_block as put};
use charon_game::{EntityWorld, PlayerInput};

/// 内側 64×64 の、上が開いた箱。ブロック (2, 5) は x 16..=23、y 40..=47 (空中)。
fn arena() -> World {
    world_from_ascii(&box_art(64, 64), 1)
}

const AT: BlockPos = BlockPos { bx: 2, by: 5 };

fn stone(world: &World) -> MaterialId {
    material_of(world, '#')
}

fn row(world: &World, y: i32, x0: i32, x1: i32) -> String {
    (x0..=x1)
        .map(|x| {
            let cell = world.get(CellPos::new(x, y));
            if cell.has(FLAG_BLOCK) {
                'B'
            } else {
                match cell.material {
                    m if m == MaterialId::EMPTY => '.',
                    m if m == material_of(world, 's') => 's',
                    m if m == material_of(world, 'w') => 'w',
                    m if m == material_of(world, '#') => '#',
                    _ => '?',
                }
            }
        })
        .collect()
}

/// 空きに置くと、64 セルが旗つきで並び、ほかは変わらない。
#[test]
fn place_block_in_empty_space() {
    let mut world = arena();
    let mut entities = EntityWorld::new();
    let material = stone(&world);
    let before_stone = count(&world, '#');
    let placed = put(&mut world, &mut entities, AT, material).unwrap();
    assert_eq!((placed.pushed_cells, placed.pushed_entities), (0, 0));
    assert_eq!(count_block_cells(&world, 16, 40, 23, 47), 64);
    assert_eq!(count_block_cells(&world, 0, 0, 70, 70), 64, "ほかには旗がない");
    assert_eq!(count(&world, '#'), before_stone + 64, "増えたのは 64 セルだけ");
    assert_eq!(world.get(CellPos::new(16, 40)).material, material);
}

/// 砂場に置くと、左半分のセルは左へ、右半分は右へ、順番を保って詰まる。砂場全体が外へずれる。
#[test]
fn place_block_pushes_a_sand_field_outward() {
    let mut world = arena();
    fill(&mut world, 5, 40, 45, 63, 's');
    let mut entities = EntityWorld::new();
    let sand = count(&world, 's');
    let material = stone(&world);
    let placed = put(&mut world, &mut entities, AT, material).unwrap();
    assert_eq!(placed.pushed_cells, 8 * 8);
    for y in 40..=47 {
        // 左は、左の壁 (x = 0) の内側 (x = 1) まで詰まる。右は、砂場の端 (45) を越えて 49 まで伸びる。
        assert_eq!(row(&world, y, 1, 15), "s".repeat(15), "行 {y} の左");
        assert_eq!(row(&world, y, 16, 23), "BBBBBBBB", "行 {y} のブロック");
        assert_eq!(row(&world, y, 24, 50), format!("{}.", "s".repeat(26)), "行 {y} の右");
    }
    // ブロックの外の行 (39 以下・48 以上) は動かない。
    assert_eq!(row(&world, 48, 4, 6), ".ss");
    assert_eq!(count(&world, 's'), sand, "砂の数は変わらない");
}

/// 置く前の空でないセルは、材質ごとの数が、すべて残る (砂・水・石)。増えるのは、ブロックの 64 セルだけ。
#[test]
fn place_block_conserves_cells() {
    let mut world = arena();
    fill(&mut world, 4, 36, 30, 52, 's');
    fill(&mut world, 31, 36, 40, 52, 'w');
    fill(&mut world, 3, 33, 40, 35, 'w');
    let mut entities = EntityWorld::new();
    let before = (count(&world, 's'), count(&world, 'w'), count(&world, '#'));
    let material = stone(&world);
    put(&mut world, &mut entities, AT, material).unwrap();
    let after = (count(&world, 's'), count(&world, 'w'), count(&world, '#'));
    assert_eq!(after, (before.0, before.1, before.2 + 64));
}

/// 左右が壁で塞がっていれば、上下へ: 上半分のセルは上へ、下半分は下へ。
#[test]
fn place_block_falls_back_to_vertical() {
    let mut world = arena();
    fill(&mut world, 15, 40, 15, 47, '#'); // 左の壁
    fill(&mut world, 24, 40, 24, 47, '#'); // 右の壁
    fill(&mut world, 16, 40, 23, 47, 's'); // 筒の中は砂
    let mut entities = EntityWorld::new();
    let material = stone(&world);
    put(&mut world, &mut entities, AT, material).unwrap();
    for x in 16..=23 {
        for y in 36..=39 {
            assert_eq!(
                world.get(CellPos::new(x, y)).material,
                material_of(&world, 's'),
                "上 ({x}, {y})"
            );
        }
        for y in 48..=51 {
            assert_eq!(
                world.get(CellPos::new(x, y)).material,
                material_of(&world, 's'),
                "下 ({x}, {y})"
            );
        }
    }
    assert_eq!(count(&world, 's'), 64, "砂は 64 セルのまま");
    assert_eq!(count_block_cells(&world, 16, 40, 23, 47), 64);
}

/// 左右も上下も塞がっていれば、上下左右でいちばん近い空きへ。同じ距離なら、左・右・上・下の順。
#[test]
fn place_block_uses_the_nearest_free_cell_as_a_last_resort() {
    let mut world = arena();
    fill(&mut world, 15, 40, 15, 47, '#'); // 左の壁 (その行は左へ押せない)
    fill(&mut world, 16, 39, 23, 39, '#'); // 上の壁 (列は上へ押せない)
    place(&mut world, 16, 40, 's'); // 左上の角の砂 1 つ
    let mut entities = EntityWorld::new();
    let material = stone(&world);
    put(&mut world, &mut entities, AT, material).unwrap();
    assert_eq!(count(&world, 's'), 1);
    // 左は壁、上は壁。右へブロックの中を通り抜けて、右の外の最初の空き (24, 40) と、下の外の最初の空き
    // (16, 48) が、同じ距離 (8)。左・右・上・下の順なので、右。
    assert_eq!(world.get(CellPos::new(24, 40)).material, material_of(&world, 's'));
}

/// 上下左右がすべて塞がっていて、行き先がなければ、何も変えずに失敗する。
#[test]
fn place_block_without_any_room_fails_and_changes_nothing() {
    let mut world = arena();
    fill(&mut world, 15, 40, 15, 47, '#');
    fill(&mut world, 24, 40, 24, 47, '#');
    fill(&mut world, 16, 39, 23, 39, '#');
    fill(&mut world, 16, 48, 23, 48, '#');
    place(&mut world, 18, 42, 's');
    let mut entities = EntityWorld::new();
    let (hw, he) = (world.state_hash(), entities.state_hash());
    let material = stone(&world);
    assert_eq!(put(&mut world, &mut entities, AT, material), Err(PlaceError::NoRoom));
    assert_eq!((world.state_hash(), entities.state_hash()), (hw, he), "何も変わらない");
}

/// 固体・ほかのブロック・読み込まれていない場所・不正な材質では、何も変えずに失敗する。
#[test]
fn place_block_refuses_solid_or_unloaded_or_invalid() {
    let mut world = arena();
    let mut entities = EntityWorld::new();
    let material = stone(&world);
    let water = material_of(&world, 'w');
    let hash = world.state_hash();
    assert_eq!(put(&mut world, &mut entities, AT, water), Err(PlaceError::InvalidMaterial));
    assert_eq!(
        put(&mut world, &mut entities, AT, MaterialId::EMPTY),
        Err(PlaceError::InvalidMaterial)
    );
    assert_eq!(
        put(&mut world, &mut entities, AT, MaterialId::BOUNDARY),
        Err(PlaceError::InvalidMaterial)
    );
    assert_eq!(
        put(&mut world, &mut entities, BlockPos { bx: 5000, by: 5 }, material),
        Err(PlaceError::Unloaded)
    );
    assert_eq!(
        put(&mut world, &mut entities, BlockPos { bx: i32::MAX, by: 0 }, material),
        Err(PlaceError::Unloaded)
    );
    // 固体の地形が 1 セルでもあれば置けない。
    place(&mut world, 20, 44, '#');
    assert_eq!(put(&mut world, &mut entities, AT, material), Err(PlaceError::Occupied));
    place(&mut world, 20, 44, '.');
    // ほかのブロックがあれば置けない (同じ場所にも、重なる場所にも、格子に揃っているので、同じ場所だけ)。
    place_block(&mut world, 2, 5, '#');
    assert_eq!(put(&mut world, &mut entities, AT, material), Err(PlaceError::Occupied));
    // 失敗では何も変わっていない (最初の置き直しの分だけ)。
    let after = world.state_hash();
    assert_ne!(after, hash);
    assert_eq!(put(&mut world, &mut entities, AT, material), Err(PlaceError::Occupied));
    assert_eq!(world.state_hash(), after);
}

/// 敵・プレイヤーは、左右の近い方のブロックの外へ飛ばされる。
#[test]
fn place_block_pushes_entities_sideways() {
    for (x, expect_left) in [(13, true), (21, false)] {
        let mut world = arena();
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(i64::from(x) * 16, 38 * 16, &world).unwrap();
        let before = entities.get(id).unwrap().clone();
        let material = stone(&world);
        let placed = put(&mut world, &mut entities, AT, material).unwrap();
        assert_eq!(placed.pushed_entities, 1);
        let after = entities.get(id).unwrap();
        assert_eq!(after.y16, before.y16, "横へ飛ぶだけ");
        if expect_left {
            assert!(after.x16 < before.x16, "左へ");
            assert!(after.x16 + 80 <= 16 * 16 + 40, "左へ出た: x16 = {}", after.x16);
        } else {
            assert!(after.x16 > before.x16, "右へ");
        }
        // 出たあとは、ブロックに重なっていない (固体の中にいない)。
        let mut check = EntityWorld::new();
        assert!(
            check.spawn_checked(after.x16, after.y16, after.width, after.height, &world).is_ok()
        );
    }
}

/// 動かせないエンティティがいれば、何も変えずに失敗する。
#[test]
fn place_block_fails_when_an_entity_cannot_move() {
    let mut world = arena();
    // 閉じた部屋 (内側 x 14..=25、y 38..=47)。中の高さ 10・幅 5 のプレイヤーは、ブロックの外へ出られない。
    fill(&mut world, 10, 34, 30, 52, '#');
    fill(&mut world, 14, 38, 25, 47, '.');
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(14 * 16, 38 * 16, &world).unwrap();
    let (hw, he) = (world.state_hash(), entities.state_hash());
    let material = stone(&world);
    assert_eq!(put(&mut world, &mut entities, AT, material), Err(PlaceError::EntityStuck(id)));
    assert_eq!((world.state_hash(), entities.state_hash()), (hw, he), "何も変わらない");
}

/// 砂のブロックの上に、エンティティが立てる (ブロックは、粉でも固体)。
#[test]
fn entities_stand_on_block_cells() {
    let mut world = arena();
    place_block(&mut world, 2, 5, 's'); // 砂のブロック (旗つき)。
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(17 * 16, 20 * 16, &world).unwrap();
    for _ in 0..120 {
        entities.step_players(&[(id, PlayerInput::default())], &world).unwrap();
        world.step();
    }
    let e = entities.get(id).unwrap();
    assert!(e.grounded, "ブロックの上に立つ");
    assert_eq!((e.y16 + 160).div_euclid(16), 40, "足がブロックの上端 (y = 40) に着く");
}

/// 失敗したら状態のハッシュが変わらない。同じ入力なら、同じ結果。
#[test]
fn place_block_is_all_or_nothing_and_deterministic() {
    let run = || {
        let mut world = arena();
        fill(&mut world, 5, 40, 45, 63, 's');
        fill(&mut world, 3, 36, 12, 39, 'w');
        let mut entities = EntityWorld::new();
        entities.spawn_player(13 * 16, 38 * 16, &world).unwrap();
        let material = stone(&world);
        put(&mut world, &mut entities, AT, material).unwrap();
        (world.state_hash(), entities.state_hash())
    };
    assert_eq!(run(), run());
}

/// 完全なブロックは壊せて、材質を返す。欠けたブロックや、何もない場所は壊せない。
#[test]
fn break_block_returns_the_material_or_refuses() {
    let mut world = arena();
    let mut entities = EntityWorld::new();
    let material = stone(&world);
    put(&mut world, &mut entities, AT, material).unwrap();
    assert_eq!(break_block(&mut world, AT), Ok(material));
    assert_eq!(count_block_cells(&world, 0, 0, 70, 70), 0);
    assert_eq!(world.get(CellPos::new(20, 44)), Cell::default(), "空になる");
    assert_eq!(break_block(&mut world, AT), Err(BreakError::NotABlock), "もう何もない");
    // 欠けたブロックは、普通のセル。
    put(&mut world, &mut entities, AT, material).unwrap();
    world.set(CellPos::new(20, 44), Cell::default());
    for _ in 0..30 {
        world.step();
    }
    assert_eq!(break_block(&mut world, AT), Err(BreakError::NotABlock));
    assert_eq!(break_block(&mut world, BlockPos { bx: 9999, by: 0 }), Err(BreakError::Unloaded));
}

mod game_api {
    use charon_core::coords::CellPos;
    use charon_core::material::MaterialTable;
    use charon_core::testkit::{box_art, count, count_block_cells, fill, world_from_ascii};
    use charon_game::blocks::{BlockPos, BreakError, PlaceError};
    use charon_game::defs::Definitions;
    use charon_game::game::{BuildError, GameEvent, GameState};
    use charon_game::vitals::{MAX_HP, Vitals};
    use charon_game::{EntityId, EntityWorld};

    const AT: BlockPos = BlockPos { bx: 2, by: 5 };

    struct Rig {
        world: charon_core::world::World,
        entities: EntityWorld,
        game: GameState,
        player: EntityId,
    }

    fn rig() -> Rig {
        let world = world_from_ascii(&box_art(64, 64), 1);
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(40 * 16, 20 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        Rig { world, entities, game, player }
    }

    /// 粉 64 個 → ブロック 1 個 → 置く → 壊す → ブロック 1 個にもどる。出来事が出る。
    #[test]
    fn craft_place_and_break_roundtrip_through_the_inventory() {
        let mut r = rig();
        let sand = r.world.materials().id("sand").unwrap();
        let water = r.world.materials().id("water").unwrap();
        r.game.inventories.get_mut(&r.player).unwrap().add(sand, 130);
        assert!(!r.game.craft_block(&r.world, r.player, water), "液体はブロックにできない");
        assert!(r.game.craft_block(&r.world, r.player, sand));
        assert!(r.game.craft_block(&r.world, r.player, sand));
        assert!(!r.game.craft_block(&r.world, r.player, sand), "2 個分 (128) で、残りは 2");
        let inv = |r: &Rig| {
            let i = &r.game.inventories[&r.player];
            (i.count(sand), i.block_count(sand))
        };
        assert_eq!(inv(&r), (2, 2));

        let placed =
            r.game.place_block_item(&mut r.world, &mut r.entities, r.player, AT, sand).unwrap();
        assert_eq!(placed.pushed_cells, 0);
        assert_eq!(inv(&r), (2, 1), "ブロックを 1 個使った");
        assert_eq!(count_block_cells(&r.world, 16, 40, 23, 47), 64);
        // もう一度、同じ場所には置けない。持ち物は減らない。
        assert_eq!(
            r.game.place_block_item(&mut r.world, &mut r.entities, r.player, AT, sand),
            Err(BuildError::Place(PlaceError::Occupied))
        );
        assert_eq!(inv(&r), (2, 1));
        // 壊すと、ブロックが 1 個もどる。
        assert_eq!(r.game.break_block_item(&mut r.world, r.player, AT), Ok(sand));
        assert_eq!(inv(&r), (2, 2));
        assert_eq!(
            r.world.get(CellPos::new(20, 44)).material,
            charon_core::cell::MaterialId::EMPTY
        );
        assert_eq!(
            r.game.break_block_item(&mut r.world, r.player, AT),
            Err(BuildError::Break(BreakError::NotABlock))
        );
        let events = r.game.take_events();
        assert!(events.contains(&GameEvent::BlockPlaced(AT, sand)));
        assert!(events.contains(&GameEvent::BlockBroken(AT, sand)));
    }

    /// ブロックを持っていなければ置けない。持ち物のない者も置けない。砂場に置いても、砂のセルは持ち物に入らない。
    #[test]
    fn placing_needs_a_block_item_and_pushing_keeps_the_cells_in_the_world() {
        let mut r = rig();
        let stone = r.world.materials().id("stone").unwrap();
        assert_eq!(
            r.game.place_block_item(&mut r.world, &mut r.entities, r.player, AT, stone),
            Err(BuildError::NoBlock)
        );
        r.game.inventories.get_mut(&r.player).unwrap().add_block(stone, 1);
        fill(&mut r.world, 5, 40, 45, 63, 's');
        let sand = count(&r.world, 's');
        let hash = r.game.inventories[&r.player].state_hash();
        r.game.place_block_item(&mut r.world, &mut r.entities, r.player, AT, stone).unwrap();
        assert_eq!(count(&r.world, 's'), sand, "押し出された砂は世界に残る");
        assert_ne!(r.game.inventories[&r.player].state_hash(), hash);
        // 持ち物のないエンティティ (id 99) は置けない。
        assert_eq!(
            r.game.place_block_item(&mut r.world, &mut r.entities, EntityId(99), AT, stone),
            Err(BuildError::NoBlock)
        );
    }
}
