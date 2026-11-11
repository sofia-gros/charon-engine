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
            PointerEvent::Down { id: 0, pos: [48.0, 40.0] },
            4,
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
            4,
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
