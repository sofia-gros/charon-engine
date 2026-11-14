//! 仕様 24 §4: 設備の原子的配置、チャンク退避と名前保存、傷んだアンカー。
#![allow(clippy::unwrap_used)] // 場面の前提が崩れたら即時失敗させる。

use std::sync::Arc;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::testkit::{
    assert_sleeps, box_art, count_block_cells, place, place_block, world_from_ascii,
};
use charon_core::world::World;
use charon_game::blocks::BlockPos;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::fixtures::{FixtureError, FixtureMap, FixtureSaveError};
use charon_game::game::{BuildError, GameState};
use charon_game::items::{ItemId, ItemKind};
use charon_game::loadout::ItemError;
use charon_game::{EntityId, EntityWorld};
use charon_world::generator::FlatGenerator;
use charon_world::save::WorldMeta;
use charon_world::streaming::{LOAD_DELAY, MemoryStore, Streamer};

const LEFT: BlockPos = BlockPos { bx: 7, by: 5 };
const RIGHT: BlockPos = BlockPos { bx: 8, by: 5 };

fn setup(seed: u64) -> (World, EntityWorld, GameState, Definitions, EntityId, ItemId) {
    let world = world_from_ascii(&box_art(130, 64), seed);
    let defs = Definitions::engine_defaults(world.materials()).unwrap();
    let workbench = defs.item_id("workbench", world.materials()).unwrap();
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(2 * 16, 2 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(
        id,
        charon_game::vitals::Vitals::new(charon_game::vitals::MAX_HP).unwrap(),
        defs.wand("basic").unwrap(),
    );
    game.give_inventory(id).unwrap();
    game.inventories.get_mut(&id).unwrap().add_item(workbench, 4);
    (world, entities, game, defs, id, workbench)
}

#[test]
fn fixtures_place_only_on_complete_blocks() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut game, defs, id, item) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let stone = world.materials().id("stone").unwrap();
        assert_eq!(defs.fixture_for_item(item).unwrap().material, wood);
        assert_eq!(defs.fixture("furnace").unwrap().material, stone);
        let before = game.inventories[&id].item_count(item);
        game.place_fixture(&mut world, &mut entities, id, LEFT, item, &defs).unwrap();
        assert!(game.fixtures.active(&world, LEFT));
        assert_eq!(count_block_cells(&world, 56, 40, 63, 47), 64);
        assert_eq!(game.inventories[&id].item_count(item), before - 1);
        let world_hash = world.state_hash();
        let game_hash = game.state_hash();
        assert_eq!(
            game.place_fixture(&mut world, &mut entities, id, LEFT, item, &defs),
            Err(FixtureError::Occupied)
        );
        assert_eq!(
            game.place_block_item(&mut world, &mut entities, id, LEFT, wood),
            Err(BuildError::FixtureAnchor)
        );
        assert_eq!(world.state_hash(), world_hash);
        assert_eq!(game.state_hash(), game_hash);
        let wrong = BlockPos { bx: 2, by: 8 }; // 箱の石の床と重なる。
        assert!(game.place_fixture(&mut world, &mut entities, id, wrong, item, &defs).is_err());
        assert_eq!(game.inventories[&id].item_count(item), before - 1);
        let other = defs.item_id("storage_box", world.materials()).unwrap();
        assert_eq!(
            game.place_fixture(&mut world, &mut entities, id, RIGHT, other, &defs),
            Err(FixtureError::NotOwned)
        );
        let fill = charon_game::inventory::STACK_MAX - (before - 1);
        game.inventories.get_mut(&id).unwrap().add_item(item, fill);
        let full_hash = game.state_hash();
        assert_eq!(game.remove_fixture(&mut world, id, LEFT, &defs), Err(FixtureError::NoRoom));
        assert_eq!(game.state_hash(), full_hash);
        assert_eq!(world.state_hash(), world_hash);
        game.inventories.get_mut(&id).unwrap().take_item(item, fill);
        game.remove_fixture(&mut world, id, LEFT, &defs).unwrap();
        assert_eq!(game.inventories[&id].item_count(item), before);
        assert!(game.fixtures.get(LEFT).is_none());
        assert_eq!(count_block_cells(&world, 56, 40, 63, 47), 0);
        // 固定ブロックは余計に書き換えられず、最終的に休眠する。
        game.place_fixture(&mut world, &mut entities, id, LEFT, item, &defs).unwrap();
        assert_sleeps(&mut world, 100);
        assert!(game.fixtures.active(&world, LEFT));
    }
}

