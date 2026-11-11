#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::{FLAG_FALLING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::rng::{SALT_FLOW, hash};
use charon_core::testkit::{assert_sleeps, box_art, count, fill, to_ascii, world_from_ascii};
use charon_physics::{PARTICLE_LIMIT, Particle, PhysicsEngine};

#[test]
fn thrown_particle_lands_without_tunneling() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 5, 20, 57, 20, '#');
        let pos = CellPos::new(10, 10);
        let sand = world.materials().id("sand").unwrap();
        let mut engine = PhysicsEngine::new();
        assert!(engine.spawn_particle(Particle {
            pos: [10.5, 10.5],
            vel: [0.0, 31.0],
            cell: world.make_cell(sand, pos),
        }));
        engine.update_particles(&mut world);
        assert!(engine.particles().is_empty(), "seed {seed}");
        let landed = world.get(CellPos::new(10, 19));
        assert_eq!(landed.material, sand, "seed {seed}\n{}", to_ascii(&world, 8, 17, 13, 22));
        assert!(landed.has(FLAG_FALLING));
        assert_eq!(count(&world, 's'), 1);
        assert_sleeps(&mut world, 400);
    }
}

#[test]
fn particles_conserve_cells_and_have_limit() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 1, 45, 62, 45, '#');
        let sand = world.materials().id("sand").unwrap();
        let mut engine = PhysicsEngine::new();
        for i in 0..24 {
            let x = 6 + i * 2;
            let pos = CellPos::new(x, 8);
            let vx = (hash(seed, 0, x, 8, SALT_FLOW) % 5) as f32 * 0.4 - 0.8;
            assert!(engine.spawn_particle(Particle {
                pos: [x as f32 + 0.5, 8.5],
                vel: [vx, 0.0],
                cell: world.make_cell(sand, pos),
            }));
        }
        for tick in 0..180 {
            world.step();
            engine.update_particles(&mut world);
            assert_eq!(
                count(&world, 's') + engine.particles().len(),
                24,
                "seed {seed}, tick {tick}"
            );
        }
        assert!(engine.particles().is_empty(), "seed {seed}: particles remained");
        assert_eq!(count(&world, 's'), 24, "seed {seed}");
        assert_sleeps(&mut world, 1000);
    }

    let mut engine = PhysicsEngine::new();
    let particle = Particle {
        pos: [0.5, 0.5],
        vel: [0.0, 0.0],
        cell: charon_core::cell::Cell { material: MaterialId(2), ..Default::default() },
    };
    for _ in 0..PARTICLE_LIMIT {
        assert!(engine.spawn_particle(particle));
    }
    assert!(!engine.spawn_particle(particle));
    assert_eq!(engine.particles().len(), PARTICLE_LIMIT);
}
