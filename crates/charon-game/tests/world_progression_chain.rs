//! 仕様 26 §6: 生成資源→設備→装備→深層ボス→回収→別インスタンスで再開。
#![allow(clippy::unwrap_used)] // 場面の前提が崩れたら直ちに失敗させる。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::EntityId;
use charon_game::EntityWorld;
use charon_game::blocks::BlockPos;
use charon_game::cursor::center_of;
use charon_game::defs::Definitions;
use charon_game::fixtures::FixtureMap;
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::items::EquipSlot;
use charon_game::projectile::Payload;
use charon_game::save::GameSave;
use charon_game::vitals::{MAX_HP, Vitals};
use charon_physics::PhysicsEngine;
use charon_world::generator::ChunkGenerator;
use charon_world::region::RegionStore;
use charon_world::save::WorldMeta;
use charon_world::terrain::{AdventureGenerator, AdventureMaterials};

struct Scene {
    world: World,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    player: EntityId,
    harvested: BTreeMap<MaterialId, Vec<CellPos>>,
    events: Vec<GameEvent>,
}

/// 先に純粋な生成座標から採集先を選び、実チャンクの同じ材質だけを 1 セルずつ採集する。
fn generated_resources(
    seed: u64,
    materials: Arc<MaterialTable>,
) -> (World, BTreeMap<MaterialId, Vec<CellPos>>) {
    let generator =
        AdventureGenerator { materials: AdventureMaterials::from_table(&materials).unwrap() };
    let quotas =
        [("wood", 60usize), ("stone", 40), ("coal", 70), ("iron_ore", 64), ("gold_ore", 144)];
    let mut needed: BTreeMap<_, _> =
        quotas.into_iter().map(|(name, n)| (materials.id(name).unwrap(), n)).collect();
    let mut harvested: BTreeMap<MaterialId, Vec<CellPos>> = BTreeMap::new();
    for y in 128..1200 {
        for x in -64..320 {
            let pos = CellPos::new(x, y);
            let id = generator.cell_at(seed, x, y).map_or(MaterialId::EMPTY, |cell| cell.material);
            if let Some(left) = needed.get_mut(&id)
                && *left > 0
            {
                harvested.entry(id).or_default().push(pos);
                *left -= 1;
            }
        }
        if needed.values().all(|&left| left == 0) {
            break;
        }
    }
    assert!(
        needed.values().all(|&left| left == 0),
        "seed {seed}: 生成された鉱脈と木が足りない: {needed:?}"
    );
    let mut chunks: BTreeSet<_> = harvested.values().flatten().map(|pos| pos.chunk()).collect();
    for cy in 14..=16 {
        for cx in 0..=1 {
            chunks.insert(ChunkPos::new(cx, cy));
        }
    }
    let mut world = World::new(materials, seed);
    for pos in chunks {
        world.insert_loaded_chunk(pos, generator.generate(seed, pos, world.materials()));
    }
    (world, harvested)
}