#[test]
fn fixtures_survive_streaming_and_save() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut game, defs, id, workbench) = setup(seed);
        let chest = defs.item_id("storage_box", world.materials()).unwrap();
        game.inventories.get_mut(&id).unwrap().add_item(chest, 1);
        game.place_fixture(&mut world, &mut entities, id, LEFT, workbench, &defs).unwrap();
        game.place_fixture(&mut world, &mut entities, id, RIGHT, chest, &defs).unwrap();
        let initial_hash = game.fixtures.state_hash();
        let root =
            std::env::temp_dir().join(format!("charon-fixtures-{}-{seed}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        game.fixtures.write(&root).unwrap();
        // 書き直しも一時ファイル経由で置換する。
        game.fixtures.write(&root).unwrap();
        let stored = FixtureMap::read(&root, &defs, &world).unwrap();
        assert_eq!(stored.state_hash(), initial_hash);
        let materials = Arc::clone(world.materials());
        let meta = WorldMeta::new(seed, &materials);
        let store = Arc::new(MemoryStore::new());
        let generator = Arc::new(FlatGenerator { stone: materials.id("stone").unwrap() });
        let streamer = Streamer::with_memory_store(
            Arc::clone(&store),
            Arc::clone(&materials),
            seed,
            generator.clone(),
            0,
            meta.materials.clone(),
        )
        .unwrap();
        streamer.save_loaded(&world).unwrap();
        streamer.finish().unwrap();
        world.take_chunk(ChunkPos::new(1, 0));
        assert!(!game.fixtures.active(&world, RIGHT));
        assert!(game.fixtures.active(&world, LEFT));
        let mut loader = Streamer::with_memory_store(
            Arc::clone(&store),
            Arc::clone(&materials),
            seed,
            generator,
            0,
            meta.materials,
        )
        .unwrap();
        for tick in 0..=LOAD_DELAY {
            world.restore_tick(tick);
            loader.tick(&mut world, &[ChunkPos::new(1, 0)]).unwrap();
        }
        assert!(game.fixtures.active(&world, RIGHT));
        assert_eq!(game.fixtures.state_hash(), initial_hash);
        loader.finish().unwrap();
        let reloaded = FixtureMap::read(&root, &defs, &world).unwrap();
        assert!(reloaded.active(&world, RIGHT));
        assert_eq!(reloaded.state_hash(), initial_hash);
        let bytes = reloaded.to_ron().unwrap().into_bytes();
        let snapshot = charon_world::web_save::WebSnapshot {
            metadata: WorldMeta::new(seed, &materials),
            chunks: store.snapshot().unwrap(),
            background: Vec::new(),
            bodies: Vec::new(),
            fixtures: bytes.clone(),
        };
        let decoded =
            charon_world::web_save::WebSnapshot::decode(&snapshot.encode().unwrap()).unwrap();
        assert_eq!(decoded.fixtures, bytes);
        assert_eq!(
            FixtureMap::from_ron(std::str::from_utf8(&decoded.fixtures).unwrap(), &defs, &world)
                .unwrap(),
            reloaded
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn fixture_anchor_damage_keeps_metadata() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut game, defs, id, workbench) = setup(seed);
        game.place_fixture(&mut world, &mut entities, id, LEFT, workbench, &defs).unwrap();
        let occupied = r#"[(bx:7,by:5,fixture:(item_name:"workbench",material_name:"wood",contents:{"missing:kept":1}))]"#;
        let contents = FixtureMap::from_ron(occupied, &defs, &world).unwrap();
        game.fixtures = contents;
        assert_eq!(game.remove_fixture(&mut world, id, LEFT, &defs), Err(FixtureError::NotEmpty));
        let saved = game.fixtures.to_ron().unwrap();
        assert!(saved.contains("missing:kept"), "未知の中身も保存・再保存で保全");
        let previous = game.fixtures.state_hash();
        let wood = world.materials().id("wood").unwrap();
        place(&mut world, 56, 40, '.');
        assert!(!game.fixtures.active(&world, LEFT));
        assert_eq!(game.break_block_item(&mut world, id, LEFT), Err(BuildError::FixtureAnchor));
        assert_eq!(game.remove_fixture(&mut world, id, LEFT, &defs), Err(FixtureError::Inactive));
        assert_eq!(game.fixtures.state_hash(), previous);
        assert_eq!(game.fixtures.to_ron().unwrap(), saved);
        // 修復しても metadata は保持し、同じ材質の完全ブロックなら再び使える。
        place_block(&mut world, LEFT.bx, LEFT.by, 'W');
        assert!(game.fixtures.active(&world, LEFT));
        let wrong =
            r#"[(bx:7,by:5,fixture:(item_name:"missing",material_name:"wood",contents:{}))]"#;
        assert!(matches!(
            FixtureMap::from_ron(wrong, &defs, &world),
            Err(FixtureSaveError::UnknownFixture(_))
        ));
        let wrong_material =
            r#"[(bx:7,by:5,fixture:(item_name:"workbench",material_name:"missing",contents:{}))]"#;
        assert!(matches!(
            FixtureMap::from_ron(wrong_material, &defs, &world),
            Err(FixtureSaveError::UnknownMaterial(_))
        ));
        assert!(matches!(
            FixtureMap::from_ron("broken", &defs, &world),
            Err(FixtureSaveError::Parse(_))
        ));
        // MOD の設備も名前空間を通して解決し、item の共有は拒否する。
        let mut custom = defs.clone();
        custom
            .layer(
                DefinitionFiles {
                    items: Some(r#"[(name:"demo_bench",kind:Misc)]"#),
                    fixtures: Some(
                        r#"[(name:"bench_fixture",item:"demo_bench",material:"wood",kind:Station)]"#,
                    ),
                    ..Default::default()
                },
                world.materials(),
                Some("custom"),
            )
            .unwrap();
        assert_eq!(
            custom.fixture("custom:bench_fixture").unwrap().item,
            custom.item_id("custom:demo_bench", world.materials()).unwrap()
        );
        custom.layer(
            DefinitionFiles {
                recipes: Some(r#"[(name:"custom_recipe",inputs:[("cells:wood",1)],output:("wood_sword",1),station:Some("demo_bench"))]"#),
                ..Default::default()
            },
            world.materials(), Some("custom"),
        ).unwrap();
        let before = custom.content_hash();
        assert!(custom.layer(
            DefinitionFiles {
                fixtures: Some(r#"[(name:"custom:bench_fixture",replace:true,item:"demo_bench",material:"wood",kind:Chest)]"#),
                ..Default::default()
            },
            world.materials(), Some("custom"),
        ).is_err(), "Station を Chest に上書きして既存レシピを壊せない");
        assert_eq!(custom.content_hash(), before);
        assert!(
            custom
                .layer(
                    DefinitionFiles {
                        fixtures: Some(
                            r#"[(name:"another",item:"demo_bench",material:"wood",kind:Chest)]"#
                        ),
                        ..Default::default()
                    },
                    world.materials(),
                    Some("custom")
                )
                .is_err()
        );
        assert_eq!(custom.content_hash(), before);
        assert_eq!(world.materials().id("wood"), Some(wood));
    }
}

#[test]
fn craft_requires_the_correct_fixture_and_is_atomic() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut game, defs, id, bench) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let stone = world.materials().id("stone").unwrap();
        let furnace = defs.item_id("furnace", world.materials()).unwrap();
        let center = CellPos::new(64, 40);
        let inv = game.inventories.get_mut(&id).unwrap();
        inv.add(wood, 80);
        inv.add(stone, 40);
        let before = game.state_hash();
        assert_eq!(game.craft(id, "furnace", &[], &defs), Err(ItemError::MissingStation));
        assert_eq!(game.state_hash(), before, "設備なしでは材料も減らない seed={seed}");
        // 普通の石ブロックや箱は Station にならない。
        place_block(&mut world, 6, 5, '#');
        assert!(game.fixtures.nearby_stations(&world, center, &defs).is_empty());
        assert_eq!(game.craft(id, "furnace", &[], &defs), Err(ItemError::MissingStation));
        game.place_fixture(&mut world, &mut entities, id, LEFT, bench, &defs).unwrap();
        let nearby = game.fixtures.nearby_stations(&world, center, &defs);
        assert_eq!(nearby, vec![bench], "チャンク境界の Station を見つける seed={seed}");
        game.craft(id, "furnace", &nearby, &defs).unwrap();
        assert_eq!(game.inventories[&id].item_count(furnace), 1);
        let previous = game.state_hash();
        assert_eq!(game.craft(id, "furnace", &nearby, &defs), Err(ItemError::MissingIngredients));
        assert_eq!(game.state_hash(), previous);
        let inv = game.inventories.get_mut(&id).unwrap();
        inv.add(wood, 8);
        inv.add(stone, 40);
        inv.add_item(furnace, charon_game::inventory::STACK_MAX - 1);
        let previous = game.state_hash();
        assert_eq!(game.craft(id, "furnace", &nearby, &defs), Err(ItemError::NoRoom));
        assert_eq!(game.state_hash(), previous, "満杯の失敗も原子的 seed={seed}");
        // アンカーの 1 セルが壊れれば Station は見えない。metadata は保つ。
        let saved = game.fixtures.state_hash();
        place(&mut world, 56, 40, '.');
        assert!(game.fixtures.nearby_stations(&world, center, &defs).is_empty());
        assert_eq!(game.fixtures.state_hash(), saved);
        assert_eq!(game.craft(id, "furnace", &[], &defs), Err(ItemError::MissingStation));
    }
}

