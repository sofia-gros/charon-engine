#![allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。

use charon_core::cell::{Cell, MaterialId};
use charon_core::coords::CellPos;
use charon_core::testkit::{box_art, count, fill, place, to_ascii, world_from_ascii};
use charon_core::world::World;
use charon_physics::{CELLS_PER_METER, PhysicsEngine};
use rapier2d::prelude::Vector;

fn advance(engine: &mut PhysicsEngine, world: &mut World) {
    engine.apply_cell_forces(world);
    engine.unstamp_all(world);
    engine.world.step();
    engine.stamp_all(world);
    world.step();
    engine.update_particles(world);
    engine.update_terrain(world);
}

fn cup(width: usize, height: usize, material: MaterialId) -> Vec<Cell> {
    let mut cells = vec![Cell::default(); width * height];
    for y in 0..height {
        for x in 0..width {
            if x == 0 || x == width - 1 || y == height - 1 {
                cells[y * width + x].material = material;
            }
        }
    }
    cells
}

#[test]
fn stone_sinks_wood_floats() {
    for seed in 1..=8 {
        let mut final_heights = Vec::new();
        for material_name in ["wood", "stone"] {
            let mut world = world_from_ascii(&box_art(62, 64), seed);
            fill(&mut world, 2, 28, 61, 60, 'w');
            let material = world.materials().id(material_name).unwrap();
            let mut engine = PhysicsEngine::new();
            let index = engine
                .create_body(
                    4,
                    4,
                    vec![Cell { material, ..Cell::default() }; 16],
                    world.materials(),
                    [32.0, 22.0],
                )
                .unwrap();
            let handle = engine.bodies[index].handle;
            engine.stamp_all(&mut world);
            for _ in 0..600 {
                advance(&mut engine, &mut world);
            }
            let y = engine.world.bodies[handle].translation().y * CELLS_PER_METER;
            final_heights.push(y);
            if material_name == "wood" {
                assert!(
                    y < 34.0,
                    "seed {seed}: 木が水面へ浮かばなかった: y={y}\n{}",
                    to_ascii(&world, 24, 24, 41, 62)
                );
            }
        }
        assert!(
            final_heights[1] > 55.0,
            "seed {seed}: 石が底まで沈まなかった: y={:?}",
            final_heights
        );
    }
}

#[test]
fn boat_floats() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 2, 30, 61, 60, 'w');
        let water_before = count(&world, 'w');
        let wood = world.materials().id("wood").unwrap();
        let mut engine = PhysicsEngine::new();
        let mut hull = cup(18, 6, wood);
        for cell in &mut hull[..18] {
            cell.material = wood; // 空洞を閉じて浸水を防ぐ甲板。
        }
        let index = engine.create_body(18, 6, hull, world.materials(), [32.0, 26.0]).unwrap();
        let handle = engine.bodies[index].handle;
        engine.stamp_all(&mut world);
        for _ in 0..600 {
            advance(&mut engine, &mut world);
        }
        let y = engine.world.bodies[handle].translation().y * CELLS_PER_METER;
        let water = world.materials().id("water").unwrap();
        let stamped = &engine.bodies[index].stamped;
        let deck_in_air = stamped
            .iter()
            .any(|p| world.get(CellPos::new(p.x, p.y - 1)).material == MaterialId::EMPTY);
        let hull_in_water =
            stamped.iter().any(|p| world.get(CellPos::new(p.x, p.y + 1)).material == water);
        assert!(
            deck_in_air && hull_in_water && y < 50.0,
            "seed {seed}: 船が水面に浮かなかった: y={y}, air={deck_in_air}, water={hull_in_water}\n{}",
            to_ascii(&world, 19, 22, 46, 43)
        );
        assert_eq!(count(&world, 'w') + engine.particles().len(), water_before, "seed {seed}");
    }
}

#[test]
fn filled_cup_is_heavier() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        let wood = world.materials().id("wood").unwrap();
        let mut engine = PhysicsEngine::new();
        engine.world.gravity = Vector::ZERO; // 同じ支えで自重を相殺した天秤の状態。
        let empty =
            engine.create_body(9, 6, cup(9, 6, wood), world.materials(), [20.0, 16.0]).unwrap();
        let filled =
            engine.create_body(9, 6, cup(9, 6, wood), world.materials(), [43.0, 16.0]).unwrap();
        let empty_handle = engine.bodies[empty].handle;
        let filled_handle = engine.bodies[filled].handle;
        engine.stamp_all(&mut world);
        let stamped = &engine.bodies[filled].stamped;
        let left = stamped.iter().map(|p| p.x).min().unwrap();
        let right = stamped.iter().map(|p| p.x).max().unwrap();
        let bottom = stamped.iter().map(|p| p.y).max().unwrap();
        for y in bottom - 3..bottom {
            for x in left + 1..right {
                let pos = CellPos::new(x, y);
                assert_eq!(world.get(pos).material, MaterialId::EMPTY, "seed {seed}");
                place(&mut world, x, y, 'w');
            }
        }
        let water_before = count(&world, 'w');
        engine.apply_cell_forces(&world);
        engine.world.step();
        let empty_y = engine.world.bodies[empty_handle].translation().y;
        let filled_y = engine.world.bodies[filled_handle].translation().y;
        assert!(
            filled_y > empty_y + 0.001,
            "seed {seed}: 水入りのコップが下がらない: empty={empty_y}, filled={filled_y}\n{}",
            to_ascii(&world, 12, 10, 52, 23)
        );
        assert_eq!(count(&world, 'w'), water_before, "seed {seed}");
    }
}

#[test]
fn liquid_surface_crosses_under_body() {
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 2, 28, 61, 60, 'w');
        let wood = world.materials().id("wood").unwrap();
        let mut engine = PhysicsEngine::new();
        let index = engine
            .create_body(
                4,
                4,
                vec![Cell { material: wood, ..Cell::default() }; 16],
                world.materials(),
                [32.0, 40.0],
            )
            .unwrap();
        let handle = engine.bodies[index].handle;
        engine.stamp_all(&mut world);
        engine.apply_cell_forces(&world);
        engine.world.step();
        let vy = engine.world.bodies[handle].linvel().y;
        assert!(vy < 0.0, "seed {seed}: 水没した木の底面に浮力がない: vy={vy}");
    }
}
