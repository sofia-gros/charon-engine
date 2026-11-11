#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use std::collections::{BTreeSet, VecDeque};

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{assert_sleeps, box_art, place, to_ascii, world_from_ascii};
use charon_core::world::World;
use charon_physics::PhysicsEngine;
use rapier2d::prelude::Rotation;

fn ring(size: usize, material: MaterialId) -> Vec<Cell> {
    let mut cells = vec![Cell::default(); size * size];
    for y in 0..size {
        for x in 0..size {
            if x == 0 || y == 0 || x == size - 1 || y == size - 1 {
                cells[y * size + x].material = material;
            }
        }
    }
    cells
}

fn inside_reaches_outside(world: &World) -> bool {
    let mut queue = VecDeque::from([CellPos::new(32, 32)]);
    let mut visited = BTreeSet::new();
    while let Some(pos) = queue.pop_front() {
        if !visited.insert(pos) || world.get(pos).material != MaterialId::EMPTY {
            continue;
        }
        if pos == CellPos::new(10, 10) {
            return true;
        }
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            queue.push_back(CellPos::new(pos.x + dx, pos.y + dy));
        }
    }
    false
}

#[test]
fn stamp_has_no_holes() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let material = world.materials().id("stone").unwrap();
        let mut engine = PhysicsEngine::new();
        let index = engine
            .create_body(19, 19, ring(19, material), world.materials(), [32.5, 32.5])
            .unwrap();
        let handle = engine.bodies[index].handle;
        for angle in 0..=360 {
            engine
                .world
                .bodies
                .get_mut(handle)
                .unwrap()
                .set_rotation(Rotation::new((angle as f32).to_radians()), true);
            engine.stamp_all(&mut world);
            assert!(!engine.bodies[index].stamped.is_empty(), "seed {seed}, angle {angle}");
            assert!(
                !inside_reaches_outside(&world),
                "seed {seed}, angle {angle}: 内側が外側とつながる\n{}",
                to_ascii(&world, 18, 18, 47, 47)
            );
            engine.unstamp_all(&mut world);
        }
        assert_sleeps(&mut world, 200);
    }
}

#[test]
fn stamp_unstamp_restores_world() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        place(&mut world, 24, 24, 'd');
        let original = world.state_hash();
        let material = world.materials().id("wood").unwrap();
        let mut engine = PhysicsEngine::new();
        engine.create_body(19, 19, ring(19, material), world.materials(), [32.5, 32.5]).unwrap();
        engine.stamp_all(&mut world);
        assert_eq!(world.get(CellPos::new(24, 24)).material, world.materials().id("dirt").unwrap());
        assert!(engine.bodies[0].stamped.iter().all(|&pos| world.get(pos).has(FLAG_BODY)));
        engine.unstamp_all(&mut world);
        assert!(engine.bodies[0].stamped.is_empty());
        assert_eq!(world.state_hash(), original, "seed {seed}");
        assert_sleeps(&mut world, 200);
    }
}
