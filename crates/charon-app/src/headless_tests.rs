//! ウィンドウなしでデスクトップの入力 → 保存 → 再読込を通す。
#![allow(clippy::unwrap_used)] // テストの失敗は保存・復元した段階で報告する。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::Cell;
use charon_core::testkit::{box_art, count, material_of, world_from_ascii};
use charon_world::generator::FlatGenerator;
use charon_world::region::RegionStore;

use super::*;

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "charon-headless-app-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("テストの保存先を作れません: {err}"),
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn test_world(seed: u64) -> World {
    world_from_ascii(&box_art(62, 64), seed)
}

#[test]
fn headless_windows_pointer_save_reload() {
    for seed in 1..=8 {
        let dir = TestDir::new();
        let world = test_world(seed);
        let materials = Arc::clone(world.materials());
        let sand = material_of(&world, 's');
        let wood = material_of(&world, 'W');
        let stone = material_of(&world, '#');
        let metadata = WorldMeta::new(seed, &materials);
        let streamer = Streamer::with_saved_names(
            dir.0.clone(),
            Arc::clone(&materials),
            seed,
            Arc::new(FlatGenerator { stone }),
            0,
            metadata.materials.clone(),
        )
        .unwrap();
        let mut app = App::new(world, sand, test_world);
        app.streaming = Some(StreamingState { streamer, save_root: dir.0.clone(), metadata });
        for _ in 0..7 {
            app.world.step();
        }

        let placed = CellPos::new(12, 10);
        let operations = apply_pointer(
            &mut app.world,
            sand,
            None,
            PointerEvent::Down { id: 0, pos: [48.0, 40.0] },
            4.0,
            [0.0, 0.0],
            1,
        );
        assert!(!operations.is_empty(), "seed={seed}");
        assert_eq!(app.world.get(placed).material, sand, "seed={seed}");
        let background_pos = CellPos::new(20, 12);
        apply_build_pointer(
            &mut app.world,
            &mut app.background,
            Layer::Background,
            wood,
            PointerEvent::Down { id: 0, pos: [80.0, 48.0] },
            4.0,
            [0.0, 0.0],
        )
        .unwrap();
        assert_eq!(app.background.get(background_pos), wood, "seed={seed}");

        app.physics
            .create_body(
                2,
                2,
                vec![Cell { material: wood, ..Cell::default() }; 4],
                &materials,
                [32.0, 24.0],
            )
            .unwrap();
        app.physics.stamp_all(&mut app.world);
        let original_cells = app.physics.bodies[0].cells.clone();
        let original_position =
            app.physics.world.bodies[app.physics.bodies[0].handle].translation();
        app.save_world().unwrap();

        let saved = WorldMeta::load(&dir.0).unwrap().unwrap();
        assert_eq!(saved.seed, seed);
        assert_eq!(saved.tick, 7);
        assert!(saved.materials.iter().any(|name| name == "sand"));
        let background = BackgroundWorld::load(&dir.0, &materials, &saved.materials).unwrap();
        assert_eq!(background.get(background_pos), wood, "seed={seed}");
        let physics =
            PhysicsEngine::load_bodies(&dir.0, &materials, &saved.materials).unwrap().unwrap();
        assert_eq!(physics.bodies[0].cells, original_cells, "seed={seed}");
        assert_eq!(
            physics.world.bodies[physics.bodies[0].handle].translation(),
            original_position,
            "seed={seed}"
        );

        let chunk = RegionStore::new(dir.0.clone())
            .read_chunk(ChunkPos::new(0, 0), &saved.materials, Arc::clone(&materials))
            .unwrap()
            .unwrap();
        let mut restored = World::new(materials, saved.seed);
        restored.insert_loaded_chunk(ChunkPos::new(0, 0), chunk);
        restored.restore_tick(saved.tick);
        assert_eq!(restored.get(placed).material, sand, "seed={seed}");
        assert_eq!(count(&restored, 's'), count(&app.world, 's'), "seed={seed}");
    }
}

