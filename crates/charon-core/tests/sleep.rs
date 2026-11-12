//! 休眠 (仕様 02 §9)。変化しなくなった世界は、必ず全チャンクが眠る。

use charon_core::rng::hash;
use charon_core::testkit::*;

const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

#[test]
fn settles_to_sleep() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(60, 40), seed);
        pour(&mut world, 15, 's', 200);
        pour(&mut world, 45, 'w', 200);
        assert_sleeps(&mut world, 3000);
        assert_eq!((count(&world, 's'), count(&world, 'w')), (200, 200), "seed {seed}");
    }
}

/// 更新範囲の外のチャンクは動かず、範囲に入ると動いて眠る (Terraria と同じく、遠くの砂や水は計算しない)。
#[test]
fn outside_active_region_is_frozen() {
    use charon_core::coords::{CellPos, ChunkPos};
    let snapshot = |world: &charon_core::world::World| -> Vec<u16> {
        (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .map(|(x, y)| world.get(CellPos::new(x, y)).material.0)
            .collect()
    };
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(60, 40), seed);
        pour(&mut world, 15, 's', 100);
        world.set_active_region(Some((ChunkPos::new(20, 20), 1)));
        let before = snapshot(&world);
        for _ in 0..60 {
            world.step();
        }
        assert_eq!(snapshot(&world), before, "seed {seed}: 範囲の外で動いた");
        world.set_active_region(None);
        assert_sleeps(&mut world, 3000);
        assert_ne!(snapshot(&world), before, "seed {seed}: 範囲に入っても動かない");
    }
}

#[test]
fn full_pool_sleeps() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(20, 30), seed);
        fill(&mut world, 1, 25, 20, 29, 'w'); // ちょうど 20×5
        assert_sleeps(&mut world, 2000);
        assert_eq!(column_heights(&world, 'w', 1, 21), vec![5; 20], "seed {seed}");
    }
}

#[test]
fn partial_layer_sleeps() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(20, 30), seed);
        pour(&mut world, 10, 'w', 20 * 5 + 7); // 最上段が埋まりきらない数
        assert_sleeps(&mut world, 3000);
        let heights = column_heights(&world, 'w', 1, 21);
        assert_eq!(heights.iter().sum::<u32>(), 107, "seed {seed}");
        assert!(heights.iter().all(|&h| h == 5 || h == 6), "seed {seed}: {heights:?}");
    }
}

#[test]
fn gas_under_ceiling_sleeps() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(20, 12), seed);
        fill(&mut world, 1, 11, 20, 11, 'g');
        fill(&mut world, 3, 10, 9, 10, 'g'); // 合計 27 個。天井の 2 段目が埋まりきらない
        assert_sleeps(&mut world, 3000);
        assert_eq!(count(&world, 'g'), 27, "seed {seed}");
        assert_eq!(to_ascii(&world, 1, 0, 21, 1), "g".repeat(20), "seed {seed}");
        assert_eq!(to_ascii(&world, 1, 1, 21, 2).matches('g').count(), 7, "seed {seed}");
    }
}

#[test]
fn mixed_liquids_sleep() {
    for seed in SEEDS {
        let mut world = world_from_ascii(&box_art(30, 30), seed);
        for y in 5..25 {
            for x in 3..27 {
                let ch = ['w', 'o', 'm'][(hash(seed, 0, x, y, 0) % 3) as usize];
                place(&mut world, x, y, ch);
            }
        }
        let before = [count(&world, 'w'), count(&world, 'o'), count(&world, 'm')];
        assert_sleeps(&mut world, 8000);
        assert_eq!([count(&world, 'w'), count(&world, 'o'), count(&world, 'm')], before);
        // 重い順に下から: スライム (m)、水 (w)、油 (o)。どの列でも、重い液体の上に軽い液体がない。
        let rows: Vec<String> = (0..30).map(|y| to_ascii(&world, 1, y, 31, y + 1)).collect();
        let rank = |c: char| match c {
            'o' => Some(0),
            'w' => Some(1),
            'm' => Some(2),
            _ => None,
        };
        for x in 0..30 {
            // 上から順に見て、重さの順位が減ってはいけない
            let mut last = 0;
            for row in &rows {
                if let Some(r) = row.chars().nth(x).and_then(rank) {
                    assert!(r >= last, "seed {seed}: 列 {x} で重さの順が逆\n{}", rows.join("\n"));
                    last = r;
                }
            }
        }
        assert!(rows[29].chars().all(|c| c == 'm'), "seed {seed}: 最下段はスライム");
    }
}
