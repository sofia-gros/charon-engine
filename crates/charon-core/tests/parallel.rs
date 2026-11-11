//! 同一フェーズの並列処理と逐次処理の同値性 (仕様 02 §2)。

use charon_core::rng::hash;
use charon_core::testkit::*;
use charon_core::world::World;

fn scene(seed: u64) -> World {
    // 4×4 チャンクに分布させ、同じフェーズで複数チャンクが起きるようにする。
    let mut world = world_from_ascii(&box_art(200, 200), seed);
    let kinds = ['s', 'w', 'l', 'a', 'W', 'f'];
    for y in (0..200).step_by(5) {
        for x in (1..=200).step_by(5) {
            let h = hash(seed, 0, x, y, 0);
            place(&mut world, x, y, kinds[((h >> 8) % kinds.len() as u32) as usize]);
        }
    }
    world
}

#[test]
fn parallel_matches_serial() {
    for seed in 1..=8 {
        let mut parallel = scene(seed);
        let mut serial = scene(seed);
        for tick in 0..100 {
            parallel.step();
            serial.step_serial();
            assert_eq!(parallel.state_hash(), serial.state_hash(), "seed {seed}, tick {tick}");
        }
    }
}

#[cfg(feature = "parallel")]
#[test]
fn thread_count_is_deterministic() {
    use rayon::ThreadPoolBuilder;

    for seed in 1..=8 {
        let mut reference = None;
        for threads in [1, 2, 4, 8] {
            let pool = ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let mut world = scene(seed);
            let trace: Vec<_> = (0..100)
                .map(|_| {
                    pool.install(|| world.step());
                    world.state_hash()
                })
                .collect();
            if let Some(expected) = &reference {
                assert_eq!(&trace, expected, "seed {seed}, threads {threads}");
            } else {
                reference = Some(trace);
            }
        }
    }
}
