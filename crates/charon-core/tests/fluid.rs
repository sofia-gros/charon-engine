//! 液体と気体の移動 (仕様 02 §8)。

use charon_core::testkit::*;

const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

fn spread(heights: &[u32]) -> u32 {
    let max = heights.iter().copied().max().unwrap_or(0);
    let min = heights.iter().copied().min().unwrap_or(0);
    max - min
}

#[test]
fn water_levels() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(40, 30), seed);
        pour(&mut world, 1, 'w', 400);
        assert_sleeps(&mut world, 3000);
        assert_eq!(count(&world, 'w'), 400, "seed {seed}");
        let heights = column_heights(&world, 'w', 1, 41);
        assert!(spread(&heights) <= 1, "seed {seed}: {heights:?}");
    }
}

#[test]
fn sand_sinks_in_water() {
    for seed in SEEDS {
        let mut world = world_from_ascii(
            "
            #......#
            #..ss..#
            #wwwwww#
            #wwwwww#
            ########
            ",
            seed,
        );
        assert_sleeps(&mut world, 2000);
        assert_eq!((count(&world, 's'), count(&world, 'w')), (2, 12), "seed {seed}");
        // 砂は 2 個とも最下段にあり、その分の水が上に押し出されている
        assert_eq!(to_ascii(&world, 1, 3, 7, 4).matches('s').count(), 2, "seed {seed}");
        assert_eq!(to_ascii(&world, 1, 1, 7, 2).matches('w').count(), 2, "seed {seed}");
    }
}

#[test]
fn oil_floats_on_water() {
    for seed in SEEDS {
        // 油と水を 1 列おきに並べる
        let mut world = world_from_ascii(&box_art(20, 20), seed);
        for x in 1..=20 {
            fill(&mut world, x, 10, x, 19, if x % 2 == 0 { 'o' } else { 'w' });
        }
        assert_sleeps(&mut world, 5000);
        assert_eq!((count(&world, 'o'), count(&world, 'w')), (100, 100), "seed {seed}");
        // 下 5 段はすべて水、その上 5 段はすべて油
        assert_eq!(to_ascii(&world, 1, 15, 21, 20).matches('w').count(), 100, "seed {seed}");
        assert_eq!(to_ascii(&world, 1, 10, 21, 15).matches('o').count(), 100, "seed {seed}");
    }
}

#[test]
fn slime_is_slower_than_water() {
    for seed in SEEDS {
        let mut widths = Vec::new();
        for ch in ['w', 'm'] {
            let mut world = world_from_ascii(&box_art(60, 20), seed);
            fill(&mut world, 26, 10, 35, 19, ch);
            run(&mut world, 100);
            assert_eq!(count(&world, ch), 100);
            let heights = column_heights(&world, ch, 1, 61);
            widths.push(heights.iter().filter(|&&h| h > 0).count());
        }
        assert!(widths[1] < widths[0], "seed {seed}: 水 {} スライム {}", widths[0], widths[1]);
    }
}

#[test]
fn gas_rises() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(10, 40), seed);
        fill(&mut world, 3, 38, 6, 39, 'g');
        assert_sleeps(&mut world, 3000);
        assert_eq!(count(&world, 'g'), 8, "seed {seed}");
        // 8 個すべてが天井 (y = 0) に並んでいる
        assert_eq!(to_ascii(&world, 1, 0, 11, 1).matches('g').count(), 8, "seed {seed}");
    }
}

/// 左の池、長さ 30 の水平なトンネル、右の部屋。
fn tunnel_art() -> String {
    let mut art = String::new();
    for y in 0..20 {
        art.push('#');
        art.push_str(&".".repeat(15));
        art.push_str(&(if y == 19 { "." } else { "#" }).repeat(30));
        art.push_str(&".".repeat(15));
        art.push_str("#\n");
    }
    art.push_str(&"#".repeat(62));
    art
}

#[test]
fn water_flows_through_tunnel() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&tunnel_art(), seed);
        fill(&mut world, 1, 5, 15, 19, 'w');
        assert_sleeps(&mut world, 6000);
        assert_eq!(count(&world, 'w'), 225, "seed {seed}");
        // トンネル (x = 16..46) が水で満ち、右の部屋 (x = 46..61) の床が水で覆われる
        assert_eq!(to_ascii(&world, 16, 19, 46, 20), "w".repeat(30), "seed {seed}");
        assert_eq!(to_ascii(&world, 46, 19, 61, 20), "w".repeat(15), "seed {seed}");
    }
}

#[test]
fn breached_wall_drains() {
    for seed in SEEDS {
        // 幅 50 の箱の左 20 列に水をため、x = 21 に壁を立てる
        let mut world = world_from_ascii(&box_art(50, 20), seed);
        fill(&mut world, 1, 10, 20, 19, 'w');
        fill(&mut world, 21, 10, 21, 19, '#');
        assert_sleeps(&mut world, 2000);
        assert_eq!(column_heights(&world, 'w', 22, 51), vec![0; 29]);

        place(&mut world, 21, 15, '.'); // 壁に穴を開ける
        assert_sleeps(&mut world, 6000);
        assert_eq!(count(&world, 'w'), 200, "seed {seed}");
        let left = column_heights(&world, 'w', 1, 21);
        let right = column_heights(&world, 'w', 22, 51);
        // 穴は床から 5 段目。左は穴の高さまで下がり、あふれた分が右に流れている
        assert!(left.iter().all(|&h| h <= 5), "seed {seed}: 左 {left:?}");
        assert!(spread(&left) <= 1 && spread(&right) <= 1, "seed {seed}: 左 {left:?} 右 {right:?}");
        assert!(right.iter().all(|&h| h >= 3), "seed {seed}: 右 {right:?}");
    }
}
