//! 仕様 21: 冒険の世界の地形 (層・洞窟・鉱石・池・構造物・底・決定性)。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::collections::VecDeque;

use charon_core::cell::{FLAG_BLOCK, MaterialId};
use charon_core::coords::{CHUNK_SIZE, ChunkPos};
use charon_core::material::MaterialTable;
use charon_world::generator::ChunkGenerator;
use charon_world::terrain::{
    AdventureGenerator, AdventureMaterials, BOTTOM, Biome, SURFACE_BASE, biome, surface_y,
};

fn generator() -> (AdventureGenerator, MaterialTable) {
    let table = MaterialTable::builtin().unwrap();
    let materials = AdventureMaterials::from_table(&table).unwrap();
    (AdventureGenerator { materials }, table)
}

/// 同じ seed なら同じチャンク。seed が違えば違う。生成の順序によらない。
#[test]
fn chunks_are_deterministic_and_seed_dependent() {
    let (g, table) = generator();
    for pos in [ChunkPos::new(0, 3), ChunkPos::new(-5, 10), ChunkPos::new(7, 20)] {
        let a = g.generate(1, pos, &table);
        let b = g.generate(1, pos, &table);
        assert!(a.cells == b.cells, "{pos:?}");
    }
    let differs = (0..8).any(|i| {
        let pos = ChunkPos::new(i, 10);
        g.generate(1, pos, &table).cells != g.generate(2, pos, &table).cells
    });
    assert!(differs, "seed が違えば、地形が違う");
}

/// チャンクの中身は、`cell_at` と一致する (隣のチャンクを見なくても、境界でつながる)。
#[test]
fn chunk_cells_match_cell_at_across_boundaries() {
    let (g, table) = generator();
    let seed = 3;
    for pos in [ChunkPos::new(0, 3), ChunkPos::new(-1, 6), ChunkPos::new(2, 14)] {
        let chunk = g.generate(seed, pos, &table);
        let origin = pos.origin();
        for ly in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let want = g.cell_at(seed, origin.x + lx, origin.y + ly);
                let got = chunk.get(lx as u8, ly as u8);
                assert_eq!(
                    got.material,
                    want.map_or(MaterialId::EMPTY, |c| c.material),
                    "{pos:?} ({lx}, {ly})"
                );
            }
        }
    }
}

/// 表面は滑らかにつながり、出発点のまわりは平ら。
#[test]
fn the_surface_is_continuous_and_flat_at_spawn() {
    for seed in 1..=4 {
        let mut max_step = 0;
        for x in -2000..2000 {
            max_step = max_step.max((surface_y(seed, x + 1) - surface_y(seed, x)).abs());
        }
        // 家の下の平らな台の段差 (最大 1 ブロックと少し) まで。
        assert!(max_step <= 16, "seed {seed}: 段差 {max_step}");
        for x in -24..=24 {
            assert_eq!(surface_y(seed, x), SURFACE_BASE, "seed {seed} x {x}");
        }
        assert_eq!(biome(seed, 0), Biome::Forest);
    }
}

/// 層: 表面の下に土 (または砂・氷)、その下は石。深いところに鉱石。底は境界。
#[test]
fn layers_run_from_dirt_through_stone_to_the_boundary() {
    let (g, _) = generator();
    let m = g.materials;
    let seed = 1;
    let x = 0;
    let top = surface_y(seed, x);
    assert!(g.cell_at(seed, x, top - 1).is_none(), "表面の上は空");
    assert_eq!(g.cell_at(seed, x, top).unwrap().material, m.grass);
    assert_eq!(g.cell_at(seed, x, top + 10).unwrap().material, m.dirt);
    // 石の層 (洞窟でない列を探す)。
    let stone_seen = (-300..300).any(|x| {
        let t = surface_y(seed, x);
        g.cell_at(seed, x, t + 60).is_some_and(|c| c.material == m.stone)
    });
    assert!(stone_seen);
    // 底: これ以上は境界。
    for x in [-500, 0, 123, 4000] {
        assert_eq!(g.cell_at(seed, x, BOTTOM).unwrap().material, MaterialId::BOUNDARY);
        assert_eq!(g.cell_at(seed, x, BOTTOM + 500).unwrap().material, MaterialId::BOUNDARY);
        assert_ne!(
            g.cell_at(seed, x, BOTTOM - 1).map(|c| c.material),
            Some(MaterialId::BOUNDARY),
            "底の 1 つ上は境界ではない"
        );
    }
}

