//! 仕様 02 §9.5: 更新範囲の外 (コールドフリーズ) でも、液体は大雑把に下へ落ち続ける。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;

/// 縦に 6 チャンク (y = 0..384) の世界。一番下の行 (y = 383) は石の床。
fn tall_world() -> World {
    let mut world = World::new(Arc::new(MaterialTable::builtin().unwrap()), 1);
    for cy in 0..6 {
        world.insert_chunk(ChunkPos::new(0, cy));
    }
    testkit::fill(&mut world, 0, 383, 63, 383, '#');
    world
}

fn water_rows(world: &World) -> Vec<i32> {
    let water = testkit::material_of(world, 'w');
    (0..384).filter(|&y| (0..64).any(|x| world.get(CellPos::new(x, y)).material == water)).collect()
}

/// 範囲の外の水は、チャンクをまたいで床まで落ちる。数は変わらない。範囲の中の世界は、これまでどおり動く。
#[test]
fn liquids_keep_falling_outside_the_active_region() {
    let mut world = tall_world();
    // 更新範囲は、いちばん下のチャンクだけ。上の 5 チャンクは外 (コールドフリーズ)。
    world.set_active_region(Some((ChunkPos::new(0, 5), 0)));
    for y in 5..9 {
        testkit::place(&mut world, 10, y, 'w');
    }
    let before = testkit::count(&world, 'w');
    for _ in 0..600 {
        world.step();
    }
    assert_eq!(testkit::count(&world, 'w'), before, "セルの数は変わらない");
    let rows = water_rows(&world);
    assert!(rows.iter().all(|&y| y >= 320), "外のチャンクを抜けて、範囲の中まで落ちた: {rows:?}");
    assert!(rows.iter().all(|&y| y >= 380), "床の上に積もる: {rows:?}");
    // 床は石のまま。
    assert_eq!(world.get(CellPos::new(10, 383)).material, testkit::material_of(&world, '#'));
}

/// 固体の上では、落ちない。更新範囲の外は、広がらない (落下だけ)。
#[test]
fn coarse_flow_only_falls_and_stops_on_solids() {
    let mut world = tall_world();
    world.set_active_region(Some((ChunkPos::new(0, 5), 0)));
    testkit::fill(&mut world, 0, 100, 63, 100, '#');
    testkit::place(&mut world, 10, 90, 'w');
    for _ in 0..200 {
        world.step();
    }
    assert_eq!(water_rows(&world), vec![99], "棚の上で止まる (横へ広がらない)");
}

/// 同じ入力なら同じ結果。
#[test]
fn coarse_flow_is_deterministic() {
    let run = || {
        let mut world = tall_world();
        world.set_active_region(Some((ChunkPos::new(0, 5), 0)));
        for x in 0..20 {
            for y in 3..12 {
                testkit::place(&mut world, x, y, 'w');
            }
        }
        for _ in 0..300 {
            world.step();
        }
        world.state_hash()
    };
    assert_eq!(run(), run());
}

/// 範囲を決めていないとき (すべて更新) は、この処理は働かない。
#[test]
fn without_an_active_region_the_normal_simulation_runs() {
    let mut world = tall_world();
    testkit::place(&mut world, 10, 5, 'w');
    for _ in 0..600 {
        world.step();
    }
    assert!(water_rows(&world).iter().all(|&y| y >= 378));
}
