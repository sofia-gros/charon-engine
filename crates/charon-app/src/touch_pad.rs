//! 仕様 10 §10: 画面のタッチ操作 (仮想スティック・スワイプ・ボタン)。ゲームパッドと同じ `PadOutput` を作る。
//!
//! 左下の**移動スティック** (横にずらすと歩く、上へスワイプするとジャンプ)、右下の**狙いスティック**
//! (触れている間は武器を撃ち、ずらした向きへ狙う)、右端の**ボタン** (`JUMP`・`HOOK` = 右手の道具・`PAINT` = 武器 / ブラシの切り替え)。
//! 部品の外のタッチは、これまでどおり 2 本指のカメラ操作とブラシになる。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

/// 1 フレームぶんの操作 (タッチ・ゲームパッド共通)。`aim` は 1/16 セル単位の向き (長さは意味を持たない)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PadOutput {
    pub horizontal: i8,
    pub jump: bool,
    /// 左手の武器。
    pub fire: bool,
    /// 右手の道具 (ワイヤーフック)。
    pub tool: bool,
    pub aim: Option<(i32, i32)>,
    /// このフレームに、武器 / ブラシの切り替えが押された。
    pub toggle_paint: bool,
}

/// スワイプとみなす、上向きのずれ (スティックの半径に対する割合)。
const SWIPE_UP: f32 = 0.7;
/// 歩き始めるずれ (スティックの半径に対する割合)。
const WALK_ZONE: f32 = 0.3;
/// 狙いが変わる最小のずれ (スティックの半径に対する割合)。
const AIM_ZONE: f32 = 0.25;
/// 狙いの既定の向き (右上)。一度も狙っていないとき、`HOOK` がこの向きへ撃つ。
const DEFAULT_AIM: (i32, i32) = (1000, -400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Stick,
    Aim,
    Jump,
    Hook,
    Paint,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    owner: Owner,
    start: [f32; 2],
    now: [f32; 2],
}

#[derive(Debug, Default)]
pub(crate) struct TouchPad {
    viewport: [f32; 2],
    visible: bool,
    touches: BTreeMap<u64, Touch>,
    last_aim: Option<(i32, i32)>,
    toggle: bool,
}

/// 画面の大きさ (ピクセル) から決まる配置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// ボタンの半径。スティックの半径は 3 倍。
    pub unit: f32,
    pub stick_home: [f32; 2],
    pub aim_home: [f32; 2],
    pub jump: [f32; 2],
    pub hook: [f32; 2],
    pub paint: [f32; 2],
    /// 狙いスティックの領域の左端と右端 (x)。
    aim_zone: [f32; 2],
    /// 2 つのスティックの領域の上端 (y)。
    zone_top: f32,
    /// 移動スティックの領域の右端 (x)。
    stick_right: f32,
}

impl Layout {
    pub(crate) fn new(viewport: [f32; 2]) -> Self {
        let (w, h) = (viewport[0], viewport[1]);
        let unit = (w.min(h) * 0.07).max(20.0);
        Self {
            unit,
            stick_home: [3.4 * unit, h - 3.4 * unit],
            aim_home: [w - 7.2 * unit, h - 3.4 * unit],
            jump: [w - 1.7 * unit, h - 1.8 * unit],
            hook: [w - 1.7 * unit, h - 4.5 * unit],
            paint: [w - 1.7 * unit, h - 7.2 * unit],
            aim_zone: [w * 0.45, w - 3.6 * unit],
            zone_top: h * 0.4,
            stick_right: w * 0.4,
        }
    }

    /// スティックの半径 (ずらせる最大の距離)。
    pub(crate) fn radius(&self) -> f32 {
        self.unit * 3.0
    }

    fn owner_at(&self, pos: [f32; 2]) -> Option<Owner> {
        let near =
            |center: [f32; 2]| (pos[0] - center[0]).hypot(pos[1] - center[1]) <= self.unit * 1.3;
        if near(self.jump) {
            return Some(Owner::Jump);
        }
        if near(self.hook) {
            return Some(Owner::Hook);
        }
        if near(self.paint) {
            return Some(Owner::Paint);
        }
        if pos[1] < self.zone_top {
            return None;
        }
        if pos[0] < self.stick_right {
            return Some(Owner::Stick);
        }
        if pos[0] >= self.aim_zone[0] && pos[0] < self.aim_zone[1] {
            return Some(Owner::Aim);
        }
        None
    }
}

impl TouchPad {
    pub(crate) fn set_viewport(&mut self, viewport: [f32; 2]) {
        self.viewport = viewport;
    }

    /// 画面に出すか。最初のタッチ (または Android) で出す。
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn show(&mut self) {
        self.visible = true;
    }

    pub(crate) fn layout(&self) -> Layout {
        Layout::new(self.viewport)
    }

    /// タッチのイベントを反映する。この指が部品のものなら true (カメラ操作やブラシに渡さない)。
    pub(crate) fn update(&mut self, id: u64, phase: TouchPhase, pos: [f32; 2]) -> bool {
        match phase {
            TouchPhase::Started => {
                self.visible = true;
                let Some(owner) = self.layout().owner_at(pos) else { return false };
                if owner == Owner::Paint {
                    self.toggle = true;
                }
                self.touches.insert(id, Touch { owner, start: pos, now: pos });
                true
            }
            TouchPhase::Moved => match self.touches.get_mut(&id) {
                Some(touch) => {
                    touch.now = pos;
                    true
                }
                None => false,
            },
            TouchPhase::Ended | TouchPhase::Cancelled => self.touches.remove(&id).is_some(),
        }
    }

