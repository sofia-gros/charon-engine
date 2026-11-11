//! 仕様 06 §6: 60 tick/秒、最大 0.1 秒を加算し、1 フレームで最大 3 tick。

use std::time::Duration;

const SECOND: u128 = 1_000_000_000;
const HZ: u128 = 60;
const MAX_ELAPSED_NS: u128 = 100_000_000;
const MAX_STEPS: u32 = 3;

/// 小数の丸め誤差なしで 1/60 秒を累積する固定ステップ時計。
#[derive(Debug, Default)]
pub struct FixedTimestep {
    accumulator: u128,
}

/// 描画ループから切り出した停止・1 tick 送り・再開の規則。
/// ウィンドウがなくても同じ状態遷移を検査できる。
pub(crate) fn advance_frame(
    timer: &mut FixedTimestep,
    elapsed: Duration,
    paused: bool,
    step_once: bool,
    mut step: impl FnMut(),
) -> u32 {
    if paused {
        timer.reset();
        if step_once {
            step();
            return 1;
        }
        0
    } else {
        timer.advance(elapsed, step)
    }
}

impl FixedTimestep {
    /// 経過時間のうち最大 0.1 秒を加え、必要な回数だけ `step` を呼ぶ。
    pub fn advance(&mut self, elapsed: Duration, mut step: impl FnMut()) -> u32 {
        self.accumulator += elapsed.as_nanos().min(MAX_ELAPSED_NS) * HZ;
        let mut steps = 0;
        while self.accumulator >= SECOND && steps < MAX_STEPS {
            step();
            self.accumulator -= SECOND;
            steps += 1;
        }
        if steps == MAX_STEPS {
            self.accumulator = 0;
        }
        steps
    }

    /// `suspended` / `resumed` で経過時間を持ち越さない。
    pub fn reset(&mut self) {
        self.accumulator = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_timestep() {
        let mut timer = FixedTimestep::default();
        let mut count = 0;
        assert_eq!(timer.advance(Duration::from_millis(16), || count += 1), 0);
        assert_eq!(timer.advance(Duration::from_millis(1), || count += 1), 1);
        assert_eq!(count, 1);
        assert_eq!(timer.advance(Duration::from_millis(50), || count += 1), 3);
        assert_eq!(count, 4);
        // 3 ステップを踏んだら遅れを捨てる。
        assert_eq!(timer.advance(Duration::from_millis(10), || count += 1), 0);
        assert_eq!(timer.advance(Duration::from_millis(7), || count += 1), 1);
        // 長いフレームは 0.1 秒に抑え、3 tick で打ち切る。
        assert_eq!(timer.advance(Duration::from_secs(2), || count += 1), 3);
        timer.reset();
        assert_eq!(timer.advance(Duration::from_millis(16), || count += 1), 0);
        assert_eq!(count, 8);
    }

    #[test]
    fn headless_pause_and_resume() {
        use charon_core::testkit::{box_art, world_from_ascii};

        for seed in 1..=8 {
            let mut world = world_from_ascii(&box_art(62, 64), seed);
            let mut timer = FixedTimestep::default();
            assert_eq!(
                advance_frame(&mut timer, Duration::from_millis(17), false, false, || world.step()),
                1,
                "seed={seed}"
            );
            let before = world.tick();
            assert_eq!(
                advance_frame(&mut timer, Duration::from_secs(2), true, false, || world.step()),
                0,
                "seed={seed}"
            );
            assert_eq!(world.tick(), before, "seed={seed}");
            assert_eq!(
                advance_frame(&mut timer, Duration::from_secs(2), true, true, || world.step()),
                1,
                "seed={seed}"
            );
            assert_eq!(world.tick(), before + 1, "seed={seed}");
            // 復帰時の reset と同じ操作を行い、停止中の 2 秒を持ち越さない。
            timer.reset();
            assert_eq!(
                advance_frame(&mut timer, Duration::from_millis(1), false, false, || world.step()),
                0,
                "seed={seed}"
            );
            assert_eq!(world.tick(), before + 1, "seed={seed}");
            assert_eq!(
                advance_frame(&mut timer, Duration::from_millis(17), false, false, || world.step()),
                1,
                "seed={seed}"
            );
        }
    }
}