fn run(seed: u64) -> Scene {
    let materials = Arc::new(MaterialTable::builtin().unwrap());
    let defs = Definitions::engine_defaults(&materials).unwrap();
    let (mut world, harvested) = generated_resources(seed, materials);
    let mut entities = EntityWorld::new();
    let mut game = GameState::from_definitions(&defs);
    // テスト用の原子的な採掘代用。実際の掘削・移動の所要時間を表さない。
    for (&id, positions) in &harvested {
        for &pos in positions {
            assert_eq!(world.get(pos).material, id, "seed {seed}: 生成器と実チャンク");
            world.set(pos, world.make_cell(MaterialId::EMPTY, pos));
        }
    }
    // 採集した資源を持つプレイヤーの安全な深層戦場。道中の経路は §2 で別に検証する。
    testkit::fill(&mut world, 20, 940, 120, 1000, '.');
    testkit::fill(&mut world, 20, 1001, 120, 1002, '#');
    let player = entities.spawn_player(40 * 16, 960 * 16, &world).unwrap();
    game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    game.give_inventory(player).unwrap();
    for (&id, positions) in &harvested {
        assert_eq!(
            game.inventories.get_mut(&player).unwrap().add(id, positions.len() as u32),
            positions.len() as u32
        );
    }
    let bench = defs.item_id("workbench", world.materials()).unwrap();
    let furnace = defs.item_id("furnace", world.materials()).unwrap();
    game.craft(player, "workbench", &[], &defs).unwrap();
    game.place_fixture(
        &mut world,
        &mut entities,
        player,
        BlockPos { bx: 4, by: 123 },
        bench,
        &defs,
    )
    .unwrap();
    let stations =
        game.fixtures.nearby_stations(&world, center_of(&entities, player).unwrap(), &defs);
    assert_eq!(stations, vec![bench]);
    game.craft(player, "furnace", &stations, &defs).unwrap();
    game.place_fixture(
        &mut world,
        &mut entities,
        player,
        BlockPos { bx: 6, by: 123 },
        furnace,
        &defs,
    )
    .unwrap();
    let stations =
        game.fixtures.nearby_stations(&world, center_of(&entities, player).unwrap(), &defs);
    assert!(stations.contains(&bench) && stations.contains(&furnace));
    for _ in 0..4 {
        game.craft(player, "iron_bar", &stations, &defs).unwrap();
    }
    for _ in 0..9 {
        game.craft(player, "gold_bar", &stations, &defs).unwrap();
    }
    game.craft(player, "gold_pickaxe", &stations, &defs).unwrap();
    let pick = defs.item_id("gold_pickaxe", world.materials()).unwrap();
    game.inventories.get_mut(&player).unwrap().pin(0, pick);
    for name in ["gold_helmet", "gold_chestplate", "gold_greaves"] {
        game.craft(player, name, &stations, &defs).unwrap();
        let item = defs.item_id(name, world.materials()).unwrap();
        game.equip(player, item, &defs).unwrap();
    }
    assert_eq!(game.equipment[&player].stats(|item| defs.item_def(item)).defense, 27);
    let boss = game.spawn_enemy(&defs, &mut entities, &world, "golem", (80, 960)).unwrap();
    game.projectiles
        .spawn(&world, 76 * 16, 972 * 16, 64, 0, 0, 16, 1000, Payload::Explode { radius: 2 })
        .unwrap();
    let mut physics = PhysicsEngine::new();
    let report = game
        .step(&mut world, &mut physics, &mut entities, &[(player, GameInput::default())], 0, seed)
        .unwrap();
    assert_eq!(report.defeated, vec![boss], "seed {seed}: 実ダメージによる撃破");
    let gold = world.materials().id("gold_ore").unwrap();
    let iron = world.materials().id("iron_ore").unwrap();
    let dropped = |game: &GameState, id| {
        game.drops.iter().filter(|drop| drop.material == id).map(|drop| drop.count).sum::<u32>()
    };
    assert_eq!((dropped(&game, gold), dropped(&game, iron)), (40, 40));
    // 30 tick の拾得待ちを進めてから、戦利品の中心へプレイヤーを移す。
    for tick in 1..=31 {
        game.step(
            &mut world,
            &mut physics,
            &mut entities,
            &[(player, GameInput::default())],
            tick,
            seed,
        )
        .unwrap();
    }
    let first_drop = game.drops.iter().next().unwrap();
    let (x, y) = (first_drop.x16, first_drop.y16);
    entities.get_mut(player).unwrap().x16 = x - 8 * 16;
    entities.get_mut(player).unwrap().y16 = y - 12 * 16;
    game.step(&mut world, &mut physics, &mut entities, &[(player, GameInput::default())], 32, seed)
        .unwrap();
    assert_eq!(
        (game.inventories[&player].count(gold), game.inventories[&player].count(iron)),
        (40, 40)
    );
    assert!(game.drops.is_empty());
    // 保存する位置は床と重ならない、元の安全な場所へ戻す。
    entities.get_mut(player).unwrap().x16 = 40 * 16;
    entities.get_mut(player).unwrap().y16 = 960 * 16;
    let events = game.take_events();
    assert_eq!(events.iter().filter(|e| matches!(e, GameEvent::BossDefeated(..))).count(), 1);
    assert_eq!(events.iter().filter(|e| matches!(e, GameEvent::Defeated(..))).count(), 1);
    assert_eq!(events.iter().filter(|e| matches!(e, GameEvent::Pickup(..))).count(), 2);
    assert!(game.progress.bosses.contains("golem"));
    assert!(game.progress.recipes.contains("gold_pickaxe"));
    assert!(game.progress.regions.contains("region:0:0"));
    Scene { world, entities, game, defs, player, harvested, events }
}

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "charon-progression-chain-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 同じ seed・生成順・固定操作なら、資源の採集・製作・撃破と拾得が完全に一致する。
#[test]
fn world_progression_chain_is_deterministic() {
    for seed in 1..=8 {
        let a = run(seed);
        let b = run(seed);
        assert_eq!(a.world.state_hash(), b.world.state_hash(), "seed {seed}: 地形");
        assert_eq!(a.entities.state_hash(), b.entities.state_hash(), "seed {seed}: プレイヤー");
        assert_eq!(a.game.state_hash(), b.game.state_hash(), "seed {seed}: 進行と持ち物");
        assert_eq!(a.events, b.events, "seed {seed}: 出来事の順序");
        for (&id, cells) in &a.harvested {
            assert_eq!(cells.len(), b.harvested[&id].len());
            for &pos in cells {
                // 戦場の石床に重なった鉱脈はテスト用戦場の準備で上書きされる。
                if !(20..=120).contains(&pos.x) || !(940..=1002).contains(&pos.y) {
                    assert_eq!(
                        a.world.get(pos).material,
                        MaterialId::EMPTY,
                        "seed {seed}: 採集したセル"
                    );
                }
            }
        }
        // 進行の記録自体はセルに触れない。溶岩を含まない隔離場面だけが休眠対象。
        let mut dry = testkit::world_from_ascii("W..#\n####", seed);
        testkit::assert_sleeps(&mut dry, 2000);
        let asleep = dry.state_hash();
        dry.step();
        assert_eq!(dry.state_hash(), asleep);
    }
}

