#![allow(clippy::unwrap_used)] // 各テストの前提違反はその場で失敗させる。

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, fill, material_of, world_from_ascii};
use charon_core::world::World;
use charon_physics::{ExplosionError, PhysicsEngine, explode};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "charon-body-heat-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("テスト用ディレクトリを作れません: {err}"),
            }
        }
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn heat_total(world: &World, physics: &PhysicsEngine) -> i64 {
    let foreground: i64 = world
        .chunk_positions()
        .into_iter()
        .filter_map(|pos| world.chunk(pos))
        .flat_map(|chunk| chunk.cells.iter())
        .filter(|cell| !cell.has(FLAG_BODY))
        .map(|cell| i64::from(cell.temp))
        .sum();
    foreground
        + physics
            .bodies
            .iter()
            .flat_map(|body| &body.cells)
            .map(|cell| i64::from(cell.temp))
            .sum::<i64>()
}

fn cell_total(world: &World, physics: &PhysicsEngine) -> usize {
    let foreground = world
        .chunk_positions()
        .into_iter()
        .filter_map(|pos| world.chunk(pos))
        .flat_map(|chunk| chunk.cells.iter())
        .filter(|cell| cell.material != MaterialId::EMPTY && !cell.has(FLAG_BODY))
        .count();
    let bodies = physics
        .bodies
        .iter()
        .flat_map(|body| &body.cells)
        .filter(|cell| cell.material != MaterialId::EMPTY)
        .count();
    foreground + bodies + physics.particles().len()
}

#[test]
fn body_heat_is_conserved() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let stone = material_of(&world, '#');
        fill(&mut world, 23, 20, 23, 23, '#');
        let cell = Cell { material: stone, temp: 1000, ..Cell::default() };
        let mut physics = PhysicsEngine::new();
        physics.create_body(4, 4, vec![cell; 16], world.materials(), [26.0, 22.0]).unwrap();
        physics.stamp_all(&mut world);
        let before = heat_total(&world, &physics);
        for tick in 0..16 {
            physics.exchange_body_heat(&mut world);
            assert_eq!(heat_total(&world, &physics), before, "seed={seed}, tick={tick}");
        }
        assert!(physics.bodies[0].cells.iter().any(|cell| cell.temp < 1000), "seed={seed}");
        assert!(world.get(CellPos::new(23, 20)).temp > 0, "seed={seed}");
    }
}

#[test]
fn body_heat_roundtrip() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let stone = material_of(&world, '#');
        let cell = Cell { material: stone, temp: 1010, ..Cell::default() };
        let mut physics = PhysicsEngine::new();
        physics.create_body(4, 4, vec![cell; 16], world.materials(), [26.0, 22.0]).unwrap();
        physics.stamp_all(&mut world);
        physics.exchange_body_heat(&mut world);
        let names: Vec<_> = (0..world.materials().len())
            .map(|index| world.materials().get(MaterialId(index as u16)).name.clone())
            .collect();
        let dir = TestDir::new();
        physics.save_bodies(&dir.0, world.materials(), &names).unwrap();
        let restored =
            PhysicsEngine::load_bodies(&dir.0, world.materials(), &names).unwrap().unwrap();
        assert_eq!(restored.bodies[0].cells, physics.bodies[0].cells, "seed={seed}");
    }
}

#[test]
fn body_heat_eventually_sleeps() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(32, 32), seed);
        let stone = material_of(&world, '#');
        fill(&mut world, 13, 14, 13, 17, '#');
        let cell = Cell { material: stone, temp: 300, ..Cell::default() };
        let mut physics = PhysicsEngine::new();
        physics.create_body(4, 4, vec![cell; 16], world.materials(), [16.0, 16.0]).unwrap();
        physics.stamp_all(&mut world);
        let mut slept = false;
        for _ in 0..5_000 {
            let previous: Vec<_> = physics.bodies[0].cells.iter().map(|cell| cell.temp).collect();
            physics.exchange_body_heat(&mut world);
            world.step();
            if world.awake_chunks() == 0
                && previous
                    == physics.bodies[0].cells.iter().map(|cell| cell.temp).collect::<Vec<_>>()
            {
                slept = true;
                break;
            }
        }
        assert!(slept, "seed={seed}: body heat never settled");
    }
}

#[test]
fn explosion_conserves_cells() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let wood = material_of(&world, 'W');
        fill(&mut world, 14, 20, 18, 24, 's');
        let cell = Cell { material: wood, ..Cell::default() };
        let mut physics = PhysicsEngine::new();
        physics.create_body(4, 4, vec![cell; 16], world.materials(), [32.0, 24.0]).unwrap();
        physics.stamp_all(&mut world);
        let before = cell_total(&world, &physics);
        assert_eq!(explode(&mut world, &mut physics, CellPos::new(16, 22), 3), Ok(()));
        assert_eq!(explode(&mut world, &mut physics, CellPos::new(31, 23), 1), Ok(()));
        assert_eq!(cell_total(&world, &physics), before, "seed={seed}, before split");
        physics.split_damaged(&mut world).unwrap();
        assert_eq!(cell_total(&world, &physics), before, "seed={seed}, after split");
        assert_eq!(
            explode(&mut world, &mut physics, CellPos::new(16, 22), 0),
            Err(ExplosionError::InvalidRadius)
        );
        assert_eq!(cell_total(&world, &physics), before, "seed={seed}, invalid radius");
    }
}

#[test]
fn explosion_damages_body() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let stone = material_of(&world, '#');
        let mut cells = vec![Cell::default(); 21 * 5];
        for y in 0..5 {
            for x in 0..21 {
                if !(8..13).contains(&x) || y == 2 {
                    cells[y * 21 + x].material = stone;
                }
            }
        }
        let mut physics = PhysicsEngine::new();
        physics.create_body(21, 5, cells, world.materials(), [32.0, 24.0]).unwrap();
        physics.stamp_all(&mut world);
        let center = CellPos::new(31, 24);
        assert!(world.get(center).has(FLAG_BODY), "seed={seed}");
        let before = cell_total(&world, &physics);
        explode(&mut world, &mut physics, center, 1).unwrap();
        assert_eq!(cell_total(&world, &physics), before, "seed={seed}, after explosion");
        physics.step(&mut world).unwrap();
        assert_eq!(physics.bodies.len(), 2, "seed={seed}");
        assert_eq!(cell_total(&world, &physics), before, "seed={seed}, after tick");
    }
}
