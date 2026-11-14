//! 仕様 26 §3: 深い金の資源を既存の炉・作業台で道具と防具へ変える。
#![allow(clippy::unwrap_used)] // 場面テストの前提が崩れたら直ちに失敗させる。

use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::game::GameState;
use charon_game::items::{EquipSlot, ItemId, ItemKind};
use charon_game::loadout::ItemError;
use charon_game::save::{GAME_SAVE_VERSION, GameSave};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};

fn scene(seed: u64) -> (World, EntityWorld, GameState, Definitions, EntityId) {
    let world = testkit::world_from_ascii(&testkit::box_art(72, 64), seed);
    let defs = Definitions::engine_defaults(world.materials()).unwrap();
    let mut entities = EntityWorld::new();
    let player = entities.spawn_player(10 * 16, 26 * 16, &world).unwrap();
    let mut game = GameState::from_definitions(&defs);
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    (world, entities, game, defs, player)
}

fn item(defs: &Definitions, world: &World, name: &str) -> ItemId {
    defs.item_id(name, world.materials()).unwrap()
}

/// 金鉱石・石炭と炉、金/鉄の延べ棒と作業台を順番に使う。失敗時は無変更。
#[test]
fn gold_tier_requires_deep_ore_and_station() {
    for seed in 1..=8 {
        let (world, _entities, mut game, defs, player) = scene(seed);
        let ore = world.materials().id("gold_ore").unwrap();
        let coal = world.materials().id("coal").unwrap();
        let wood = testkit::material_of(&world, 'W');
        let furnace = item(&defs, &world, "furnace");
        let bench = item(&defs, &world, "workbench");
        let bar = item(&defs, &world, "gold_bar");
        let iron = item(&defs, &world, "iron_bar");
        let pick = item(&defs, &world, "gold_pickaxe");
        let helmet = item(&defs, &world, "gold_helmet");
        let chestplate = item(&defs, &world, "gold_chestplate");
        let greaves = item(&defs, &world, "gold_greaves");
        let inventory = game.inventories.get_mut(&player).unwrap();
        inventory.add(ore, 16);
        inventory.add(coal, 5);
        inventory.add_item(iron, 4);
        inventory.add(wood, 20);
        let before = inventory.clone();
        assert_eq!(
            game.craft(player, "gold_bar", &[furnace], &defs),
            Err(ItemError::MissingIngredients)
        );
        assert_eq!(game.inventories[&player], before, "seed {seed}: 石炭不足は原子的");
        game.inventories.get_mut(&player).unwrap().add(coal, 1);
        let before = game.inventories[&player].clone();
        for nearby in [&[][..], &[bench][..]] {
            assert_eq!(
                game.craft(player, "gold_bar", nearby, &defs),
                Err(ItemError::MissingStation)
            );
            assert_eq!(game.inventories[&player], before, "seed {seed}: 炉なしでは変化しない");
        }
        game.craft(player, "gold_bar", &[furnace], &defs).unwrap();
        assert_eq!(game.inventories[&player].item_count(bar), 1);
        let before = game.inventories[&player].clone();
        assert_eq!(
            game.craft(player, "gold_pickaxe", &[bench], &defs),
            Err(ItemError::MissingIngredients)
        );
        assert_eq!(game.inventories[&player], before);
        // 合計 9 本: 道具に 2、本体/頭/脚に 3+2+2。金鉱石 144 と石炭 54 を消費する。
        game.inventories.get_mut(&player).unwrap().add(ore, 8 * 16);
        game.inventories.get_mut(&player).unwrap().add(coal, 8 * 6);
        for _ in 0..8 {
            game.craft(player, "gold_bar", &[furnace], &defs).unwrap();
        }
        let before = game.inventories[&player].clone();
        assert_eq!(
            game.craft(player, "gold_pickaxe", &[furnace], &defs),
            Err(ItemError::MissingStation)
        );
        assert_eq!(game.inventories[&player], before);
        game.craft(player, "gold_pickaxe", &[bench], &defs).unwrap();
        for (recipe, output) in
            [("gold_helmet", helmet), ("gold_chestplate", chestplate), ("gold_greaves", greaves)]
        {
            game.craft(player, recipe, &[bench], &defs).unwrap();
            assert_eq!(game.inventories[&player].item_count(output), 1);
            game.equip(player, output, &defs).unwrap();
        }
        let inv = &game.inventories[&player];
        assert_eq!(
            (inv.count(ore), inv.count(coal), inv.item_count(bar), inv.item_count(iron)),
            (0, 0, 0, 0)
        );
        assert_eq!(inv.item_count(pick), 1);
        assert!(matches!(defs.item_def(pick).unwrap().kind, ItemKind::Tool { pick: 24, axe: 0 }));
        let stats = game.equipment[&player].stats(|id| defs.item_def(id));
        assert_eq!((stats.defense, stats.resist.fire), (27, 15));
        let stone = item(&defs, &world, "stone_helmet");
        assert!(defs.item_def(helmet).unwrap().rating() > defs.item_def(stone).unwrap().rating());
    }
}

