#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::Cell;
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, fill, place, to_ascii, world_from_ascii};
use charon_core::world::World;
use charon_physics::{PhysicsEngine, PixelBody};
use rapier2d::prelude::Vector;

fn scene(seed: u64) -> (World, PhysicsEngine) {
    let mut world = world_from_ascii(&box_art(62, 64), seed);
    fill(&mut world, 14, 41, 48, 58, 'w');
    let wood = world.materials().id("wood").unwrap();
    let mut cells = vec![Cell::default(); 13 * 9];
    for y in 0..9 {
        for x in 0..13 {
            if x == 0 || x == 12 || y == 8 {
                cells[y * 13 + x].material = wood;
            }
        }
    }
    let mut engine = PhysicsEngine::new();
    engine.create_body(13, 9, cells, world.materials(), [30.0, 24.0]).unwrap();
    engine.stamp_all(&mut world);
    let stamped = &engine.bodies[0].stamped;
    let left = stamped.iter().map(|p| p.x).min().unwrap();
    let right = stamped.iter().map(|p| p.x).max().unwrap();
    let bottom = stamped.iter().map(|p| p.y).max().unwrap();
    for y in bottom - 4..bottom {
        for x in left + 2..right - 1 {
            place(&mut world, x, y, 's');
        }
    }
    (world, engine)
}

fn body_state(engine: &PhysicsEngine) -> Vec<(Vec<Cell>, [u32; 5])> {
    engine
        .bodies
        .iter()
        .map(|body: &PixelBody| {
            let rigid = &engine.world.bodies[body.handle];
            let pos = rigid.translation();
            let vel = rigid.linvel();
            (
                body.cells.clone(),
                [
                    pos.x.to_bits(),
                    pos.y.to_bits(),
                    vel.x.to_bits(),
                    vel.y.to_bits(),
                    rigid.rotation().angle().to_bits(),
                ],
            )
        })
        .collect()
}

#[test]
fn deterministic() {
    for seed in 1..=8 {
        let (mut left, mut physics_left) = scene(seed);
        let (mut right, mut physics_right) = scene(seed);
        for tick in 0..150 {
            if tick == 12 {
                for physics in [&mut physics_left, &mut physics_right] {
                    let handle = physics.bodies[0].handle;
                    physics
                        .world
                        .bodies
                        .get_mut(handle)
                        .unwrap()
                        .apply_impulse(Vector::new(0.12, -0.03), true);
                }
            }
            if tick == 28 {
                assert!(physics_left.damage(0, 0, 1));
                assert!(physics_right.damage(0, 0, 1));
            }
            physics_left.step(&mut left).unwrap();
            physics_right.step(&mut right).unwrap();
            assert_eq!(left.tick(), tick + 1);
            assert_eq!(
                left.state_hash(),
                right.state_hash(),
                "seed {seed}, tick {tick}\n{}",
                to_ascii(&left, 15, 15, 48, 54)
            );
            assert_eq!(
                body_state(&physics_left),
                body_state(&physics_right),
                "seed {seed}, tick {tick}"
            );
            assert_eq!(
                physics_left.particles(),
                physics_right.particles(),
                "seed {seed}, tick {tick}"
            );
        }
    }
}

#[test]
fn sleeping_body_keeps_stamp() {
    let (mut world, mut engine) = scene(1);
    engine.update_terrain(&mut world);
    let handle = engine.bodies[0].handle;
    engine.world.bodies.get_mut(handle).unwrap().sleep();
    let stamped = engine.bodies[0].stamped.clone();
    let before: Vec<_> = stamped.iter().map(|&pos| world.get(pos)).collect();
    engine.step(&mut world).unwrap();
    assert_eq!(world.tick(), 1);
    assert_eq!(engine.bodies[0].stamped, stamped);
    assert_eq!(stamped.iter().map(|&pos| world.get(pos)).collect::<Vec<_>>(), before);
    assert!(
        stamped.iter().all(|&pos| world.get(pos).material == world.materials().id("wood").unwrap())
    );
    assert!(world.get(CellPos::new(30, 24)).material != charon_core::cell::MaterialId::BOUNDARY);
}
