//! 仕様 28 §9: 画面の文字の、時間で消える表示。選んだアイテムの名前と、体力・マナの数値。egui の時計 (秒) で判定する。

use charon_game::items::ItemId;

/// 体力・マナの数値を出しておく時間 (秒)。
pub(crate) const VITALS_SHOW_SECS: f64 = 3.0;
/// 名前の消える時間の範囲 (秒)。
pub(crate) const NAME_FADE_RANGE: (f32, f32) = (0.0, 15.0);
/// 既定 (秒)。
pub(crate) const NAME_FADE_DEFAULT: f32 = 1.0;

/// 選んだアイテムの名前を、選び直してから `secs` 秒だけ出す。
#[derive(Clone, Debug, Default)]
pub(crate) struct NameFade {
    key: Option<(usize, Option<ItemId>)>,
    since: f64,
}

impl NameFade {
    /// 枠かアイテムが変わったら、そこから数え直す。`secs` が 0 以下 (または不正) なら、いつも出さない。
    pub(crate) fn visible(&mut self, now: f64, key: (usize, Option<ItemId>), secs: f32) -> bool {
        if self.key != Some(key) {
            self.key = Some(key);
            self.since = now;
        }
        secs.is_finite() && secs > 0.0 && now - self.since <= f64::from(secs)
    }
}

/// ハート・星の数値を、タップされたとき、または体力・マナが変わった (使った・回復した) ときから 3 秒出す。
#[derive(Clone, Debug, Default)]
pub(crate) struct VitalsPopup {
    shown_at: Option<f64>,
    last: Option<(i32, u32)>,
}

impl VitalsPopup {
    pub(crate) fn visible(&mut self, now: f64, current: (i32, u32), tapped: bool) -> bool {
        if tapped || self.last.is_some_and(|last| last != current) {
            self.shown_at = Some(now);
        }
        self.last = Some(current);
        self.shown_at.is_some_and(|at| now - at <= VITALS_SHOW_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 名前は選んだときから設定の秒数だけ出て、消える。選び直すと、また出る。0 秒なら出ない。
    #[test]
    fn tooltip_fade_shows_then_hides_the_item_name() {
        let mut fade = NameFade::default();
        let sword = (0, Some(ItemId(1)));
        assert!(fade.visible(10.0, sword, 1.0), "選んだ直後");
        assert!(fade.visible(10.9, sword, 1.0));
        assert!(!fade.visible(11.2, sword, 1.0), "1 秒を過ぎたら消える");
        assert!(!fade.visible(500.0, sword, 1.0), "消えたまま");
        assert!(fade.visible(501.0, (1, Some(ItemId(1))), 1.0), "枠を変えると、また出る");
        assert!(
            fade.visible(502.0, (1, Some(ItemId(2))), 1.0),
            "同じ枠でも、アイテムが変わると出る"
        );
        // 長くすると長く出る。0 秒・不正な値は出さない。
        assert!(fade.visible(503.0, (2, None), 15.0));
        assert!(fade.visible(517.0, (2, None), 15.0));
        assert!(!fade.visible(519.0, (2, None), 15.0));
        assert!(!fade.visible(600.0, (3, None), 0.0));
        assert!(!fade.visible(600.0, (3, None), f32::NAN));
        assert!(!fade.visible(600.0, (3, None), -1.0));
    }

    /// 数値は、タップされたとき、体力・マナが変わったとき (使った・回復中)、3 秒出る。最初の 1 回は、変わったとは数えない。
    #[test]
    fn tapping_hearts_shows_numbers_for_three_seconds() {
        let mut popup = VitalsPopup::default();
        assert!(!popup.visible(0.0, (100, 50), false), "何もなければ出ない");
        assert!(!popup.visible(5.0, (100, 50), false));
        assert!(popup.visible(6.0, (100, 50), true), "タップ");
        assert!(popup.visible(8.9, (100, 50), false), "3 秒以内");
        assert!(!popup.visible(9.1, (100, 50), false), "3 秒を過ぎたら消える");
        assert!(popup.visible(20.0, (80, 50), false), "体力が減った");
        assert!(popup.visible(22.0, (80, 50), false));
        assert!(!popup.visible(23.5, (80, 50), false));
        assert!(popup.visible(30.0, (80, 70), false), "マナが回復した");
        // 変化が続く間は、出し続ける。
        assert!(popup.visible(32.0, (80, 90), false));
        assert!(popup.visible(34.5, (80, 100), false));
        assert!(!popup.visible(38.0, (80, 100), false));
    }
}
