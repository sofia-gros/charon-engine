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
}