/// 金の道具・3 部位と印を名前で保存し、別のゲームで能力値を再計算する。
#[test]
fn equipment_progression_survives_save() {
    for seed in 1..=8 {
        let (world, entities, mut game, defs, player) = scene(seed);
        let pick = item(&defs, &world, "gold_pickaxe");
        let bar = item(&defs, &world, "gold_bar");
        let head = item(&defs, &world, "gold_helmet");
        let body = item(&defs, &world, "gold_chestplate");
        let legs = item(&defs, &world, "gold_greaves");
        let inv = game.inventories.get_mut(&player).unwrap();
        inv.add_item(pick, 1);
        inv.add_item(bar, 3);
        inv.pin(0, pick);
        inv.select(0);
        for item in [head, body, legs] {
            game.inventories.get_mut(&player).unwrap().add_item(item, 1);
            game.equip(player, item, &defs).unwrap();
        }
        let saved = GameSave::capture(&game, &entities, player, world.materials(), 42).with_items(
            &game,
            player,
            &defs,
            world.materials(),
        );
        assert_eq!(saved.version, GAME_SAVE_VERSION);
        let text = saved.to_ron().unwrap();
        for name in ["gold_pickaxe", "gold_bar", "gold_helmet", "gold_chestplate", "gold_greaves"] {
            assert!(text.contains(name), "seed {seed}: {name}");
        }
        let saved = GameSave::from_ron(&text).unwrap();
        let (world2, mut entities2, mut game2, defs2, player2) = scene(seed);
        saved.apply(&mut game2, &mut entities2, &world2, player2);
        assert_eq!(saved.apply_items(&mut game2, player2, &defs2, world2.materials()), 3);
        assert_eq!(game2.inventories[&player2].item_count(bar), 3);
        assert_eq!(game2.inventories[&player2].selected_item(), Some(pick));
        for (slot, item) in
            [(EquipSlot::Head, head), (EquipSlot::Body, body), (EquipSlot::Legs, legs)]
        {
            assert_eq!(game2.equipment[&player2].get(slot), Some(item));
            assert_eq!(game2.inventories[&player2].item_count(item), 0);
        }
        let stats = game2.equipment[&player2].stats(|id| defs2.item_def(id));
        assert_eq!((stats.defense, stats.resist.fire), (27, 15));
        let resaved = GameSave::capture(&game2, &entities2, player2, world2.materials(), 43)
            .with_items(&game2, player2, &defs2, world2.materials());
        assert_eq!(
            resaved
                .player
                .unwrap()
                .items
                .iter()
                .find(|(name, _)| name == "gold_bar")
                .map(|(_, n)| *n),
            Some(3)
        );
    }
}
