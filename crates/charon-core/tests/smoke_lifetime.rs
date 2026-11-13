//! 煙は寿命で消えて、世界は眠る (仕様 03 §2、仕様 02 §9)。止まらずに動き続けるのは煙ではなく、
//! 溶岩の熱の伝導と、溶岩の上の高温の蒸気である (`docs/spec/02-simulation.md` §9.4 の注意)。

#![allow(clippy::unwrap_used)]

use charon_core::testkit::{assert_sleeps, box_art, count, fill, run, world_from_ascii};

/// 閉じた箱の中の煙は、寿命 (30〜90 の 4 tick 刻み) で消え、すべて消えたら世界は眠る。
#[test]
fn smoke_decays_by_itself_and_the_world_falls_asleep() {
    for seed in 1..=4 {
        let mut world = world_from_ascii(&box_art(60, 60), seed);
        fill(&mut world, 10, 20, 50, 40, 'k');
        assert!(count(&world, 'k') > 800);
        run(&mut world, 500);
        assert_eq!(count(&world, 'k'), 0, "seed {seed}: 煙は寿命で消える");
        assert_sleeps(&mut world, 200);
    }
}
