//! 仕様 26 §2: 生成順に依存しない資源と、掘削後の安全な採集経路。
#![allow(clippy::unwrap_used)] // 場面テストの前提が崩れたら直ちに失敗させる。

use std::collections::VecDeque;
use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_world::generator::ChunkGenerator;
use charon_world::terrain::{
    AdventureGenerator, AdventureMaterials, BOTTOM, SURFACE_BASE, surface_y,
};

fn generator() -> (AdventureGenerator, MaterialTable) {
    let table = MaterialTable::builtin().unwrap();
    let materials = AdventureMaterials::from_table(&table).unwrap();
    (AdventureGenerator { materials }, table)
}

/// 石炭・鉄・金を含むチャンクを逆順に生成しても、境界のセルと資源の個数は等しい。
#[test]
fn world_resources_are_order_independent_across_chunks() {
    let (g, table) = generator();
    let positions: Vec<_> = [3, 4, 5, 9, 11, 12, 13, 14, 16]
        .into_iter()
        .flat_map(|cy| (0..4).map(move |cx| ChunkPos::new(cx, cy)))
        .collect();
    for seed in 1..=8 {
        let build = |reverse: bool| {
            let mut world = World::new(Arc::new(table.clone()), seed);
            let mut ordered = positions.clone();
            if reverse {
                ordered.reverse();
            }
            for pos in ordered {
                world.insert_loaded_chunk(pos, g.generate(seed, pos, &table));
            }
            world
        };
        let normal = build(false);
        let reverse = build(true);
        assert_eq!(normal.state_hash(), reverse.state_hash(), "seed {seed}: 生成順");
        for &pos in &positions {
            assert_eq!(normal.chunk(pos).unwrap().cells, reverse.chunk(pos).unwrap().cells);
            let origin = pos.origin();
            for lx in [0, 63] {
                for ly in [0, 63] {
                    let at = CellPos::new(origin.x + lx, origin.y + ly);
                    let expected = g.cell_at(seed, at.x, at.y);
                    assert_eq!(
                        normal.get(at).material,
                        expected.map_or(MaterialId::EMPTY, |c| c.material),
                        "seed {seed}: {at:?}"
                    );
                }
            }
        }
        let counts = [g.materials.coal, g.materials.iron, g.materials.gold].map(|id| {
            positions
                .iter()
                .map(|&pos| {
                    normal.chunk(pos).unwrap().cells.iter().filter(|c| c.material == id).count()
                })
                .sum::<usize>()
        });
        println!("seed {seed}: coal={}, iron={}, gold={}", counts[0], counts[1], counts[2]);
        assert!(counts.into_iter().all(|count| count > 0), "seed {seed}: {counts:?}");
        assert_eq!(testkit::count(&normal, 'W'), testkit::count(&reverse, 'W'));
        assert!((-24..=24).all(|x| surface_y(seed, x) == SURFACE_BASE));
    }
}

const X0: i32 = -64;
const X1: i32 = 320;
const Y0: i32 = 128;
const Y1: i32 = 1200;
const _: () = assert!(Y1 < BOTTOM);
const WIDTH: usize = (X1 - X0) as usize;
const HEIGHT: usize = (Y1 - Y0) as usize;

/// 面積和で、16×24 セルの掘削通路が流体と境界を避けるかを検査する。
struct Survey {
    hazards: Vec<u32>,
    woods: Vec<u32>,
    coals: Vec<u32>,
    golds: Vec<u32>,
    counts: [u32; 4],
}

