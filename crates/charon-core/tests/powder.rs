//! 粉体の移動 (仕様 02 §7)。

use charon_core::testkit::*;

const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

#[test]
fn sand_falls() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(5, 50), seed);
        place(&mut world, 3, 0, 's');
        run(&mut world, 200);
        assert_eq!(to_ascii(&world, 3, 49, 4, 50), "s", "seed {seed}");
        assert_eq!(count(&world, 's'), 1);
    }
}

#[test]
fn sand_piles() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(60, 50), seed);
        pour(&mut world, 30, 's', 500);
        assert_sleeps(&mut world, 3000);
        assert_eq!(count(&world, 's'), 500, "seed {seed}");
        let heights = column_heights(&world, 's', 1, 61);
        let height = heights.iter().copied().max().unwrap_or(0);
        let width = heights.iter().filter(|&&h| h > 0).count() as u32;
        assert!(
            height < width,
            "seed {seed}: 高さ {height} 幅 {width}\n{}",
            to_ascii(&world, 0, 20, 62, 51)
        );
    }
}

#[test]
fn friction_makes_steeper_piles() {
    for seed in SEEDS {
        let mut heights = Vec::new();
        for ch in ['s', 'd'] {
            let mut world = world_from_ascii(&box_art(60, 60), seed);
            pour(&mut world, 30, ch, 400);
            assert_sleeps(&mut world, 3000);
            assert_eq!(count(&world, ch), 400);
            heights.push(column_heights(&world, ch, 1, 61).into_iter().max().unwrap_or(0));
        }
        // 土 (friction 220) は砂 (friction 60) より崩れにくい
        assert!(heights[1] > heights[0], "seed {seed}: 砂 {} 土 {}", heights[0], heights[1]);
    }
}

#[test]
fn chunk_border() {
    for seed in SEEDS {
        // 縦に 2 チャンク、横に 2 チャンクにまたがる箱
        let mut world = world_from_ascii(&box_art(100, 100), seed);
        assert_eq!(world.chunk_positions().len(), 4);
        pour(&mut world, 64, 's', 300);
        assert_sleeps(&mut world, 3000);
        assert_eq!(count(&world, 's'), 300, "seed {seed}");
        // 山は床 (y = 99、下のチャンク) の上にあり、チャンクの境界 x = 64 をまたいでいる
        let heights = column_heights(&world, 's', 1, 101);
        assert!(heights[62] > 0 && heights[63] > 0, "seed {seed}");
        assert_eq!(to_ascii(&world, 64, 99, 65, 100), "s");
    }
}

#[test]
fn unsupported_sand_falls_after_sleeping() {
    for seed in SEEDS {
        let mut world = world_from_ascii(
            "
            #s#
            ###
            #.#
            #.#
            ###
            ",
            seed,
        );
        assert_sleeps(&mut world, 100);
        // 支えを消すと、眠っていた砂が落ちる (parity が古くても取りこぼさない)
        place(&mut world, 1, 1, '.');
        assert_sleeps(&mut world, 100);
        assert_eq!(to_ascii(&world, 1, 3, 2, 4), "s", "seed {seed}");
    }
}
