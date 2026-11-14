//! 仕様 26 §5: ボス・製作・訪問した領域の保存と旧版移行。
#![allow(clippy::unwrap_used)] // 場面テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::EntityWorld;
use charon_game::defs::Definitions;
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::progression::{PROGRESS_MAX, ProgressSave};
use charon_game::projectile::Payload;
use charon_game::save::{GAME_SAVE_VERSION, GameSave, SaveError};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_physics::PhysicsEngine;
use charon_world::generator::ChunkGenerator;
use charon_world::terrain::{AdventureGenerator, AdventureMaterials};

/// ボスを実際の弾で倒し、製作と領域の訪問を保存・復帰しても各名前は一度だけ。
#[test]
fn progression_flags_roundtrip_without_duplicates() {
    for seed in 1..=8 {
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let generator =
            AdventureGenerator { materials: AdventureMaterials::from_table(&materials).unwrap() };
        let mut world = World::new(Arc::new(materials), seed);
        for cy in 14..=16 {
            for cx in 0..=1 {
                let pos = ChunkPos::new(cx, cy);
                world.insert_loaded_chunk(pos, generator.generate(seed, pos, world.materials()));
            }
        }
        testkit::fill(&mut world, 20, 940, 120, 1000, '.');
        testkit::fill(&mut world, 20, 1001, 120, 1002, '#');
        let mut entities = EntityWorld::new();
        let first = entities.spawn_player(40 * 16, 960 * 16, &world).unwrap();
        let second = entities.spawn_player(25 * 16, 960 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        for id in [first, second] {
            game.add_character(id, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
            game.give_inventory(id).unwrap();
        }
        let gold = world.materials().id("gold_ore").unwrap();
        let coal = world.materials().id("coal").unwrap();
        let inventory = game.inventories.get_mut(&first).unwrap();
        inventory.add(gold, 16);
        inventory.add(coal, 6);
        let furnace = defs.item_id("furnace", world.materials()).unwrap();
        let before_craft = game.state_hash();
        game.craft(first, "gold_bar", &[furnace], &defs).unwrap();
        let after_craft = game.state_hash();
        assert_ne!(before_craft, after_craft);
        assert!(game.craft(first, "gold_bar", &[furnace], &defs).is_err());
        assert_eq!(game.state_hash(), after_craft, "失敗した製作では何も変わらない");
        let boss = game.spawn_enemy(&defs, &mut entities, &world, "golem", (80, 960)).unwrap();
        game.projectiles
            .spawn(&world, 76 * 16, 972 * 16, 64, 0, 0, 16, 1000, Payload::Explode { radius: 2 })
            .unwrap();
        let mut physics = PhysicsEngine::new();
        let report = game
            .step(
                &mut world,
                &mut physics,
                &mut entities,
                &[(first, GameInput::default()), (second, GameInput::default())],
                0,
                seed,
            )
            .unwrap();
        assert_eq!(report.defeated, vec![boss], "seed {seed}");
        let events = game.take_events();
        assert_eq!(events.iter().filter(|e| matches!(e, GameEvent::BossDefeated(..))).count(), 1);
        assert_eq!(game.progress.bosses.len(), 1);
        assert_eq!(game.progress.recipes.len(), 1);
        assert!(game.progress.regions.contains("region:0:0"));
        // 別の領域に 2 人で入っても世界で共有される。負の座標は床除算。
        game.progress.visit(-1, 2050);
        game.progress.visit(-1, 2050);
        assert!(game.progress.regions.contains("region:-1:1"));
        let drops_before = game.drops.len();
        let save = GameSave::capture(&game, &entities, first, world.materials(), 1).with_items(
            &game,
            first,
            &defs,
            world.materials(),
        );
        let mut loaded = GameSave::from_ron(&save.to_ron().unwrap()).unwrap();
        loaded.progress.bosses.push("golem".into());
        loaded.progress.bosses.push("absent:boss".into());
        loaded.progress.recipes.push("gold_bar".into());
        loaded.progress.recipes.push("absent:recipe".into());
        loaded.progress.regions.push("region:01:1".into());
        let loaded = GameSave::from_ron(&loaded.to_ron().unwrap()).unwrap();
        assert_eq!(loaded.progress.bosses.len(), 2, "重複は読込時に除去");
        let mut restored = GameState::from_definitions(&defs);
        loaded.apply_progress(&mut restored, &defs);
        let expected = game.progress.clone();
        assert_eq!(restored.progress, expected, "未知の名前と不正な領域を読み飛ばす: seed {seed}");
        let hash = restored.state_hash();
        loaded.apply_progress(&mut restored, &defs);
        assert_eq!(restored.progress, expected);
        assert_eq!(restored.state_hash(), hash);
        assert_eq!(game.drops.len(), drops_before, "進行復帰では戦利品を増やさない");
        let progress_only = GameSave::capture_progress_only(&game, 1);
        assert!(progress_only.player.is_none());
        assert_eq!(progress_only.progress, save.progress);
    }
}

/// 版 1 の保存は空の進行で始まり、未来版・壊れた入力は既存の進行を変更しない。
#[test]
fn legacy_save_starts_without_progress() {
    let fixture = include_str!("fixtures/game-v1.ron");
    let materials = MaterialTable::builtin().unwrap();
    let defs = Definitions::engine_defaults(&materials).unwrap();
    for seed in 1..=8 {
        let legacy = GameSave::from_ron(fixture).unwrap();
        assert_eq!(legacy.version, GAME_SAVE_VERSION);
        assert_eq!(legacy.tick, 42);
        assert_eq!(legacy.player.as_ref().unwrap().inventory, [("stone".into(), 3)]);
        assert_eq!(legacy.progress, ProgressSave::default());
        let mut game = GameState::from_definitions(&defs);
        game.progress.visit(seed, 0);
        legacy.apply_progress(&mut game, &defs);
        assert!(game.progress.is_empty());
        let newer = fixture.replace("version: 1", "version: 99");
        assert_eq!(GameSave::from_ron(&newer), Err(SaveError::Version(99)));
        assert!(matches!(GameSave::from_ron("bad ron"), Err(SaveError::Parse(_))));
        let mut excessive = GameSave::capture_progress_only(&game, 0);
        excessive.progress.bosses = (0..=PROGRESS_MAX).map(|n| format!("boss:{n}")).collect();
        assert!(matches!(
            GameSave::from_ron(&excessive.to_ron().unwrap()),
            Err(SaveError::Invalid(_))
        ));
        assert!(game.progress.is_empty(), "不正な保存では現在の進行を変更しない: seed {seed}");
        assert_eq!(GameSave::from_ron(&legacy.to_ron().unwrap()).unwrap(), legacy);
    }
}
