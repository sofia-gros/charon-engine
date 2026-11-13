//! カメラの演出 (仕様 06 §11): なめらかな追従と、画面の揺れ。描画側のもので、シミュレーションには影響しない。

/// 追従と揺れの状態。ゲームは [`CameraFx::shake`] を呼ぶだけでよい。
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraFx {
    /// なめらかに追っている位置 (まだ追い始めていなければ `None`)。
    smoothed: Option<[f32; 2]>,
    /// 揺れの強さ 0..=1。時間で減る。
    trauma: f32,
    /// 揺れの位相 (フレームごとに進む)。
    phase: u32,
}

/// 追従の速さ (1 秒あたり)。大きいほどきびきび追う。
pub const FOLLOW_RATE: f32 = 14.0;
/// これ以上離れたら、なめらかに追わずに一度で移る (リスポーン・ワープ)。
pub const SNAP_DISTANCE: f32 = 320.0;
/// 揺れの最大の振れ幅 (セル)。
pub const SHAKE_MAX: f32 = 10.0;
/// 揺れが消える速さ (1 秒あたりの強さ)。
pub const SHAKE_DECAY: f32 = 1.8;

impl CameraFx {
    /// 画面を揺らす。強さ 0..=1 (重ねると足され、1 で頭打ち)。
    pub fn shake(&mut self, amount: f32) {
        if amount.is_finite() {
            self.trauma = (self.trauma + amount.max(0.0)).min(1.0);
        }
    }

    /// いまの揺れの強さ。
    pub fn trauma(&self) -> f32 {
        self.trauma
    }

    /// 追う位置を忘れる (次のフレームで目標に直接移る)。
    pub fn reset(&mut self) {
        self.smoothed = None;
        self.trauma = 0.0;
    }

    /// 1 フレーム進める。`target` は追いたい左上の位置、`dt` は秒。揺れを足した、描く位置を返す。
    pub fn step(&mut self, target: [f32; 2], dt: f32, smooth: bool) -> [f32; 2] {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let base = match (smooth, self.smoothed) {
            (true, Some(prev)) => {
                let (dx, dy) = (target[0] - prev[0], target[1] - prev[1]);
                if dx * dx + dy * dy > SNAP_DISTANCE * SNAP_DISTANCE {
                    target
                } else {
                    // 時間に依らない指数の追い方: 1 - e^(-rate * dt)。
                    let k = 1.0 - (-FOLLOW_RATE * dt).exp();
                    [prev[0] + dx * k, prev[1] + dy * k]
                }
            }
            _ => target,
        };
        self.smoothed = Some(base);
        self.phase = self.phase.wrapping_add(1);
        let amount = self.trauma * self.trauma * SHAKE_MAX;
        self.trauma = (self.trauma - SHAKE_DECAY * dt).max(0.0);
        if amount <= 0.0 {
            return base;
        }
        let (sx, sy) = jitter(self.phase);
        [base[0] + sx * amount, base[1] + sy * amount]
    }
}

/// 位相から -1..=1 の 2 つの値を作る (乱数は使わず、整数の混ぜ合わせだけ)。
fn jitter(phase: u32) -> (f32, f32) {
    let mix = |mut v: u32| {
        v ^= v >> 16;
        v = v.wrapping_mul(0x7feb_352d);
        v ^= v >> 15;
        v = v.wrapping_mul(0x846c_a68b);
        v ^= v >> 16;
        v
    };
    let unit = |v: u32| (v % 2001) as f32 / 1000.0 - 1.0;
    (unit(mix(phase)), unit(mix(phase ^ 0x9e37_79b9)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_converges_without_overshooting() {
        let mut fx = CameraFx::default();
        assert_eq!(fx.step([0.0, 0.0], 1.0 / 60.0, true), [0.0, 0.0], "最初は目標に直接");
        let mut last = 0.0;
        for _ in 0..120 {
            let p = fx.step([100.0, 0.0], 1.0 / 60.0, true);
            assert!(p[0] >= last && p[0] <= 100.0, "行き過ぎない");
            last = p[0];
        }
        assert!(last > 99.0, "2 秒でほぼ追いつく: {last}");
    }

    #[test]
    fn following_is_frame_rate_independent() {
        let run = |dt: f32, frames: u32| {
            let mut fx = CameraFx::default();
            fx.step([0.0, 0.0], dt, true);
            let mut p = [0.0, 0.0];
            for _ in 0..frames {
                p = fx.step([100.0, 0.0], dt, true);
            }
            p[0]
        };
        let (a, b) = (run(1.0 / 60.0, 30), run(1.0 / 30.0, 15));
        assert!((a - b).abs() < 0.5, "30 fps と 60 fps で同じ: {a} {b}");
    }

    #[test]
    fn far_targets_snap_and_smoothing_can_be_off() {
        let mut fx = CameraFx::default();
        fx.step([0.0, 0.0], 0.016, true);
        assert_eq!(fx.step([5000.0, 0.0], 0.016, true), [5000.0, 0.0]);
        assert_eq!(fx.step([5001.0, 0.0], 0.016, false), [5001.0, 0.0]);
    }

    #[test]
    fn shake_moves_the_view_and_dies_out() {
        let mut fx = CameraFx::default();
        fx.step([0.0, 0.0], 0.016, false);
        fx.shake(1.0);
        let moved = (0..10).any(|_| fx.step([0.0, 0.0], 0.016, false) != [0.0, 0.0]);
        assert!(moved, "揺れると位置がずれる");
        for _ in 0..120 {
            fx.step([0.0, 0.0], 0.016, false);
        }
        assert_eq!(fx.trauma(), 0.0);
        assert_eq!(fx.step([7.0, 7.0], 0.016, false), [7.0, 7.0], "収まったらずれない");
        // 振れ幅は SHAKE_MAX を超えない。強さは重ねても 1 で頭打ち。
        fx.shake(0.8);
        fx.shake(0.8);
        assert_eq!(fx.trauma(), 1.0);
        let p = fx.step([0.0, 0.0], 0.0, false);
        assert!(p[0].abs() <= SHAKE_MAX && p[1].abs() <= SHAKE_MAX);
        fx.shake(f32::NAN);
        fx.reset();
        assert_eq!(fx.trauma(), 0.0);
    }
}