/// リージョン・設備・game.ron を別世界へ戻し、回収済み報酬と進行を重ねずに保つ。
#[test]
fn progression_reload_keeps_world_and_rewards() {
    for seed in 1..=8 {
        let scene = run(seed);
        let dir = TempDir::new();
        let store = RegionStore::new(dir.0.clone());
        let meta = WorldMeta::new(seed, scene.world.materials());
        meta.save(&dir.0).unwrap();
        for pos in scene.world.chunk_positions() {
            store
                .write_chunk(pos, scene.world.chunk(pos).unwrap(), scene.world.materials())
                .unwrap();
        }
        scene.game.fixtures.write(&dir.0).unwrap();
        let saved = GameSave::capture(
            &scene.game,
            &scene.entities,
            scene.player,
            scene.world.materials(),
            scene.world.tick(),
        )
        .with_items(&scene.game, scene.player, &scene.defs, scene.world.materials());
        saved.write(&dir.0).unwrap();
        let read_meta = WorldMeta::load(&dir.0).unwrap().unwrap();
        let materials = Arc::new(MaterialTable::builtin().unwrap());
        let mut world = World::new(materials, read_meta.seed);
        for pos in scene.world.chunk_positions() {
            let chunk = store
                .read_chunk(pos, &read_meta.materials, Arc::clone(world.materials()))
                .unwrap()
                .unwrap();
            world.insert_loaded_chunk(pos, chunk);
        }
        world.restore_tick(read_meta.tick);
        assert_eq!(world.state_hash(), scene.world.state_hash(), "seed {seed}: 全チャンクの状態");
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(40 * 16, 960 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&scene.defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), scene.defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        game.fixtures = FixtureMap::read(&dir.0, &scene.defs, &world).unwrap();
        let save = GameSave::read(&dir.0).unwrap().unwrap();
        save.apply(&mut game, &mut entities, &world, player);
        assert_eq!(save.apply_items(&mut game, player, &scene.defs, world.materials()), 3);
        save.apply_progress(&mut game, &scene.defs);
        assert_eq!(game.progress, scene.game.progress);
        let progress_hash = game.progress.clone();
        save.apply_progress(&mut game, &scene.defs);
        assert_eq!(game.progress, progress_hash);
        assert_eq!(game.fixtures.state_hash(), scene.game.fixtures.state_hash());
        let gold = world.materials().id("gold_ore").unwrap();
        let iron = world.materials().id("iron_ore").unwrap();
        assert_eq!(
            (game.inventories[&player].count(gold), game.inventories[&player].count(iron)),
            (40, 40)
        );
        assert_eq!(
            game.inventories[&player].selected_item(),
            scene.game.inventories[&scene.player].selected_item()
        );
        assert_eq!(
            game.equipment[&player].get(EquipSlot::Head),
            scene.game.equipment[&scene.player].get(EquipSlot::Head)
        );
        assert!(game.enemies.is_empty() && game.drops.is_empty() && game.projectiles.is_empty());
        game.step(
            &mut world,
            &mut PhysicsEngine::new(),
            &mut entities,
            &[(player, GameInput::default())],
            33,
            seed,
        )
        .unwrap();
        let armor = game.vitals.get(player).unwrap().armor;
        assert_eq!((armor.defense, armor.fire), (27, 15));
        assert_eq!(
            (game.inventories[&player].count(gold), game.inventories[&player].count(iron)),
            (40, 40)
        );
        assert!(game.take_events().iter().all(|e| !matches!(
            e,
            GameEvent::BossDefeated(..) | GameEvent::Defeated(..) | GameEvent::Pickup(..)
        )));
    }
}