    /// 全部の指を離した状態に戻す (ウィンドウがフォーカスを失ったとき)。
    pub(crate) fn release_all(&mut self) {
        self.touches.clear();
        self.toggle = false;
    }

    /// 指が触れている部品と、その押し込み (描画用)。`(部品の中心, 指の位置)`。
    pub(crate) fn stick_touch(&self) -> Option<([f32; 2], [f32; 2])> {
        self.touches.values().find(|t| t.owner == Owner::Stick).map(|t| (t.start, t.now))
    }

    pub(crate) fn aim_touch(&self) -> Option<([f32; 2], [f32; 2])> {
        self.touches.values().find(|t| t.owner == Owner::Aim).map(|t| (t.start, t.now))
    }

    pub(crate) fn held(&self, button: &str) -> bool {
        let owner = match button {
            "jump" => Owner::Jump,
            "hook" => Owner::Hook,
            _ => Owner::Paint,
        };
        self.touches.values().any(|t| t.owner == owner)
    }

    /// 今の操作。切り替えは 1 度だけ返す。
    pub(crate) fn output(&mut self) -> PadOutput {
        let radius = self.layout().radius();
        let mut out = PadOutput::default();
        for touch in self.touches.values() {
            let (dx, dy) = (touch.now[0] - touch.start[0], touch.now[1] - touch.start[1]);
            match touch.owner {
                Owner::Stick => {
                    if dx.abs() > radius * WALK_ZONE {
                        out.horizontal = if dx > 0.0 { 1 } else { -1 };
                    }
                    if dy < -radius * SWIPE_UP {
                        out.jump = true;
                    }
                }
                Owner::Aim => {
                    out.fire = true;
                    let length = dx.hypot(dy);
                    if length > radius * AIM_ZONE {
                        let aim = ((dx / length * 1000.0) as i32, (dy / length * 1000.0) as i32);
                        self.last_aim = Some(aim);
                    }
                }
                Owner::Jump => out.jump = true,
                Owner::Hook => out.tool = true,
                Owner::Paint => {}
            }
        }
        if out.fire || out.tool {
            out.aim = Some(self.last_aim.unwrap_or(DEFAULT_AIM));
        }
        out.toggle_paint = std::mem::take(&mut self.toggle);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: [f32; 2] = [1600.0, 900.0];

    fn pad() -> TouchPad {
        let mut pad = TouchPad::default();
        pad.set_viewport(VIEW);
        pad
    }

    #[test]
    fn stick_walks_and_swipe_up_jumps() {
        let mut pad = pad();
        let layout = pad.layout();
        let home = layout.stick_home;
        assert!(pad.update(1, TouchPhase::Started, home));
        assert_eq!(pad.output(), PadOutput::default(), "触れただけでは動かない");
        let r = layout.radius();
        assert!(pad.update(1, TouchPhase::Moved, [home[0] + r, home[1]]));
        assert_eq!(pad.output().horizontal, 1);
        pad.update(1, TouchPhase::Moved, [home[0] - r, home[1]]);
        assert_eq!(pad.output().horizontal, -1);
        // 上へスワイプするとジャンプ。
        pad.update(1, TouchPhase::Moved, [home[0], home[1] - r]);
        let up = pad.output();
        assert!(up.jump && up.horizontal == 0);
        assert!(pad.update(1, TouchPhase::Ended, home));
        assert_eq!(pad.output(), PadOutput::default(), "離したら止まる");
    }

    #[test]
    fn aim_stick_fires_toward_the_drag() {
        let mut pad = pad();
        let layout = pad.layout();
        let home = layout.aim_home;
        assert!(pad.update(2, TouchPhase::Started, home));
        let touching = pad.output();
        assert!(touching.fire, "触れている間は撃つ");
        assert_eq!(touching.aim, Some(DEFAULT_AIM), "まだ狙っていないので既定の向き");
        let r = layout.radius();
        pad.update(2, TouchPhase::Moved, [home[0] - r, home[1]]);
        let aimed = pad.output();
        assert_eq!(aimed.aim, Some((-1000, 0)));
        // 離すと撃つのをやめ、次の `HOOK` は最後の狙いの向きへ撃つ。
        pad.update(2, TouchPhase::Ended, home);
        assert!(!pad.output().fire);
        assert!(pad.update(3, TouchPhase::Started, layout.hook));
        let hook = pad.output();
        assert!(hook.tool && !hook.fire);
        assert_eq!(hook.aim, Some((-1000, 0)));
    }

    #[test]
    fn buttons_hold_and_paint_toggles_once() {
        let mut pad = pad();
        let layout = pad.layout();
        assert!(pad.update(4, TouchPhase::Started, layout.jump));
        assert!(pad.output().jump && pad.held("jump"));
        assert!(pad.update(5, TouchPhase::Started, layout.paint));
        assert!(pad.output().toggle_paint);
        assert!(!pad.output().toggle_paint, "切り替えは 1 度だけ");
        pad.update(4, TouchPhase::Cancelled, layout.jump);
        assert!(!pad.output().jump);
    }

    #[test]
    fn touches_outside_the_pad_are_left_to_the_camera_and_brush() {
        let mut pad = pad();
        // 画面の上半分と、2 つのスティックの間は部品ではない。
        assert!(!pad.update(6, TouchPhase::Started, [800.0, 100.0]));
        assert!(!pad.update(7, TouchPhase::Started, [700.0, 800.0]));
        assert!(!pad.update(6, TouchPhase::Moved, [810.0, 110.0]));
        assert!(!pad.update(6, TouchPhase::Ended, [810.0, 110.0]));
        assert_eq!(pad.output(), PadOutput::default());
        assert!(pad.visible(), "タッチがあれば表示する");
    }
}
