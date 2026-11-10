//! どんな世界でも成り立つ性質 (`docs/TESTING.md` §5)。

use charon_core::rng::hash;
use charon_core::testkit::*;

/// M1 の時点で動く材質: 砂、土、灰、水、スライム、油、メタン。
const KINDS: [char; 7] = ['s', 'd', 'h', 'w', 'm', 'o', 'g'];

#[test]
fn mass_is_conserved() {
    for seed in 1..=32u64 {
        // 2 チャンク × 2 チャンクにまたがる箱 (境界をまたぐ移動を含める)
        let mut world = world_from_ascii(&box_art(100, 100), seed);
        for y in 0..100 {
            for x in 1..=100 {
                let h = hash(seed, 0, x, y, 0);
                if h & 1 == 0 {
                    place(&mut world, x, y, KINDS[((h >> 8) % KINDS.len() as u32) as usize]);
                }
            }
        }
        let before: Vec<usize> = KINDS.iter().map(|&c| count(&world, c)).collect();
        for step in 0..500 {
            world.step();
            if step % 100 == 99 {
                let now: Vec<usize> = KINDS.iter().map(|&c| count(&world, c)).collect();
                assert_eq!(now, before, "seed {seed}、{} tick 後", step + 1);
            }
        }
    }
}

#[test]
fn random_world_goes_to_sleep() {
    for seed in 1..=8u64 {
        let mut world = world_from_ascii(&box_art(100, 100), seed);
        for y in 0..100 {
            for x in 1..=100 {
                let h = hash(seed, 0, x, y, 0);
                if h & 1 == 0 {
                    place(&mut world, x, y, KINDS[((h >> 8) % KINDS.len() as u32) as usize]);
                }
            }
        }
        assert_sleeps(&mut world, 20_000);
    }
}