#[test]
fn ore_processing_builds_an_iron_tool() {
    for seed in 1..=8 {
        let (mut world, mut entities, mut game, defs, id, bench) = setup(seed);
        let wood = world.materials().id("wood").unwrap();
        let stone = world.materials().id("stone").unwrap();
        let ore = world.materials().id("iron_ore").unwrap();
        let coal = world.materials().id("coal").unwrap();
        let furnace = defs.item_id("furnace", world.materials()).unwrap();
        let bar = defs.item_id("iron_bar", world.materials()).unwrap();
        let pick = defs.item_id("iron_pickaxe", world.materials()).unwrap();
        let axe = defs.item_id("wood_axe", world.materials()).unwrap();
        let inv = game.inventories.get_mut(&id).unwrap();
        inv.add(wood, 84); // 作業台 32、斧 24、炉 8、鉄の道具 20。
        inv.add(stone, 40);
        inv.add(ore, 32);
        inv.add(coal, 8);
        game.craft(id, "workbench", &[], &defs).unwrap();
        game.craft(id, "wood_axe", &[], &defs).unwrap();
        assert_eq!(game.inventories[&id].item_count(axe), 1);
        game.place_fixture(&mut world, &mut entities, id, LEFT, bench, &defs).unwrap();
        let stations = game.fixtures.nearby_stations(&world, CellPos::new(64, 40), &defs);
        assert_eq!(game.craft(id, "iron_bar", &stations, &defs), Err(ItemError::MissingStation));
        game.craft(id, "furnace", &stations, &defs).unwrap();
        game.place_fixture(&mut world, &mut entities, id, RIGHT, furnace, &defs).unwrap();
        let stations = game.fixtures.nearby_stations(&world, CellPos::new(64, 40), &defs);
        assert_eq!(stations, vec![bench, furnace]);
        game.craft(id, "iron_bar", &stations, &defs).unwrap();
        game.craft(id, "iron_bar", &stations, &defs).unwrap();
        game.craft(id, "iron_pickaxe", &stations, &defs).unwrap();
        let inv = &game.inventories[&id];
        assert_eq!(inv.item_count(pick), 1);
        assert_eq!(
            (inv.count(wood), inv.count(stone), inv.count(ore), inv.count(coal)),
            (0, 0, 0, 0)
        );
        assert_eq!(inv.item_count(bar), 0);
        assert!(matches!(defs.item_def(pick).unwrap().kind, ItemKind::Tool { pick: 16, axe: 0 }));
        assert_eq!(defs.item_name(pick, world.materials()).as_deref(), Some("iron_pickaxe"));
    }
}
