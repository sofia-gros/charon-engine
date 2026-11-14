//! 仕様 24 §6: 箱の個数、原子的な移動、退避・破損時の保全。
#![allow(clippy::unwrap_used)] // 場面の前提が崩れたら即時失敗させる。

use charon_core::coords::ChunkPos;
use charon_core::testkit::{box_art, place, place_block, world_from_ascii};
use charon_core::world::World;
use charon_game::blocks::BlockPos;
use charon_game::defs::Definitions;
use charon_game::fixtures::{FixtureError, FixtureMap};
use charon_game::items::{EquipSlot, ItemId};
use charon_game::{EntityId, EntityWorld};

const CHEST: BlockPos = BlockPos { bx: 8, by: 5 };
const BENCH: BlockPos = BlockPos { bx: 7, by: 5 };

fn setup(seed: u64) -> (World, EntityWorld, charon_game::game::GameState, Definitions, EntityId) {
    let mut world = world_from_ascii(&box_art(130, 64), seed);
    let defs = Definitions::engine_defaults(world.materials()).unwrap();
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(2 * 16, 2 * 16, &world).unwrap();
    let mut game = charon_game::game::GameState::from_definitions(&defs);
    game.add_character(
        id,
        charon_game::vitals::Vitals::new(charon_game::vitals::MAX_HP).unwrap(),
        defs.wand("basic").unwrap(),
    );
    game.give_inventory(id).unwrap();
    let chest = defs.item_id("storage_box", world.materials()).unwrap();
    let bench = defs.item_id("workbench", world.materials()).unwrap();
    let inv = game.inventories.get_mut(&id).unwrap();
    inv.add_item(chest, 1);
    inv.add_item(bench, 1);
    game.place_fixture(&mut world, &mut entities, id, CHEST, chest, &defs).unwrap();
    game.place_fixture(&mut world, &mut entities, id, BENCH, bench, &defs).unwrap();
    (world, entities, game, defs, id)
}

#[test]
fn chest_transfer_is_atomic_and_bounded() {
    for seed in 1..=8 {
        let (world, _entities, mut game, defs, id) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let stone = world.materials().id("stone").unwrap();
        let helmet = defs.item_id("wood_helmet", world.materials()).unwrap();
        let bar = defs.item_id("iron_bar", world.materials()).unwrap();
        let inv = game.inventories.get_mut(&id).unwrap();
        inv.add(wood, 25);
        inv.add_block(stone, 3);
        inv.add_item(bar, 2);
        inv.add_item(helmet, 1);
        inv.pin(0, ItemId::cells(wood));
        inv.select(0);
        game.equip(id, helmet, &defs).unwrap();
        let empty_hash = game.fixtures.state_hash();
        game.transfer_to_chest(&world, id, CHEST, ItemId::cells(wood), 25, &defs).unwrap();
        game.transfer_to_chest(&world, id, CHEST, ItemId::block(stone), 2, &defs).unwrap();
        game.transfer_to_chest(&world, id, CHEST, bar, 1, &defs).unwrap();
        assert_ne!(game.fixtures.state_hash(), empty_hash);
        assert_eq!(game.fixtures.get(CHEST).unwrap().contents.len(), 3);
        let inv = &game.inventories[&id];
        assert_eq!(inv.item_count(ItemId::cells(wood)), 0);
        assert_eq!(inv.hotbar(0), Some(ItemId::cells(wood)));
        assert_eq!(inv.hotbar_active(0), None);
        assert_eq!(inv.selected(), 0);
        assert_eq!(game.equipment[&id].get(EquipSlot::Head), Some(helmet));
        let before = game.state_hash();
        for amount in [0, 100_000] {
            assert_eq!(
                game.transfer_to_chest(&world, id, CHEST, bar, amount, &defs),
                Err(FixtureError::InvalidAmount)
            );
            assert_eq!(
                game.transfer_from_chest(&world, id, CHEST, bar, amount, &defs),
                Err(FixtureError::InvalidAmount)
            );
        }
        assert_eq!(
            game.transfer_to_chest(&world, id, CHEST, ItemId::block(stone), 2, &defs),
            Err(FixtureError::InsufficientItems)
        );
        assert_eq!(
            game.transfer_from_chest(&world, id, CHEST, bar, 2, &defs),
            Err(FixtureError::InsufficientItems)
        );
        assert_eq!(
            game.transfer_to_chest(&world, id, BENCH, bar, 1, &defs),
            Err(FixtureError::NotChest)
        );
        assert_eq!(
            game.transfer_to_chest(&world, id, CHEST, helmet, 1, &defs),
            Err(FixtureError::InsufficientItems),
            "装備中のものは持ち物ではない seed={seed}"
        );
        assert_eq!(
            game.transfer_from_chest(&world, id, CHEST, ItemId::def(u16::MAX), 1, &defs),
            Err(FixtureError::UnknownItem(ItemId::def(u16::MAX)))
        );
        assert_eq!(game.state_hash(), before, "失敗は持ち物・印・装備・箱を変えない seed={seed}");

        game.transfer_from_chest(&world, id, CHEST, ItemId::cells(wood), 25, &defs).unwrap();
        assert_eq!(game.inventories[&id].hotbar_active(0), Some(ItemId::cells(wood)));
        assert_eq!(game.inventories[&id].count(wood), 25);
        assert_eq!(game.fixtures.get(CHEST).unwrap().contents.len(), 2);
        let inv = game.inventories.get_mut(&id).unwrap();
        inv.add_item(bar, charon_game::inventory::STACK_MAX - 1);
        let before = game.state_hash();
        assert_eq!(
            game.transfer_from_chest(&world, id, CHEST, bar, 1, &defs),
            Err(FixtureError::NoRoom)
        );
        assert_eq!(game.state_hash(), before);

        // 未知の 40 種類も枠を占める。既存の種類の個数上限は別に守る。
        let full = (0..40).map(|n| format!("\"missing:{n}\":1")).collect::<Vec<_>>().join(",");
        let text = format!(
            "[(bx:8,by:5,fixture:(item_name:\"storage_box\",material_name:\"wood\",contents:{{{full}}}))]"
        );
        game.fixtures = FixtureMap::from_ron(&text, &defs, &world).unwrap();
        let before = game.state_hash();
        assert_eq!(
            game.transfer_to_chest(&world, id, CHEST, ItemId::cells(wood), 1, &defs),
            Err(FixtureError::NoRoom)
        );
        assert_eq!(game.state_hash(), before);
        let text = "[(bx:8,by:5,fixture:(item_name:\"storage_box\",material_name:\"wood\",contents:{\"cells:wood\":99999}))]";
        game.fixtures = FixtureMap::from_ron(text, &defs, &world).unwrap();
        let before = game.state_hash();
        assert_eq!(
            game.transfer_to_chest(&world, id, CHEST, ItemId::cells(wood), 1, &defs),
            Err(FixtureError::NoRoom)
        );
        assert_eq!(game.state_hash(), before);
    }
}

