#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::{Cell, FLAG_BODY};
use charon_core::testkit::{assert_sleeps, box_art, to_ascii, world_from_ascii};
use charon_physics::{CELL_MASS_SCALE, CELLS_PER_METER, PhysicsEngine};
use rapier2d::prelude::{Rotation, Vector};

#[test]
fn body_splits_when_cut() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let stone = world.materials().id("stone").unwrap();
        let mut cells = vec![Cell::default(); 21 * 5];
        for y in 0..5 {
            for x in 0..21 {
                if !(8..13).contains(&x) || y == 2 {
                    cells[y * 21 + x].material = stone;
                }
            }
        }
        let mut engine = PhysicsEngine::new();
        let index = engine.create_body(21, 5, cells, world.materials(), [32.0, 24.0]).unwrap();
        let handle = engine.bodies[index].handle;
        let rotation = Rotation::new(0.35);
        let velocity = Vector::new(1.5, -0.75);
        engine.world.bodies.get_mut(handle).unwrap().set_rotation(rotation, true);
        engine.world.bodies.get_mut(handle).unwrap().set_linvel(velocity, true);
        engine.world.bodies.get_mut(handle).unwrap().set_angvel(0.6, true);
        engine.stamp_all(&mut world);
        let old_stamp = engine.bodies[index].stamped.clone();
        let old_origin = engine.bodies[index].origin;
        assert!(engine.damage(index, 10, 2));
        assert!(!engine.damage(index, 10, 2));
        engine.split_damaged(&mut world).unwrap();
        assert_eq!(engine.bodies.len(), 2, "seed {seed}");
        assert!(old_stamp.iter().all(|&pos| !world.get(pos).has(FLAG_BODY)), "seed {seed}");
        let mut centers = Vec::new();
        let mut total_cells = 0;
        for part in &engine.bodies {
            assert_ne!(part.handle, handle, "seed {seed}");
            let count = part.cells.iter().filter(|c| c.material == stone).count();
            assert_eq!(count, 42, "seed {seed}");
            total_cells += count;
            let rigid = &engine.world.bodies[part.handle];
            assert!((rigid.rotation().angle() - rotation.angle()).abs() < 0.0001);
            assert_eq!(rigid.linvel(), velocity);
            assert!((rigid.angvel() - 0.6).abs() < 0.0001);
            assert!((rigid.mass() - 42.0 * 2600.0 * CELL_MASS_SCALE).abs() < 0.0001);
            let dx = part.origin[0] - old_origin[0];
            let dy = part.origin[1] - old_origin[1];
            let expected = [
                32.0 + rotation.angle().cos() * dx - rotation.angle().sin() * dy,
                24.0 + rotation.angle().sin() * dx + rotation.angle().cos() * dy,
            ];
            let actual = rigid.translation() * CELLS_PER_METER;
            assert!(
                (actual.x - expected[0]).abs() < 0.001 && (actual.y - expected[1]).abs() < 0.001,
                "seed {seed}: 重心がずれた: {actual:?} != {expected:?}"
            );
            centers.push(actual.x);
        }
        assert_eq!(total_cells + engine.particles().len(), 84);
        assert!(centers[0] < 32.0 && centers[1] > 32.0, "seed {seed}: {centers:?}");
        engine.stamp_all(&mut world);
        assert!(
            engine.bodies.iter().all(|b| !b.stamped.is_empty()),
            "seed {seed}\n{}",
            to_ascii(&world, 17, 15, 48, 33)
        );
        engine.unstamp_all(&mut world);
        assert_sleeps(&mut world, 2000);
    }
}

#[test]
fn damaged_fragment_becomes_particles() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let wood = world.materials().id("wood").unwrap();
        let mut cells = vec![Cell::default(); 6 * 3];
        for y in 0..3 {
            for x in 0..6 {
                if x != 3 || y == 1 {
                    cells[y * 6 + x].material = wood;
                }
            }
        }
        let mut engine = PhysicsEngine::new();
        let index = engine.create_body(6, 3, cells, world.materials(), [30.0, 20.0]).unwrap();
        engine.stamp_all(&mut world);
        assert!(engine.damage(index, 3, 1));
        engine.split_damaged(&mut world).unwrap();
        assert_eq!(engine.bodies.len(), 1, "seed {seed}");
        assert_eq!(engine.bodies[0].cells.iter().filter(|c| c.material == wood).count(), 9);
        assert_eq!(engine.particles().len(), 6, "seed {seed}");
        assert!(
            engine.particles().iter().all(|p| p.cell.material == wood && !p.cell.has(FLAG_BODY))
        );
        engine.stamp_all(&mut world);
        engine.unstamp_all(&mut world);
        assert_sleeps(&mut world, 2000);
    }
}

#[test]
fn damage_rebuilds_single_component() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let stone = world.materials().id("stone").unwrap();
        let cell = Cell { material: stone, ..Cell::default() };
        let mut engine = PhysicsEngine::new();
        let index =
            engine.create_body(4, 4, vec![cell; 16], world.materials(), [32.0, 24.0]).unwrap();
        let old = engine.bodies[index].handle;
        engine.stamp_all(&mut world);
        assert!(!engine.damage(index, 4, 0));
        assert!(engine.damage(index, 0, 0));
        engine.split_damaged(&mut world).unwrap();
        assert_eq!(engine.bodies.len(), 1, "seed {seed}");
        assert_ne!(engine.bodies[0].handle, old);
        assert!(
            (engine.world.bodies[engine.bodies[0].handle].mass() - 15.0 * 2600.0 * CELL_MASS_SCALE)
                .abs()
                < 0.0001
        );
        assert!(engine.particles().is_empty());
        engine.stamp_all(&mut world);
        engine.unstamp_all(&mut world);
        assert_sleeps(&mut world, 2000);
    }
}
