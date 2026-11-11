#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::Cell;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::testkit::{box_art, fill, place, to_ascii, world_from_ascii};
use charon_physics::{CELLS_PER_METER, PhysicsEngine};

#[test]
fn terrain_rebuild_is_limited_and_nearby() {
    let art = vec![".".repeat(192); 192].join("\n");
    let mut world = world_from_ascii(&art, 1);
    for cy in 0..3 {
        for cx in 0..3 {
            place(&mut world, cx * 64 + 32, cy * 64 + 32, '#');
        }
    }
    let stone = world.materials().id("stone").unwrap();
    let mut engine = PhysicsEngine::new();
    engine
        .create_body(
            2,
            2,
            vec![Cell { material: stone, ..Cell::default() }; 4],
            world.materials(),
            [96.0, 96.0],
        )
        .unwrap();
    engine.update_terrain(&mut world);
    assert_eq!(
        world
            .chunk_positions()
            .iter()
            .filter(|&&pos| world.chunk(pos).unwrap().collider_dirty)
            .count(),
        5
    );
    assert!(!world.chunk(ChunkPos::new(1, 1)).unwrap().collider_dirty);
    engine.update_terrain(&mut world);
    assert_eq!(
        world
            .chunk_positions()
            .iter()
            .filter(|&&pos| world.chunk(pos).unwrap().collider_dirty)
            .count(),
        1
    );
    engine.update_terrain(&mut world);
    assert!(world.chunk_positions().iter().all(|&pos| !world.chunk(pos).unwrap().collider_dirty));
}

#[test]
fn body_lands_and_falls_after_digging() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 14, 42, 50, 42, '#');
        let stone = world.materials().id("stone").unwrap();
        let mut engine = PhysicsEngine::new();
        let body = engine
            .create_body(
                4,
                4,
                vec![Cell { material: stone, ..Cell::default() }; 16],
                world.materials(),
                [32.0, 15.0],
            )
            .unwrap();
        let handle = engine.bodies[body].handle;
        engine.update_terrain(&mut world);
        assert!(!world.chunk(ChunkPos::new(0, 0)).unwrap().collider_dirty);
        for _ in 0..240 {
            engine.world.step();
            engine.update_terrain(&mut world);
        }
        let settled_y = engine.world.bodies[handle].translation().y * CELLS_PER_METER;
        assert!(
            (settled_y - 40.0).abs() < 2.0,
            "seed {seed}, body did not land: y={settled_y}\n{}",
            to_ascii(&world, 24, 36, 41, 46)
        );
        for x in 22..=42 {
            world.set(CellPos::new(x, 42), Cell::default());
        }
        assert!(world.chunk(ChunkPos::new(0, 0)).unwrap().collider_dirty);
        engine.update_terrain(&mut world);
        assert!(!world.chunk(ChunkPos::new(0, 0)).unwrap().collider_dirty);
        for _ in 0..180 {
            engine.world.step();
            engine.update_terrain(&mut world);
        }
        let fallen_y = engine.world.bodies[handle].translation().y * CELLS_PER_METER;
        assert!(
            fallen_y > 50.0,
            "seed {seed}, body did not fall after digging: y={fallen_y}, settled={settled_y}\n{}",
            to_ascii(&world, 20, 38, 45, 47)
        );
    }
}
