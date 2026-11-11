#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::{Cell, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{assert_sleeps, box_art, count, place, to_ascii, world_from_ascii};
use charon_core::world::World;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use rapier2d::prelude::{Rotation, Vector};

fn filled_c(seed: u64, contents: char) -> (World, PhysicsEngine, (i32, i32, i32, i32)) {
    let mut world = world_from_ascii(&box_art(190, 96), seed);
    let wood = world.materials().id("wood").unwrap();
    let (width, height) = (21, 15);
    let mut cells = vec![Cell::default(); width * height];
    for y in 0..height {
        for x in 0..width {
            if x == 0 || x == width - 1 || y == height - 1 {
                cells[y * width + x].material = wood;
            }
        }
    }
    let mut engine = PhysicsEngine::new();
    engine
        .create_body(width as u16, height as u16, cells, world.materials(), [32.0, 32.0])
        .unwrap();
    engine.stamp_all(&mut world);
    let stamped = &engine.bodies[0].stamped;
    let bounds = (
        stamped.iter().map(|pos| pos.x).min().unwrap(),
        stamped.iter().map(|pos| pos.y).min().unwrap(),
        stamped.iter().map(|pos| pos.x).max().unwrap(),
        stamped.iter().map(|pos| pos.y).max().unwrap(),
    );
    let (left, top, right, bottom) = bounds;
    let mut added = 0;
    for y in ((top + 2)..bottom).rev() {
        for x in (left + 2)..(right - 1) {
            if world.get(CellPos::new(x, y)).material == MaterialId::EMPTY {
                place(&mut world, x, y, contents);
                added += 1;
                if added == 200 {
                    return (world, engine, bounds);
                }
            }
        }
    }
    panic!("C 字の内側に 200 セルの空間がない: {bounds:?}, {added}");
}

fn cell_positions(world: &World, material: MaterialId) -> Vec<CellPos> {
    let mut positions = Vec::new();
    for chunk_pos in world.chunk_positions() {
        let chunk = world.chunk(chunk_pos).unwrap();
        let origin = chunk_pos.origin();
        for (i, cell) in chunk.cells.iter().enumerate() {
            if cell.material == material {
                positions.push(CellPos::new(origin.x + i as i32 % 64, origin.y + i as i32 / 64));
            }
        }
    }
    positions
}

#[test]
fn displace_conserves_mass() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        for y in 18..=36 {
            for x in 14..=48 {
                place(&mut world, x, y, 's');
            }
        }
        let original = count(&world, 's');
        let stone = world.materials().id("stone").unwrap();
        let mut engine = PhysicsEngine::new();
        let index = engine
            .create_body(
                5,
                5,
                vec![Cell { material: stone, ..Cell::default() }; 25],
                world.materials(),
                [20.5, 28.5],
            )
            .unwrap();
        let handle = engine.bodies[index].handle;
        for tick in 0..40 {
            engine.unstamp_all(&mut world);
            engine.world.bodies.get_mut(handle).unwrap().set_translation(
                Vector::new((20.5 + tick as f32) / CELLS_PER_METER, 28.5 / CELLS_PER_METER),
                true,
            );
            engine.world.bodies.get_mut(handle).unwrap().set_linvel(Vector::new(3.75, 0.0), true);
            engine.stamp_all(&mut world);
            world.step();
            engine.update_particles(&mut world);
            assert_eq!(
                count(&world, 's') + engine.particles().len(),
                original,
                "seed {seed}, tick {tick}"
            );
        }
        engine.unstamp_all(&mut world);
        assert_sleeps(&mut world, 2000);
    }
}

#[test]
fn c_shape_holds_contents() {
    for seed in 1..=8 {
        let (mut world, engine, (left, top, right, bottom)) = filled_c(seed, 's');
        for _ in 0..300 {
            world.step();
        }
        let sand = world.materials().id("sand").unwrap();
        let positions = cell_positions(&world, sand);
        assert_eq!(positions.len(), 200, "seed {seed}");
        assert!(
            positions
                .iter()
                .all(|pos| pos.x > left && pos.x < right && pos.y > top && pos.y < bottom),
            "seed {seed}: C から砂が漏れた\n{}",
            to_ascii(&world, left - 3, top - 3, right + 4, bottom + 5)
        );
        assert_eq!(engine.bodies[0].stamped.len(), 2 * 14 + 21);
    }
}

fn pours_when_rotated(seed: u64, contents: char) {
    let (mut world, mut engine, _) = filled_c(seed, contents);
    for _ in 0..300 {
        world.step();
    }
    let handle = engine.bodies[0].handle;
    for tick in 1..=90 {
        engine.unstamp_all(&mut world);
        engine
            .world
            .bodies
            .get_mut(handle)
            .unwrap()
            .set_rotation(Rotation::new(std::f32::consts::PI * tick as f32 / 90.0), true);
        engine.stamp_all(&mut world);
        world.step();
        engine.update_particles(&mut world);
    }
    let lower_edge = engine.bodies[0].stamped.iter().map(|pos| pos.y).max().unwrap();
    for _ in 0..600 {
        world.step();
        engine.update_particles(&mut world);
    }
    let material = if contents == 's' { "sand" } else { "water" };
    let id = world.materials().id(material).unwrap();
    let positions = cell_positions(&world, id);
    assert_eq!(positions.len() + engine.particles().len(), 200, "seed {seed}, {contents}");
    assert!(
        positions.iter().all(|pos| pos.y > lower_edge),
        "seed {seed}, {contents}: 開口部から流れなかった\n{}",
        to_ascii(&world, 17, 17, 48, 59)
    );
}

#[test]
fn c_shape_pours_when_rotated() {
    for seed in 1..=8 {
        pours_when_rotated(seed, 's');
    }
}

#[test]
fn c_shape_pours_water() {
    for seed in 1..=8 {
        pours_when_rotated(seed, 'w');
    }
}

#[test]
fn carried_contents_move_with_body() {
    for seed in 1..=8 {
        let (mut world, mut engine, _) = filled_c(seed, 's');
        for _ in 0..300 {
            world.step();
        }
        let sand = world.materials().id("sand").unwrap();
        let before: i32 = cell_positions(&world, sand).iter().map(|pos| pos.x).sum();
        let handle = engine.bodies[0].handle;
        for tick in 1..=500 {
            engine.unstamp_all(&mut world);
            engine.world.bodies.get_mut(handle).unwrap().set_translation(
                Vector::new((32.0 + tick as f32 * 0.2) / CELLS_PER_METER, 32.0 / CELLS_PER_METER),
                true,
            );
            engine.world.bodies.get_mut(handle).unwrap().set_linvel(Vector::new(0.75, 0.0), true);
            engine.stamp_all(&mut world);
            world.step();
            engine.update_particles(&mut world);
        }
        let after: i32 = cell_positions(&world, sand).iter().map(|pos| pos.x).sum();
        assert_eq!(count(&world, 's') + engine.particles().len(), 200, "seed {seed}");
        assert!(
            (after - before) as f32 / 200.0 >= 80.0,
            "seed {seed}: 砂の重心移動 {} セル\n{}",
            (after - before) as f32 / 200.0,
            to_ascii(&world, 20, 17, 160, 50)
        );
    }
}
