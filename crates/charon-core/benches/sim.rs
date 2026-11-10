//! シミュレーションのベンチマーク (`docs/TESTING.md` §6)。
//!
//! 実行: `cargo bench -p charon-core`
//! 結果は `docs/BENCHMARKS.md` に追記する。

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

    // 眠った世界 (水を満たして眠らせ、1 tick の費用を測る)
    g.bench_function("sleeping_world", |b| {
        let mut world = filled_box(320, 320, 'w', 150);
        assert_sleeps(&mut world, 20_000);
        b.iter(|| world.step());
    });

    g.finish();
}

criterion_group!(benches, bench_sim);
criterion_main!(benches);
