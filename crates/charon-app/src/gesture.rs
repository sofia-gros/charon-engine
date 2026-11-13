//! 仕様 06 §7: プレイヤーのいない画面 (サンドボックス) の、2 本指のスワイプ (カメラ) とピンチ (ズーム)。
//!
//! プレイヤーのいるゲーム (`follow_player`) では使わない。そちらは、ズームのボタンと、プレイヤーを少し遅れて追うカメラ
//! ([仕様 10 §11](10-noita-like-game.md))。
//!
//! - スワイプ: 指と**逆向き**にカメラが動く (画面の絵が指についてくる)。
//! - ピンチ: 2 本指の距離の比で、ズームが**滑らかに** (段階でなく) 変わる。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

use crate::ui::Controls;

#[derive(Debug, Default)]
pub(super) struct TouchGestures {
    fingers: BTreeMap<u64, [f32; 2]>,
    /// ピンチを始めたときの (2 本指の距離, ズーム)。
    pinch: Option<(f32, f32)>,
    active: bool,
}

impl TouchGestures {
    /// true の間はブラシ操作として扱わない。
    pub(super) fn update(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: [f32; 2],
        controls: &mut Controls,
    ) -> bool {
        if !pos[0].is_finite() || !pos[1].is_finite() {
            return self.active;
        }
        let previous = self.pair();
        match phase {
            TouchPhase::Started | TouchPhase::Moved => {
                self.fingers.insert(id, pos);
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.fingers.remove(&id);
            }
        }
        let next = self.pair();
        if let Some((a, b)) = next {
            self.active = true;
            let distance = distance(a, b);
            if self.pinch.is_none() {
                self.pinch = Some((distance, controls.zoom));
            }
            if let Some((old_a, old_b)) = previous {
                let prev_center = [(old_a[0] + old_b[0]) * 0.5, (old_a[1] + old_b[1]) * 0.5];
                let center = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let zoom = controls.zoom.max(1.0);
                // 指と逆向き (画面の絵が指についてくる向き) に、カメラが動く。
                controls.origin[0] -= (center[0] - prev_center[0]) / zoom;
                controls.origin[1] -= (center[1] - prev_center[1]) / zoom;
            }
            if let Some((baseline, start_zoom)) = self.pinch
                && baseline > 0.0
            {
                controls.zoom = charon_render::clean_zoom(start_zoom * distance / baseline);
            }
        } else {
            self.pinch = None;
        }
        let consumed = self.active;
        if self.fingers.is_empty() {
            self.active = false;
        }
        consumed
    }

    /// 触れている指の数。
    #[cfg(test)]
    pub(super) fn fingers(&self) -> usize {
        self.fingers.len()
    }

    fn pair(&self) -> Option<([f32; 2], [f32; 2])> {
        let mut values = self.fingers.values();
        Some((*values.next()?, *values.next()?))
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::cell::MaterialId;

    fn controls() -> Controls {
        Controls::new(MaterialId::EMPTY)
    }

    /// 2 本指で、指と逆向きにカメラが動く (右から左へスワイプ → カメラは右へ)。ピンチで滑らかに拡大する。
    #[test]
    fn two_finger_pan_moves_the_camera_opposite_and_pinch_zooms_smoothly() {
        let mut gestures = TouchGestures::default();
        let mut controls = controls();
        controls.zoom = 4.0;
        assert!(!gestures.update(1, TouchPhase::Started, [1000.0, 300.0], &mut controls));
        assert!(gestures.update(2, TouchPhase::Started, [1100.0, 300.0], &mut controls));
        gestures.update(1, TouchPhase::Moved, [920.0, 300.0], &mut controls);
        gestures.update(2, TouchPhase::Moved, [1020.0, 300.0], &mut controls);
        assert!(
            controls.origin[0] > 0.0,
            "左へスワイプすると、カメラは右へ: {:?}",
            controls.origin
        );
        assert!((controls.zoom - 4.0).abs() < 1e-4, "距離が同じならズームは変わらない");
        // 広げる: 距離が 100 → 150 で、ズームは 4 → 6 (整数に丸めない)。
        gestures.update(2, TouchPhase::Moved, [1070.0, 300.0], &mut controls);
        assert!((controls.zoom - 6.0).abs() < 1e-3, "{}", controls.zoom);
        gestures.update(2, TouchPhase::Moved, [1052.0, 300.0], &mut controls);
        assert!((controls.zoom - 5.28).abs() < 1e-3, "距離 132 / 100 × 4: {}", controls.zoom);
        // 広げすぎ・つぼめすぎは範囲で止まる。
        gestures.update(2, TouchPhase::Moved, [9000.0, 300.0], &mut controls);
        assert_eq!(controls.zoom, charon_render::ZOOM_MAX);
        gestures.update(2, TouchPhase::Moved, [921.0, 300.0], &mut controls);
        assert_eq!(controls.zoom, charon_render::ZOOM_MIN);
        assert!(gestures.update(2, TouchPhase::Ended, [921.0, 300.0], &mut controls));
        assert!(gestures.update(1, TouchPhase::Ended, [920.0, 300.0], &mut controls));
        assert!(!gestures.update(3, TouchPhase::Started, [100.0, 100.0], &mut controls));
    }

    /// 1 本指ではカメラは動かない。不正な座標は無視する。
    #[test]
    fn one_finger_and_bad_positions_do_nothing() {
        let mut gestures = TouchGestures::default();
        let mut controls = controls();
        let before = (controls.origin, controls.zoom);
        assert!(!gestures.update(1, TouchPhase::Started, [100.0, 100.0], &mut controls));
        assert!(!gestures.update(1, TouchPhase::Moved, [300.0, 300.0], &mut controls));
        assert!(!gestures.update(2, TouchPhase::Started, [f32::NAN, 0.0], &mut controls));
        assert_eq!(gestures.fingers(), 1);
        assert_eq!((controls.origin, controls.zoom), before);
    }
}
