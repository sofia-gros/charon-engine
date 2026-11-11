#![allow(clippy::unwrap_used)] // 保存の前提が崩れた場合は即時に報告する。

use std::sync::Arc;

use charon_core::cell::Cell;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::testkit::{box_art, material_of, world_from_ascii};
use charon_core::world::World;
use charon_physics::PhysicsEngine;
use charon_world::background::BackgroundWorld;
use charon_world::generator::FlatGenerator;
use charon_world::save::WorldMeta;
use charon_world::streaming::{LOAD_DELAY, MemoryStore, Streamer};
use charon_world::web_save::{WebSaveError, WebSnapshot};

#[test]
fn web_save_roundtrip() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let materials = Arc::clone(world.materials());
        let sand = material_of(&world, 's');
        let wood = material_of(&world, 'W');
        let pos = CellPos::new(12, 10);
        world.set(pos, world.make_cell(sand, pos));
        let mut background = BackgroundWorld::new();
        background.set(CellPos::new(-2, 2), wood);
        let mut physics = PhysicsEngine::new();
        let body_cell = Cell { material: wood, temp: 42, ..Cell::default() };
        let index =
            physics.create_body(2, 2, vec![body_cell; 4], &materials, [20.0, 20.0]).unwrap();
        physics.step(&mut world).unwrap();
        physics.unstamp_all(&mut world);
        let handle = physics.bodies[index].handle;
        let original = &physics.world.bodies[handle];
        let expected_pos = original.translation();
        let expected_vel = original.linvel();
        let mut meta = WorldMeta::new(seed, &materials);
        meta.tick = world.tick();
        let store = Arc::new(MemoryStore::new());
        let generator = Arc::new(FlatGenerator { stone: material_of(&world, '#') });
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
        let snapshot = WebSnapshot {
            metadata: meta,
            chunks: store.snapshot().unwrap(),
            background: background
                .to_bytes(&materials, &charon_world::material_names(&materials))
                .unwrap(),
            bodies: physics
                .encode_bodies(&materials, &charon_world::material_names(&materials))
                .unwrap(),
        };
        let data = snapshot.encode().unwrap();
        let decoded = WebSnapshot::decode(&data).unwrap();
        assert_eq!(decoded.metadata.seed, seed);
        assert_eq!(decoded.metadata.materials, snapshot.metadata.materials);
        let restored_background = BackgroundWorld::from_bytes(
            &decoded.background,
            &materials,
            &decoded.metadata.materials,
        )
        .unwrap();
        let restored_physics =
            PhysicsEngine::decode_bodies(&decoded.bodies, &materials, &decoded.metadata.materials)
                .unwrap();
        let mut restored_world = World::new(Arc::clone(&materials), decoded.metadata.seed);
        restored_world.restore_tick(decoded.metadata.tick);
        let restored_store = Arc::new(MemoryStore::new());
        restored_store.restore(decoded.chunks).unwrap();
        let mut restored_streamer = Streamer::with_memory_store(
            restored_store,
            Arc::clone(&materials),
            seed,
            generator,
            0,
            decoded.metadata.materials,
        )
        .unwrap();
        let saved_tick = restored_world.tick();
        for tick in 0..=LOAD_DELAY {
            restored_streamer.tick(&mut restored_world, &[ChunkPos::new(0, 0)]).unwrap();
            if tick < LOAD_DELAY {
                assert!(
                    restored_world.chunk(ChunkPos::new(0, 0)).is_none(),
                    "seed={seed} tick={tick}"
                );
            }
            restored_world.restore_tick(saved_tick + tick + 1);
        }
        assert_eq!(restored_world.materials().get(restored_world.get(pos).material).name, "sand");
        assert_eq!(restored_world.get(pos).material, world.get(pos).material);
        assert_eq!(restored_background.get(CellPos::new(-2, 2)), wood);
        assert_eq!(restored_physics.bodies[0].cells, physics.bodies[0].cells);
        let loaded = &restored_physics.world.bodies[restored_physics.bodies[0].handle];
        assert!((loaded.translation() - expected_pos).length() < 0.00001, "seed={seed}");
        assert_eq!(loaded.linvel(), expected_vel, "seed={seed}");
        restored_streamer.finish().unwrap();
        let mut newer = data;
        newer[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(WebSnapshot::decode(&newer), Err(WebSaveError::NewerVersion(2))));
    }
}
