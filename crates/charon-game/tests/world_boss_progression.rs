//! 仕様 26 §4: 資源の深さに沿う敵の帯と、生成された深層のボス戦。
#![allow(clippy::unwrap_used)] // 場面テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::EntityWorld;
use charon_game::defs::Definitions;
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::items::EquipSlot;
use charon_game::projectile::Payload;
use charon_game::spawner::Spawner;
use charon_game::vitals::{MAX_HP, Vitals};
use charon_physics::PhysicsEngine;
use charon_world::generator::ChunkGenerator;
use charon_world::terrain::{
    AdventureGenerator, AdventureMaterials, BOTTOM, SURFACE_BASE, surface_y,
};

/// 地表、石炭、鉄/金、深い金の各帯には指定の敵だけが現れ、ボスは最後の帯だけ。
#[test]
fn enemy_depth_matches_resource_tiers() {
    let spawner = Spawner::adventure();
    let samples: &[(i32, &[&str])] = &[
        (149, &[]),
        (150, &["archer", "walker"]),
        (259, &["archer", "walker"]),
        (260, &["shooter", "skeleton"]),
        (559, &["shooter", "skeleton"]),
        (560, &["mage", "skeleton"]),
        (919, &["mage", "skeleton"]),
        (920, &["golem", "mage"]),
        (1399, &["golem", "mage"]),
        (BOTTOM, &[]),
    ];
    assert_eq!(spawner.max_enemies, 8);
    for seed in 1..=8 {
        assert_eq!(surface_y(seed, 0), SURFACE_BASE);
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        for &(y, expected) in samples {
            let mut names: Vec<_> = spawner
                .rules
                .iter()
                .filter(|rule| (rule.min_y..rule.max_y).contains(&y))
                .map(|rule| rule.enemy.as_str())
                .collect();
            names.sort_unstable();
            assert_eq!(names, expected, "seed {seed}: y={y}");
            for rule in spawner.rules.iter().filter(|r| (r.min_y..r.max_y).contains(&y)) {
                assert!(rule.weight > 0);
                assert_eq!(defs.enemy(&rule.enemy).unwrap().boss, rule.boss);
            }
        }
    }
}

/// 生成した深層を掘った戦場で金の防具を着け、弾による実ダメージでボスを一度だけ倒す。
#[test]
fn boss_encounter_uses_terrain_and_combat() {
    for seed in 1..=8 {
        let materials = MaterialTable::builtin().unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let generator =
            AdventureGenerator { materials: AdventureMaterials::from_table(&materials).unwrap() };
        let stone = materials.id("stone").unwrap();
        let gold = materials.id("gold_ore").unwrap();
        let iron = materials.id("iron_ore").unwrap();
        let mut world = World::new(Arc::new(materials), seed);
        for cy in 14..=16 {
            for cx in 0..=1 {
                let pos = ChunkPos::new(cx, cy);
                world.insert_loaded_chunk(pos, generator.generate(seed, pos, world.materials()));
            }
        }
        assert!((920..BOTTOM).contains(&960));
        // 深層の地形に、プレイヤーとボスの体が入る戦場と石の床を掘る。
        testkit::fill(&mut world, 20, 940, 120, 1000, '.');
        testkit::fill(&mut world, 20, 1001, 120, 1002, '#');
        let floor = CellPos::new(40, 1001);
        assert_eq!(world.get(floor).material, stone);
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(40 * 16, 960 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        for name in ["gold_helmet", "gold_chestplate", "gold_greaves"] {
            let item = defs.item_id(name, world.materials()).unwrap();
            game.inventories.get_mut(&player).unwrap().add_item(item, 1);
            game.equip(player, item, &defs).unwrap();
        }
        assert!(game.equipment[&player].get(EquipSlot::Head).is_some());
        let boss = game.spawn_enemy(&defs, &mut entities, &world, "golem", (80, 960)).unwrap();
        let before = game.vitals.get(boss).unwrap().hp;
        game.projectiles
            .spawn(&world, 76 * 16, 972 * 16, 64, 0, 0, 16, 1000, Payload::Explode { radius: 2 })
            .unwrap();
        let mut physics = PhysicsEngine::new();
        let report = game
            .step(
                &mut world,
                &mut physics,
                &mut entities,
                &[(player, GameInput::default())],
                0,
                seed,
            )
            .unwrap();
        assert_eq!(before, 600);
        assert_eq!(report.defeated, vec![boss], "seed {seed}: 弾がボスを倒す");
        assert!(report.hits.iter().any(|hit| hit.entity == Some(boss)));
        assert_eq!(
            game.vitals.get(player).unwrap().armor.defense,
            27,
            "seed {seed}: 金セットの防御"
        );
        assert_eq!(world.get(floor).material, stone, "seed {seed}: 離れた床は保つ");
        let events = game.take_events();
        let defeat: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                GameEvent::Defeated(id, pos) if *id == boss => Some(*pos),
                _ => None,
            })
            .collect();
        let boss_events: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                GameEvent::BossDefeated(id, pos) if *id == boss => Some(*pos),
                _ => None,
            })
            .collect();
        assert_eq!(defeat.len(), 1);
        assert_eq!(boss_events, defeat, "seed {seed}: 同じ ID と位置の出来事");
        let dropped = |game: &GameState, material| {
            game.drops
                .iter()
                .filter(|drop| drop.material == material)
                .map(|drop| drop.count)
                .sum::<u32>()
        };
        assert_eq!((dropped(&game, gold), dropped(&game, iron)), (40, 40));
        game.step(
            &mut world,
            &mut physics,
            &mut entities,
            &[(player, GameInput::default())],
            1,
            seed,
        )
        .unwrap();
        assert!(
            game.take_events().iter().all(|event| !matches!(
                event,
                GameEvent::Defeated(..) | GameEvent::BossDefeated(..)
            ))
        );
        assert!(game.enemies.is_empty());
        assert_eq!(
            (dropped(&game, gold), dropped(&game, iron)),
            (40, 40),
            "seed {seed}: 戦利品は一度だけ"
        );
    }
}
