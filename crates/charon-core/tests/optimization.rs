//! 仕様 08 §4: 最適化前後の毎 tick の状態を比較する。

use charon_core::testkit::{box_art, fill, world_from_ascii};

#[test]
fn optimization_matches_baseline() {
    let expected: Vec<_> = include_str!("golden/optimization.txt").lines().collect();
    assert_eq!(expected.len(), 8 * 16, "seed 1〜8 の各 16 tick が必要");
    for seed in 1..=8 {
        let mut world = world_from_ascii(&box_art(62, 64), seed);
        fill(&mut world, 12, 3, 20, 10, 's');
        fill(&mut world, 25, 0, 33, 8, 'w');
        for tick in 1..=16 {
            world.step();
            let actual = format!("{:016x}", world.state_hash());
            assert_eq!(
                actual,
                expected[((seed - 1) * 16 + tick - 1) as usize],
                "seed={seed}, tick={tick}"
            );
        }
    }
}