impl Survey {
    fn new(seed: u64, g: &AdventureGenerator) -> Self {
        let len = (WIDTH + 1) * (HEIGHT + 1);
        let mut survey = Self {
            hazards: vec![0; len],
            woods: vec![0; len],
            coals: vec![0; len],
            golds: vec![0; len],
            counts: [0; 4],
        };
        let stride = WIDTH + 1;
        for y in Y0..Y1 {
            let mut running = [0; 4];
            for x in X0..X1 {
                let material = g.cell_at(seed, x, y);
                let values = [
                    material.is_some_and(|c| {
                        [g.materials.water, g.materials.lava, MaterialId::BOUNDARY]
                            .contains(&c.material)
                    }),
                    material.is_some_and(|c| {
                        c.material == g.materials.wood
                            && !c.block
                            && x >= 48
                            && y < surface_y(seed, x)
                    }),
                    material.is_some_and(|c| {
                        c.material == g.materials.coal
                            && (40..=320).contains(&(y - surface_y(seed, x)))
                    }),
                    material.is_some_and(|c| {
                        c.material == g.materials.gold && y - surface_y(seed, x) >= 360
                    }),
                ];
                let i = (y - Y0 + 1) as usize * stride + (x - X0 + 1) as usize;
                let above = i - stride;
                for (n, value) in values.into_iter().enumerate() {
                    running[n] += u32::from(value);
                    survey.counts[n] += u32::from(value);
                    let grid = match n {
                        0 => &mut survey.hazards,
                        1 => &mut survey.woods,
                        2 => &mut survey.coals,
                        _ => &mut survey.golds,
                    };
                    grid[i] = grid[above] + running[n];
                }
            }
        }
        survey
    }

    fn area(&self, grid: &[u32], x: i32, y: i32) -> u32 {
        let (left, right, top, bottom) =
            ((x - X0) as usize, (x + 16 - X0) as usize, (y - Y0) as usize, (y + 24 - Y0) as usize);
        let stride = WIDTH + 1;
        grid[bottom * stride + right] + grid[top * stride + left]
            - grid[top * stride + right]
            - grid[bottom * stride + left]
    }

    fn routes(&self, start: [i32; 2]) -> [bool; 3] {
        assert!((X0..=X1 - 16).contains(&start[0]));
        assert!((Y0..=Y1 - 24).contains(&start[1]));
        assert_eq!(self.area(&self.hazards, start[0], start[1]), 0, "出発点に危険セル");
        let mut seen = vec![false; WIDTH * HEIGHT];
        let index = |x: i32, y: i32| (y - Y0) as usize * WIDTH + (x - X0) as usize;
        let mut queue = VecDeque::from([(start[0], start[1])]);
        seen[index(start[0], start[1])] = true;
        let mut found = [false; 3];
        while let Some((x, y)) = queue.pop_front() {
            found[0] |= self.area(&self.woods, x, y) > 0;
            found[1] |= self.area(&self.coals, x, y) > 0;
            found[2] |= self.area(&self.golds, x, y) > 0;
            if found.into_iter().all(|value| value) {
                break;
            }
            for (dx, dy) in [(8, 0), (-8, 0), (0, 8), (0, -8)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < X0 || nx + 16 > X1 || ny < Y0 || ny + 24 > Y1 {
                    continue;
                }
                let i = index(nx, ny);
                if seen[i] || self.area(&self.hazards, nx, ny) != 0 {
                    continue;
                }
                seen[i] = true;
                queue.push_back((nx, ny));
            }
        }
        found
    }
}

/// プレイヤー幅の安全な掘削通路で森の木と浅い石炭を両方採集できる。
#[test]
fn spawn_has_a_safe_route_to_early_resources() {
    let (g, table) = generator();
    let world = World::new(Arc::new(table), 1);
    assert_eq!(testkit::material_of(&world, 'W'), g.materials.wood);
    for seed in 1..=8 {
        let survey = Survey::new(seed, &g);
        let route = survey.routes(g.spawn_point(seed));
        println!(
            "seed {seed}: early wood={}, coal={}, reachable={route:?}",
            survey.counts[1], survey.counts[2]
        );
        assert!(survey.counts[1] > 0 && survey.counts[2] > 0, "seed {seed}: 資源の分布");
        assert!(route[0] && route[1], "seed {seed}: 木と浅い石炭の掘削経路 {route:?}");
    }
}

/// 深い金が底の境界より上にあり、流体を通らない掘削経路で届く。
#[test]
fn deep_resources_remain_reachable() {
    let (g, _) = generator();
    for seed in 1..=8 {
        let survey = Survey::new(seed, &g);
        let route = survey.routes(g.spawn_point(seed));
        println!("seed {seed}: deep gold={}, reachable={route:?}", survey.counts[3]);
        assert!(survey.counts[3] > 0, "seed {seed}: 深い金の分布");
        assert!(route[2], "seed {seed}: 金への安全な掘削経路 {route:?}");
    }
}