/// 仕様 19 §7.1: 実際の App の保存・復帰経路で、初期アイテムを二重にしない。
fn item_app(seed: u64, defs: &charon_game::defs::Definitions) -> App {
    use charon_game::vitals::{MAX_HP, Vitals};

    let world = test_world(seed);
    let selected = material_of(&world, 's');
    let mut app = App::new(world, selected, test_world);
    app.options.definitions = Some(Arc::new(defs.clone()));
    app.game = GameState::from_definitions(defs);
    let id = app.entities.spawn_player(10 * 16, 20 * 16, &app.world).unwrap();
    app.game.add_character(id, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
    app.game.give_inventory(id).unwrap();
    app.player = Some(id);
    app
}

#[test]
fn app_save_reloads_items_hotbar_and_equipment() {
    use charon_game::items::{EquipSlot, ItemId};
    use charon_game::save::GameSave;

    for seed in 1..=8 {
        let dir = TestDir::new();
        let world = test_world(seed);
        let materials = Arc::clone(world.materials());
        let defs = charon_game::defs::Definitions::engine_defaults(&materials).unwrap();
        let mut app = item_app(seed, &defs);
        let id = app.player.unwrap();
        let pick = defs.item_id("wood_pickaxe", &materials).unwrap();
        let helmet = defs.item_id("wood_helmet", &materials).unwrap();
        let stone = material_of(&app.world, '#');
        let wood = material_of(&app.world, 'W');
        let inv = app.game.inventories.get_mut(&id).unwrap();
        inv.add_item(pick, 2);
        inv.add_item(helmet, 1);
        inv.add(stone, 7);
        inv.add_block(wood, 2);
        inv.pin(3, pick);
        inv.select(3);
        app.game.equip(id, helmet, &defs).unwrap();
        let metadata = WorldMeta::new(seed, &materials);
        let streamer = Streamer::with_saved_names(
            dir.0.clone(),
            Arc::clone(&materials),
            seed,
            Arc::new(FlatGenerator { stone }),
            0,
            metadata.materials.clone(),
        )
        .unwrap();
        app.streaming = Some(StreamingState { streamer, save_root: dir.0.clone(), metadata });
        app.save_world().unwrap();
        let save = GameSave::read(&dir.0).unwrap().unwrap();
        assert_eq!(save.player.as_ref().unwrap().hotbar.len(), 10, "seed={seed}");
        let mut restored = item_app(seed, &defs);
        let restored_id = restored.player.unwrap();
        // 生まれた直後の初期道具が、保存済みの道具へ置き換わる。
        restored.game.inventories.get_mut(&restored_id).unwrap().add_item(pick, 1);
        restore_player_save(
            &mut restored.game,
            &mut restored.entities,
            &restored.world,
            restored.options.definitions.as_deref(),
            restored_id,
            &save,
        );
        let inv = &restored.game.inventories[&restored_id];
        assert_eq!(inv.item_count(pick), 2, "seed={seed}: 初期道具を重複させない");
        assert_eq!(inv.item_count(ItemId::cells(stone)), 7, "seed={seed}");
        assert_eq!(inv.item_count(ItemId::block(wood)), 2, "seed={seed}");
        assert_eq!(inv.hotbar(3), Some(pick), "seed={seed}");
        assert_eq!(inv.selected(), 3, "seed={seed}");
        assert_eq!(
            restored.game.equipment[&restored_id].get(EquipSlot::Head),
            Some(helmet),
            "seed={seed}"
        );
    }
}

#[test]
fn legacy_game_save_preserves_starter_items() {
    let world = test_world(1);
    let materials = Arc::clone(world.materials());
    let defs = charon_game::defs::Definitions::engine_defaults(&materials).unwrap();
    let mut app = item_app(1, &defs);
    let id = app.player.unwrap();
    let pick = defs.item_id("wood_pickaxe", &materials).unwrap();
    app.game.inventories.get_mut(&id).unwrap().add_item(pick, 1);
    let mut saved = charon_game::save::GameSave::capture(
        &app.game,
        &app.entities,
        id,
        &materials,
        app.world.tick(),
    );
    let stone = material_of(&app.world, '#');
    saved.player.as_mut().unwrap().inventory.push((materials.get(stone).name.clone(), 7));
    saved.player.as_mut().unwrap().inventory.push(("unknown:material".to_owned(), 3));
    assert!(saved.player.as_ref().unwrap().hotbar.is_empty(), "旧保存にはホットバーがない");
    restore_player_save(
        &mut app.game,
        &mut app.entities,
        &app.world,
        app.options.definitions.as_deref(),
        id,
        &saved,
    );
    let inv = &app.game.inventories[&id];
    assert_eq!(inv.item_count(pick), 1, "旧保存では初期道具を維持");
    assert_eq!(inv.count(stone), 7);
}

#[test]
fn rainbow_brush_colors_by_x() {
    use charon_game::mods::{ModDirectory, ModPack};
    let pack = ModPack::load(&[ModDirectory {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods/fse_demo"),
        script: false,
    }])
    .unwrap();
    let mut world = pack.new_world(1);
    world.insert_chunk(ChunkPos::new(0, 0));
    let first = pack.materials.id("fse_demo:rainbow_sand_0").unwrap();
    let operations = apply_pointer(
        &mut world,
        MaterialId::EMPTY, // 虹色のときは選択中の材質を使わない。
        Some(first),
        PointerEvent::Down { id: 0, pos: [32.0, 32.0] },
        1.0,
        [0.0, 0.0],
        12,
    );
    assert!(!operations.is_empty());
    let mut hues = std::collections::BTreeSet::new();
    for op in &operations {
        let expected = rainbow_material(first, op.x);
        assert_eq!(MaterialId(op.material), expected, "x={}", op.x);
        assert_eq!(world.get(CellPos::new(op.x, op.y)).material, expected);
        hues.insert(op.material);
    }
    // 直径 25 セルに 4 セルごとの色がかかるので、6 色以上になる。
    assert!(hues.len() >= 6, "{hues:?}");
}
