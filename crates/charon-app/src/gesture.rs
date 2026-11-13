//! 仕様 06 §7・仕様 10 §11: 2 本指のスワイプと距離の変化を、カメラの移動とズームにする。
//!
//! - スワイプ: 指の動きと**逆向き**にカメラが動く (右から左へスワイプすると、カメラはプレイヤーの右側へ動く。画面の絵が指についてくる)。
//!   プレイヤーを追うとき (`follow`) は、プレイヤーからのずれ (`Controls::cam_offset`) を動かす。追わないときは `origin`。
//! - ピンチ: 2 本指の距離の比で、ズームが**滑らかに** (段階でなく) 変わる。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

use crate::ui::Controls;

/// プレイヤーからのカメラのずれの上限 (画面の短い辺に対する割合、セル換算は拡大率で割る)。
pub(crate) const CAM_REACH: f32 = 0.45;

#[derive(Debug, Default)]
pub(super) struct TouchGestures {
    fingers: BTreeMap<u64, [f32; 2]>,
    /// ピンチを始めたときの (2 本指の距離, ズーム)。
    pinch: Option<(f32, f32)>,
    active: bool,
}

impl TouchGestures {
    /// true の間はブラシ・ポインターの操作として扱わない。`follow` が真なら、パンは `cam_offset` に入る。
    /// `viewport` は画面の大きさ (画素)。ずれの上限に使う。
    pub(super) fn update(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: [f32; 2],
        controls: &mut Controls,
        follow: bool,
        viewport: [f32; 2],
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
                let (dx, dy) =
                    (-(center[0] - prev_center[0]) / zoom, -(center[1] - prev_center[1]) / zoom);
                if follow {
                    controls.cam_offset[0] += dx;
                    controls.cam_offset[1] += dy;
                    clamp_offset(&mut controls.cam_offset, viewport, zoom);
                } else {
                    controls.origin[0] += dx;
                    controls.origin[1] += dy;
                }
                controls.cam_idle = 0.0;
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

    /// 2 本指の操作の最中か (1 本が離れても、全部の指が離れるまで続く)。
    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// 触れている指の数。
    pub(super) fn fingers(&self) -> usize {
        self.fingers.len()
    }

    fn pair(&self) -> Option<([f32; 2], [f32; 2])> {
        let mut values = self.fingers.values();
        Some((*values.next()?, *values.next()?))
    }
}

/// カメラのずれ (セル) を、画面の短い辺の `CAM_REACH` 倍の円の中に収める。
pub(crate) fn clamp_offset(offset: &mut [f32; 2], viewport: [f32; 2], zoom: f32) {
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    let reach = viewport[0].min(viewport[1]) * CAM_REACH / zoom;
    let length = offset[0].hypot(offset[1]);
    if length > reach && length > 0.0 {
        offset[0] *= reach / length;
        offset[1] *= reach / length;
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::cell::MaterialId;

    const VIEW: [f32; 2] = [1600.0, 900.0];

    fn controls() -> Controls {
        Controls::new(MaterialId::EMPTY)
    }

    /// 2 本指で、指と逆向きにカメラが動く (右から左へスワイプ → カメラは右へ)。ピンチで滑らかに拡大する。
    #[test]
    fn two_finger_pan_moves_the_camera_opposite_and_pinch_zooms_smoothly() {
        let mut gestures = TouchGestures::default();
        let mut controls = controls();
        controls.zoom = 4.0;
        assert!(!gestures.update(
            1,
            TouchPhase::Started,
            [1000.0, 300.0],
            &mut controls,
            false,
            VIEW
        ));
        assert!(gestures.update(
            2,
            TouchPhase::Started,
            [1100.0, 300.0],
            &mut controls,
            false,
            VIEW
        ));
        // 2 本とも左へ 80 画素 (距離は変わらない)。
        gestures.update(1, TouchPhase::Moved, [920.0, 300.0], &mut controls, false, VIEW);
        gestures.update(2, TouchPhase::Moved, [1020.0, 300.0], &mut controls, false, VIEW);
        assert!(
            controls.origin[0] > 0.0,
            "左へスワイプすると、カメラは右へ: {:?}",
            controls.origin
        );
        assert!((controls.zoom - 4.0).abs() < 1e-4, "距離が同じならズームは変わらない");
        // 広げる: 距離が 100 → 150 で、ズームは 4 → 6 (整数に丸めない)。
        gestures.update(2, TouchPhase::Moved, [1070.0, 300.0], &mut controls, false, VIEW);
        assert!((controls.zoom - 6.0).abs() < 1e-3, "{}", controls.zoom);
        gestures.update(2, TouchPhase::Moved, [1052.0, 300.0], &mut controls, false, VIEW);
        assert!(controls.zoom > 4.0 && controls.zoom < 6.0 && controls.zoom.fract() != 0.0);
        assert!((controls.zoom - 5.28).abs() < 1e-3, "距離 132 / 100 × 4: {}", controls.zoom);
        // 広げすぎ・つぼめすぎは範囲で止まる。
        gestures.update(2, TouchPhase::Moved, [9000.0, 300.0], &mut controls, false, VIEW);
        assert_eq!(controls.zoom, charon_render::ZOOM_MAX);
        gestures.update(2, TouchPhase::Moved, [921.0, 300.0], &mut controls, false, VIEW);
        assert_eq!(controls.zoom, charon_render::ZOOM_MIN);
        assert!(gestures.update(2, TouchPhase::Ended, [921.0, 300.0], &mut controls, false, VIEW));
        assert!(gestures.update(1, TouchPhase::Ended, [920.0, 300.0], &mut controls, false, VIEW));
        assert!(!gestures.active());
        assert!(!gestures.update(
            3,
            TouchPhase::Started,
            [100.0, 100.0],
            &mut controls,
            false,
            VIEW
        ));
    }

    /// プレイヤーを追うときは、パンが `cam_offset` に入り (逆向き)、範囲の円で止まり、タイマーが 0 に戻る。
    #[test]
    fn following_pans_the_offset_inside_a_circle() {
        let mut gestures = TouchGestures::default();
        let mut controls = controls();
        controls.zoom = 4.0;
        controls.cam_idle = 9.0;
        gestures.update(1, TouchPhase::Started, [1000.0, 300.0], &mut controls, true, VIEW);
        gestures.update(2, TouchPhase::Started, [1100.0, 300.0], &mut controls, true, VIEW);
        gestures.update(1, TouchPhase::Moved, [960.0, 300.0], &mut controls, true, VIEW);
        gestures.update(2, TouchPhase::Moved, [1060.0, 300.0], &mut controls, true, VIEW);
        assert_eq!(controls.origin, [0.0, 0.0], "追うときは origin を動かさない");
        // 指を 1 本ずつ動かすので、途中で距離 (ズーム) も変わる。おおよそ 40 画素 ÷ 4 = 10 セル、右へ。
        assert!(
            controls.cam_offset[0] > 7.0 && controls.cam_offset[0] < 11.0,
            "{:?}",
            controls.cam_offset
        );
        assert_eq!(controls.cam_offset[1], 0.0);
        assert_eq!(controls.cam_idle, 0.0);
        // 遠くまでスワイプしても、円の中で止まる。
        for step in 1..=40 {
            let x = 960.0 - step as f32 * 200.0;
            gestures.update(1, TouchPhase::Moved, [x, 300.0], &mut controls, true, VIEW);
            gestures.update(2, TouchPhase::Moved, [x + 100.0, 300.0], &mut controls, true, VIEW);
        }
        // 上限は、そのときの拡大率で決まる。いちばん広い (拡大率が最小の) ときでも、この円の外へは出ない。
        let widest = VIEW[1] * CAM_REACH / charon_render::ZOOM_MIN;
        assert!(controls.cam_offset[0].hypot(controls.cam_offset[1]) <= widest + 1e-3);
        assert!(controls.cam_offset[0] > 50.0, "向きは保たれる: {:?}", controls.cam_offset);
        // 同じ円の判定を、いまの拡大率で直接確かめる。
        let mut far = [1000.0, 0.0];
        clamp_offset(&mut far, VIEW, 4.0);
        assert!((far[0] - VIEW[1] * CAM_REACH / 4.0).abs() < 1e-3);
    }

    /// 1 本指ではカメラは動かない。不正な座標は無視する。
    #[test]
    fn one_finger_and_bad_positions_do_nothing() {
        let mut gestures = TouchGestures::default();
        let mut controls = controls();
        let before = (controls.origin, controls.zoom, controls.cam_offset);
        assert!(!gestures.update(
            1,
            TouchPhase::Started,
            [100.0, 100.0],
            &mut controls,
            true,
            VIEW
        ));
        assert!(!gestures.update(1, TouchPhase::Moved, [300.0, 300.0], &mut controls, true, VIEW));
        assert!(!gestures.update(
            2,
            TouchPhase::Started,
            [f32::NAN, 0.0],
            &mut controls,
            true,
            VIEW
        ));
        assert_eq!(gestures.fingers(), 1);
        assert_eq!((controls.origin, controls.zoom, controls.cam_offset), before);
        let mut offset = [1000.0, 0.0];
        clamp_offset(&mut offset, VIEW, 0.0);
        assert!(offset[0] <= VIEW[1] * CAM_REACH + 1e-3, "拡大率が不正なら 1 とみなす");
    }
}
