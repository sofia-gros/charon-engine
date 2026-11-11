//! 熱の場面テスト (仕様 03 §2)。

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::{MaterialError, MaterialTable};
use charon_core::testkit::*;
use charon_core::world::World;

fn total_temp(world: &World) -> i64 {
    world
        .chunk_positions()
        .into_iter()
        .filter_map(|pos| world.chunk(pos))
        .flat_map(|chunk| chunk.cells.iter())
        .map(|cell| i64::from(cell.temp))
        .sum()
}

#[test]
fn heat_is_conserved() {
    for seed in 1..=8 {
        // 断熱境界 (読み込まれていない BOUNDARY) の内側に熱い石を配置する。
        let mut world = world_from_ascii("###\n###\n###", seed);
        let p = CellPos::new(1, 1);
        let mut cell = world.get(p);
        cell.temp = 1_000;
        world.set(p, cell);
        let before = total_temp(&world);
        for _ in 0..1_000 {
            world.step();
        }
        assert_eq!(total_temp(&world), before, "seed {seed}");
        assert_sleeps(&mut world, 2_000);
    }
}

#[test]
fn water_boils_and_condenses() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("#####\n#...#\n#.w.#\n#####", seed);
        let p = CellPos::new(2, 2);
        let mut water = world.get(p);
        water.temp = 200;
        world.set(p, water);
        for _ in 0..3 {
            world.step();
        }
        assert_eq!(count(&world, 't'), 1, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 4));
        let steam = (1..=2)
            .flat_map(|y| (1..=3).map(move |x| CellPos::new(x, y)))
            .find(|&p| world.get(p).material == material_of(&world, 't'))
            .expect("蒸気が見つかる");
        let mut cell = world.get(steam);
        cell.temp = 20;
        world.set(steam, cell);
        for _ in 0..3 {
            world.step();
        }
        assert_eq!(count(&world, 'w'), 1, "seed {seed}: {}", to_ascii(&world, 0, 0, 5, 4));
        assert_sleeps(&mut world, 2_000);
    }
}

#[test]
fn phase_loop_is_rejected() {
    let src = r#"[
        (name: "a", phase: Liquid, color: (0,0,0,0), boil: Some((at: 100, into: "b"))),
        (name: "b", phase: Gas, color: (0,0,0,0), freeze: Some((at: 100, into: "a"))),
    ]"#;
    assert!(matches!(MaterialTable::from_ron(src), Err(MaterialError::Invalid { .. })));
    assert!(MaterialTable::builtin().is_ok());
}

#[test]
fn heat_flows_hot_to_cold() {
    for seed in 1..=8 {
        let mut world = world_from_ascii("##", seed);
        let (hot, cold) = (CellPos::new(0, 0), CellPos::new(1, 0));
        let mut cell = world.get(hot);
        cell.temp = 800;
        world.set(hot, cell);
        let before = world.get(hot).temp - world.get(cold).temp;
        for _ in 0..20 {
            world.step();
        }
        let after = world.get(hot).temp - world.get(cold).temp;
        assert!(after < before, "seed {seed}: {before} -> {after}");
        assert!(world.chunk(ChunkPos::new(0, 0)).is_some());
        assert_sleeps(&mut world, 2_000);
    }
}