#[test]
fn chest_contents_survive_reopen_and_anchor_damage() {
    for seed in 1..=8 {
        let (mut world, _entities, mut game, defs, id) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        game.inventories.get_mut(&id).unwrap().add(wood, 5);
        game.transfer_to_chest(&world, id, CHEST, ItemId::cells(wood), 3, &defs).unwrap();
        let mut text = game.fixtures.to_ron().unwrap();
        text = text.replace("\"cells:wood\":3", "\"cells:wood\":3,\"missing:kept\":7");
        game.fixtures = FixtureMap::from_ron(&text, &defs, &world).unwrap();
        let saved = game.fixtures.state_hash();
        let root =
            std::env::temp_dir().join(format!("charon-chests-{}-{seed}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        game.fixtures.write(&root).unwrap();
        let reloaded = FixtureMap::read(&root, &defs, &world).unwrap();
        assert_eq!(reloaded.state_hash(), saved);
        assert_eq!(reloaded.get(CHEST).unwrap().contents["missing:kept"], 7);
        game.fixtures = reloaded;
        let chunk = world.take_chunk(ChunkPos::new(1, 0)).unwrap();
        let before = game.state_hash();
        assert_eq!(
            game.transfer_from_chest(&world, id, CHEST, ItemId::cells(wood), 1, &defs),
            Err(FixtureError::Inactive)
        );
        assert_eq!(game.state_hash(), before);
        world.insert_loaded_chunk(ChunkPos::new(1, 0), chunk);
        assert!(game.fixtures.active(&world, CHEST));
        place(&mut world, 64, 40, '.');
        let before = game.state_hash();
        assert_eq!(
            game.transfer_to_chest(&world, id, CHEST, ItemId::cells(wood), 1, &defs),
            Err(FixtureError::Inactive)
        );
        assert_eq!(game.state_hash(), before);
        assert_eq!(game.fixtures.state_hash(), saved);
        place_block(&mut world, CHEST.bx, CHEST.by, 'W');
        assert!(game.fixtures.active(&world, CHEST));
        game.transfer_from_chest(&world, id, CHEST, ItemId::cells(wood), 2, &defs).unwrap();
        assert_eq!(game.fixtures.get(CHEST).unwrap().contents["cells:wood"], 1);
        assert_eq!(game.fixtures.get(CHEST).unwrap().contents["missing:kept"], 7);
        assert_eq!(game.remove_fixture(&mut world, id, CHEST, &defs), Err(FixtureError::NotEmpty));
        assert!(game.fixtures.to_ron().unwrap().contains("missing:kept"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