/// 鉱石は、決まった深さの範囲に出る。石炭は浅く、金は深い。
#[test]
fn ores_appear_in_their_depth_ranges() {
    let (g, _) = generator();
    let m = g.materials;
    let seed = 1;
    let mut depths: [Vec<i32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for x in (-1200..1200).step_by(2) {
        let top = surface_y(seed, x);
        for depth in (30..1100).step_by(3) {
            match g.cell_at(seed, x, top + depth).map(|c| c.material) {
                Some(id) if id == m.coal => depths[0].push(depth),
                Some(id) if id == m.iron => depths[1].push(depth),
                Some(id) if id == m.gold => depths[2].push(depth),
                _ => {}
            }
        }
    }
    for (name, found) in ["coal", "iron", "gold"].iter().zip(&depths) {
        assert!(found.len() > 20, "{name} がある: {}", found.len());
    }
    assert!(depths[0].iter().all(|&d| (40..520).contains(&d)), "石炭は浅い");
    assert!(depths[1].iter().all(|&d| (130..950).contains(&d)), "鉄は中ほど");
    assert!(depths[2].iter().all(|&d| d >= 360), "金は深い");
}

/// 洞窟: 広い空洞があり、出発点から歩いて (4 方向に) 深い洞窟まで降りられる。
#[test]
fn caves_are_large_and_reachable_from_the_surface() {
    let (g, _) = generator();
    let seed = 1;
    let (x0, x1) = (-1200, 1200);
    let (y0, y1) = (SURFACE_BASE - 90, SURFACE_BASE + 420);
    let w = (x1 - x0) as usize;
    let h = (y1 - y0) as usize;
    let passable = |x: i32, y: i32| -> bool {
        match g.cell_at(seed, x, y) {
            None => true,
            Some(c) => c.material == g.materials.water,
        }
    };
    // 空洞の割合。
    let mut open_cells = 0usize;
    for y in (SURFACE_BASE + 60)..(SURFACE_BASE + 400) {
        for x in x0..x1 {
            if g.cell_at(seed, x, y).is_none() {
                open_cells += 1;
            }
        }
    }
    let total = (x1 - x0) as usize * 340;
    let percent = open_cells * 100 / total;
    assert!((8..=45).contains(&percent), "深さ 60..400 の空洞の割合 {percent}%");
    // 出発点の上の空から、空洞を通って、深さ 150 まで行けるか (幅優先)。
    let start = (0, SURFACE_BASE - 2);
    assert!(passable(start.0, start.1));
    let mut seen = vec![false; w * h];
    let index = |x: i32, y: i32| (y - y0) as usize * w + (x - x0) as usize;
    let mut queue = VecDeque::new();
    seen[index(start.0, start.1)] = true;
    queue.push_back(start);
    let mut deepest = 0;
    let mut visited = 0usize;
    while let Some((x, y)) = queue.pop_front() {
        visited += 1;
        deepest = deepest.max(y - SURFACE_BASE);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < x0
                || nx >= x1
                || ny < y0
                || ny >= y1
                || seen[index(nx, ny)]
                || !passable(nx, ny)
            {
                continue;
            }
            seen[index(nx, ny)] = true;
            queue.push_back((nx, ny));
        }
    }
    assert!(deepest >= 100, "空から洞窟へ降りられる: 最も深い {deepest}");
    assert!(visited > 20_000, "つながった空洞が広い: {visited}");
}

/// 池: 森の表面に水たまりがある。砂漠にはない。
#[test]
fn ponds_exist_in_forests_but_not_deserts() {
    let (g, _) = generator();
    let seed = 1;
    let mut forest_water = 0;
    let mut desert_water = 0;
    for x in -3000..3000 {
        let top = surface_y(seed, x);
        let wet = g.cell_at(seed, x, top).is_some_and(|c| c.material == g.materials.water);
        match biome(seed, x) {
            Biome::Forest if wet => forest_water += 1,
            Biome::Desert if wet => desert_water += 1,
            _ => {}
        }
    }
    assert!(forest_water > 20, "森に池がある: {forest_water}");
    assert_eq!(desert_water, 0, "砂漠に池はない");
}

/// 構造物: ダンジョンや家は、ブロック (完全な 8×8) だけでできている。
#[test]
fn structures_are_made_of_whole_aligned_blocks() {
    let (g, table) = generator();
    let seed = 1;
    let mut block_cells = 0;
    let mut partial = 0;
    for cy in 3..30 {
        for cx in -8..8 {
            let chunk = g.generate(seed, ChunkPos::new(cx, cy), &table);
            let origin = ChunkPos::new(cx, cy).origin();
            for by in 0..8 {
                for bx in 0..8 {
                    let flagged = (0..8)
                        .flat_map(|dy| (0..8).map(move |dx| (dx, dy)))
                        .filter(|&(dx, dy)| {
                            chunk.get((bx * 8 + dx) as u8, (by * 8 + dy) as u8).flags & FLAG_BLOCK
                                != 0
                        })
                        .count();
                    block_cells += flagged;
                    if flagged != 0 && flagged != 64 {
                        partial += 1;
                    }
                    let _ = origin;
                }
            }
        }
    }
    assert!(block_cells > 2000, "ダンジョンや家がある: {block_cells} セル");
    assert_eq!(partial, 0, "ブロックは、いつも 64 セルそろっている");
}

/// 生成の速さ: 画面のまわりの 15×11 チャンクを作る時間 (目安を出力し、極端に遅くないことだけを確かめる)。
#[test]
fn generating_the_view_around_the_spawn_is_fast_enough() {
    let (g, table) = generator();
    let start = std::time::Instant::now();
    let mut solid = 0usize;
    for cy in 0..11 {
        for cx in -7..=7 {
            let chunk = g.generate(1, ChunkPos::new(cx, cy + 1), &table);
            solid += chunk.cells.iter().filter(|c| c.material != MaterialId::EMPTY).count();
        }
    }
    let ms = start.elapsed().as_secs_f32() * 1000.0;
    println!(
        "165 チャンクの生成: {ms:.0} ms (1 チャンクあたり {:.1} ms)、固体 {solid} セル",
        ms / 165.0
    );
    assert!(solid > 0);
    assert!(ms < 30_000.0, "極端に遅い: {ms} ms");
}
