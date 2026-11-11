//! シミュレーションのベンチマーク (`docs/TESTING.md` §6)。
//!
//! 実行: `cargo bench -p charon-core`
//! 結果は `docs/BENCHMARKS.md` に追記する。

use charon_core::coords::ChunkPos;
use charon_core::testkit::*;
use charon_core::world::World;
use criterion::{Criterion, criterion_group, criterion_main};

/// 幅 `w`、高さ `h` の箱に、材質 `ch` を上から詰める (下の `rows` 段)。
fn filled_box(w: usize, h: usize, ch: char, rows: usize) -> World {
    let mut world = world_from_ascii(&box_art(w, h), 1);
    fill(&mut world, 1, (h - rows) as i32, w as i32, h as i32 - 1, ch);
    world
}

/// 空中に浮かせた粒を落とす。落下中のセルが多い状態を測る (動いているセル数が最大)。
fn falling(ch: char) -> World {
    let mut world = world_from_ascii(&box_art(320, 320), 1);
    fill(&mut world, 1, 0, 320, 300, ch);
    // 床の上 20 段は空けてあるので、全セルが落ち始める
    world
}

/// 砂 1,048,576 個を空行と交互に並べ、各セルが落下できるようにする。
fn falling_million() -> World {
    let mut world = world_from_ascii(&box_art(1024, 2048), 1);
    for y in (0..2048).step_by(2) {
        fill(&mut world, 1, y, 1024, y, 's');
    }
    // tick 0 では初期 parity と一致して更新を見送るため、tick 1 を計測する。
    world.step();
    world
}

/// 水と溶岩が接する約 6,000 セル。熱・反応・相変化の重なりを測る。
fn reacting() -> World {
    let mut world = world_from_ascii(&box_art(100, 100), 1);
    fill(&mut world, 1, 20, 100, 49, 'w');
    fill(&mut world, 1, 50, 100, 79, 'l');
    world
}

/// 空のまま眠っている 100 × 100 = 10,000 チャンク。
fn sleeping_10k() -> World {
    let mut world = world_from_ascii(".", 1);
    assert_sleeps(&mut world, 20);
    for y in 0..100 {
        for x in 0..100 {
            world.insert_chunk(ChunkPos::new(x, y));
        }
    }
    assert_eq!(world.awake_chunks(), 0);
    world
}

fn bench_sim(c: &mut Criterion) {
    let mut g = c.benchmark_group("step");
    g.sample_size(20);

    // 砂 約 10 万個の落下 (320 x 301 = 96,320 セルが落ちる)
    g.bench_function("falling_sand_100k", |b| {
        b.iter_batched(
            || falling('s'),
            |mut world| run(&mut world, 10),
            criterion::BatchSize::LargeInput,
        );
    });

    // 水 約 10 万個の流れ (落下して広がる)
    g.bench_function("falling_water_100k", |b| {
        b.iter_batched(
            || falling('w'),
            |mut world| run(&mut world, 10),
            criterion::BatchSize::LargeInput,
        );
    });

    // 砂 1,048,576 個が詰まった領域の最初の 1 tick (1024 × 1024)。
    g.bench_function("falling_sand_1m", |b| {
        b.iter_batched(falling_million, |mut world| world.step(), criterion::BatchSize::LargeInput);
    });
    g.bench_function("falling_sand_1m_serial", |b| {
        b.iter_batched(
            falling_million,
            |mut world| world.step_serial(),
            criterion::BatchSize::LargeInput,
        );
    });

    // 反応・熱・相変化が同時に動く世界を 10 tick 進める。
    g.bench_function("reaction_heat_6k", |b| {
        b.iter_batched(reacting, |mut world| run(&mut world, 10), criterion::BatchSize::LargeInput);
    });

    // 眠った世界 (水を満たして眠らせ、1 tick の費用を測る)
    g.bench_function("sleeping_world", |b| {
        let mut world = filled_box(320, 320, 'w', 150);
        assert_sleeps(&mut world, 20_000);
        b.iter(|| world.step());
    });

    g.bench_function("sleeping_10k_chunks", |b| {
        let mut world = sleeping_10k();
        b.iter(|| world.step());
    });

    g.finish();
}

criterion_group!(benches, bench_sim);
criterion_main!(benches);
