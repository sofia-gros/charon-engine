//! 仕様 10 §16: 地形の崩落。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::collapse::{Collapse, MAX_CLUSTER};

/// 64×64 の世界。床は y = 60 (左右は読み込まれていない場所に接する = 境界)。
/// 石の柱 (x = 22, y 34..=59) の上に、石の板 (x 20..=24, y 30..=33) が乗っている。
fn scene() -> World {
    let materials = MaterialTable::builtin().unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 60, 63, 60, '#');
    testkit::fill(&mut world, 22, 34, 22, 59, '#');
    testkit::fill(&mut world, 20, 30, 24, 33, '#');
    world
}

fn cut(world: &mut World, x: i32, y0: i32, y1: i32) -> Vec<CellPos> {
    let mut cells = Vec::new();
    for y in y0..=y1 {
        let pos = CellPos::new(x, y);
        world.set(pos, world.make_cell(MaterialId::EMPTY, pos));
        cells.push(pos);
    }
    cells
}

fn run(collapse: &mut Collapse, world: &mut World, steps: usize) {
    for _ in 0..steps {
        collapse.step(world);
        world.step();
    }
}

fn solid_count(world: &World) -> usize {
    testkit::count(world, '#')
}

/// 支えを失った板と柱の上半分は、ひとかたまりで落ちて、下の柱の上で止まる。セルの数は変わらない。
#[test]
fn unsupported_cluster_falls_as_one_piece_and_lands() {
    let mut world = scene();
    let mut collapse = Collapse::new();
    let gap = cut(&mut world, 22, 41, 45);
    let before = solid_count(&world);
    collapse.notice(&world, &gap);
    assert_eq!(collapse.falling(), 1, "板と上の柱が 1 つのかたまり");
    assert_eq!(collapse.falling_cells(), 5 * 4 + 7);

    run(&mut collapse, &mut world, 12);
    assert_eq!(collapse.falling(), 0, "着いたら止まる");
    assert_eq!(solid_count(&world), before, "セルは増えも減りもしない");
    // 5 セル落ちた: 板の上端 30 → 35、柱は下の柱に着く。
    let stone = testkit::material_of(&world, '#');
    assert_eq!(world.get(CellPos::new(20, 35)).material, stone);
    assert_eq!(world.get(CellPos::new(20, 30)).material, MaterialId::EMPTY);
    assert_eq!(world.get(CellPos::new(22, 45)).material, stone);
}

/// 地面とつながっているものは落ちない: 柱の下の方を削っても、床につながる部分は動かない。
#[test]
fn supported_terrain_stays() {
    let mut world = scene();
    let mut collapse = Collapse::new();
    let gap = cut(&mut world, 22, 50, 50);
    collapse.notice(&world, &gap);
    // 上の部分は落ちる (板と柱の上) が、床につながる下の柱は落ちない。
    assert_eq!(collapse.falling(), 1);
    assert!(collapse.falling_cells() < 40, "落ちるのは上の部分だけ: {}", collapse.falling_cells());
    let mut world = scene();
    let mut collapse = Collapse::new();
    // 床の近くを削っても、上は床の柱とつながったまま。
    let gap = cut(&mut world, 21, 59, 59);
    collapse.notice(&world, &gap);
    assert_eq!(collapse.falling(), 0, "何もない場所を削っても、つながっていれば落ちない");
}

/// 読み込まれていない場所 (境界) に触れるかたまりは、支えがあるとみなして落とさない。
#[test]
fn clusters_touching_the_boundary_are_supported() {
    let mut world = scene();
    testkit::fill(&mut world, 0, 10, 3, 14, '#');
    let mut collapse = Collapse::new();
    collapse.notice(&world, &[CellPos::new(4, 12)]);
    assert_eq!(collapse.falling(), 0);
}

/// 大きなかたまりは地面とつながっているとみなして落とさない。
#[test]
fn huge_clusters_are_treated_as_ground() {
    let mut world = scene();
    testkit::fill(&mut world, 5, 5, 5 + 30, 5 + 20, '#');
    assert!((31 * 21) > MAX_CLUSTER);
    let mut collapse = Collapse::new();
    collapse.notice(&world, &[CellPos::new(4, 10)]);
    assert_eq!(collapse.falling(), 0);
}

/// 水の中へ落ちると、水は押しのけられて数は変わらない。下が砂なら落ちない。
#[test]
fn falling_displaces_liquid_and_stops_on_powder() {
    let mut world = scene();
    testkit::fill(&mut world, 10, 52, 40, 59, 'w');
    // 柱の水のところは石に戻す (柱は (22, y) を通る)。
    testkit::fill(&mut world, 22, 52, 22, 59, '#');
    let mut collapse = Collapse::new();
    let gap = cut(&mut world, 22, 41, 45);
    let (stone_before, water_before) = (solid_count(&world), testkit::count(&world, 'w'));
    collapse.notice(&world, &gap);
    run(&mut collapse, &mut world, 12);
    assert_eq!(solid_count(&world), stone_before);
    assert_eq!(testkit::count(&world, 'w'), water_before, "水の数は変わらない");

    // 砂の山の上に落ちるかたまりは、砂を押しのけずに止まる。
    let mut world = scene();
    // 石の棚 (柱とつながる) の上に砂を敷く。砂は棚の上で動かない。
    testkit::fill(&mut world, 18, 44, 26, 44, '#');
    testkit::fill(&mut world, 18, 42, 26, 43, 's');
    testkit::run_until_sleep(&mut world, 200);
    let mut collapse = Collapse::new();
    let gap = cut(&mut world, 22, 38, 41);
    let sand = testkit::count(&world, 's');
    collapse.notice(&world, &gap);
    run(&mut collapse, &mut world, 6);
    assert_eq!(testkit::count(&world, 's'), sand);
    assert_eq!(collapse.falling(), 0);
}

/// 落ちている途中のセルが掘られたら、そのかたまりは追わない (落ちるのをやめる)。
#[test]
fn a_cluster_that_is_dug_stops_being_followed() {
    let mut world = scene();
    let mut collapse = Collapse::new();
    let gap = cut(&mut world, 22, 41, 45);
    collapse.notice(&world, &gap);
    run(&mut collapse, &mut world, 2);
    cut(&mut world, 21, 31, 31);
    cut(&mut world, 22, 33, 33);
    run(&mut collapse, &mut world, 1);
    assert_eq!(collapse.falling(), 0);
}
