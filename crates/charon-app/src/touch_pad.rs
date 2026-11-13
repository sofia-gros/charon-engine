//! 仕様 10 §11: 画面のタッチ操作 (Terraria 型)。ゲームパッドと同じ `PadOutput` を作る。
//!
//! - 左下の 1/4 に、**浮動の移動パッド** (触れた場所が中心。横にずらすと歩く、上へずらすとジャンプ)。
//! - **ポインター**: 1 本指で、パッドとボタンの外をなぞると、プレイヤーを中心にした円の中でポインターが動く。
//!   指のずれ (画素) ÷ 拡大率 (セル) だけ動き、指を離しても残る。2 本指になったら、ポインターは動かさない (カメラ操作)。
//! - 右下の**ボタン**: `USE` (武器 / ブラシ)・`HOOK` (右手の道具)・`DIG` (掘る道具)・`JUMP`・`PAINT` (武器 / ブラシの切り替え)。
//!   `USE`・`HOOK`・`DIG` は、ポインターのある方へ使う。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

/// 1 フレームぶんの操作 (タッチ・ゲームパッド共通)。`aim` は 1/16 セル単位の向き (長さは意味を持たない)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PadOutput {
    pub horizontal: i8,
    pub jump: bool,
    /// 左手の武器 (ゲームパッド)。
    pub fire: bool,
    /// 右手の道具 (ワイヤーフック)。
    pub tool: bool,
    /// 掘る道具 (仕様 10 §12)。
    pub dig: bool,
    /// `USE` ボタン: 武器モードでは武器、描画モードではブラシを、ポインターの場所へ使う。
    pub use_item: bool,
    pub aim: Option<(i32, i32)>,
    /// このフレームに、武器 / ブラシの切り替えが押された。
    pub toggle_paint: bool,
}

impl PadOutput {
    /// 2 つの入力 (タッチとゲームパッド) を重ねる。歩く向きは動いている方、狙いは `other` を優先する。
    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            horizontal: if other.horizontal != 0 { other.horizontal } else { self.horizontal },
            jump: self.jump || other.jump,
            fire: self.fire || other.fire,
            tool: self.tool || other.tool,
            dig: self.dig || other.dig,
            use_item: self.use_item || other.use_item,
            aim: other.aim.or(self.aim),
            toggle_paint: self.toggle_paint || other.toggle_paint,
        }
    }
}

/// ジャンプとみなす、上向きのずれ (パッドの半径に対する割合)。
const SWIPE_UP: f32 = 0.7;
/// 歩き始めるずれ (パッドの半径に対する割合)。
const WALK_ZONE: f32 = 0.3;
/// 狙いの既定の向き (右上)。ポインターをまだ動かしていないとき、ボタンがこの向きへ使う。
const DEFAULT_AIM: (i32, i32) = (1000, -400);
/// ポインターがこれより近いとき (セル) は、向きを決められないので既定の向きにする。
const AIM_MIN: f32 = 0.5;
/// ポインターが動ける範囲 (画面の短い辺に対する割合)。
const POINTER_REACH: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Stick,
    Jump,
    Hook,
    Dig,
    Paint,
    Use,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    owner: Owner,
    start: [f32; 2],
    now: [f32; 2],
}

/// ポインターを動かしている指。
#[derive(Clone, Copy, Debug)]
struct PointerTouch {
    id: u64,
    start: [f32; 2],
    /// 触れたときのポインターの位置 (プレイヤーからのセル)。
    base: [f32; 2],
}

#[derive(Debug, Default)]
pub(crate) struct TouchPad {
    viewport: [f32; 2],
    visible: bool,
    touches: BTreeMap<u64, Touch>,
    toggle: bool,
    /// 部品全体の大きさの倍率 (0 は 1 とみなす)。
    scale: f32,
    /// ポインターの位置 (プレイヤーの中心からのセル)。まだ動かしていなければ `None`。
    pointer: Option<[f32; 2]>,
    pointer_touch: Option<PointerTouch>,
    /// 描画用: 拡大率と、プレイヤーの中心の画面の位置 (画素)。
    zoom: f32,
    player_screen: Option<[f32; 2]>,
}

/// 画面の大きさ (ピクセル) から決まる配置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// ボタンの半径。
    pub unit: f32,
    /// 移動パッドの領域 (左下の 1/4): `[左, 上, 右, 下]`。
    pub stick_zone: [f32; 4],
    /// 移動パッドの描く半径 (ずらせる最大の距離)。
    pub stick_radius: f32,
    pub use_button: [f32; 2],
    pub jump: [f32; 2],
    pub hook: [f32; 2],
    pub dig: [f32; 2],
    pub paint: [f32; 2],
}

impl Layout {
    /// `scale` は部品全体の大きさの倍率 (UI の大きさの設定)。0 以下・有限でない値は 1 とみなす。
    pub(crate) fn new(viewport: [f32; 2], scale: f32) -> Self {
        let (w, h) = (viewport[0], viewport[1]);
        let scale = if scale.is_finite() && scale > 0.0 { scale.clamp(0.5, 3.0) } else { 1.0 };
        let unit = (w.min(h) * 0.07).max(20.0) * scale;
        Self {
            unit,
            stick_zone: [0.0, h * 0.5, w * 0.5, h],
            stick_radius: unit * 3.0,
            use_button: [w - 2.2 * unit, h - 2.2 * unit],
            jump: [w - 5.6 * unit, h - 1.8 * unit],
            hook: [w - 2.0 * unit, h - 5.6 * unit],
            dig: [w - 5.2 * unit, h - 5.2 * unit],
            paint: [w - 2.0 * unit, h - 8.8 * unit],
        }
    }

    /// 移動パッドのずらせる最大の距離。
    pub(crate) fn radius(&self) -> f32 {
        self.stick_radius
    }

    /// 何も触れていないときの、移動パッドの目印の位置 (領域の下寄りの左)。
    pub(crate) fn stick_home(&self) -> [f32; 2] {
        [3.4 * self.unit, self.stick_zone[3] - 3.4 * self.unit]
    }

    fn owner_at(&self, pos: [f32; 2]) -> Option<Owner> {
        let near = |center: [f32; 2], reach: f32| {
            (pos[0] - center[0]).hypot(pos[1] - center[1]) <= self.unit * reach
        };
        if near(self.use_button, 1.7) {
            return Some(Owner::Use);
        }
        if near(self.jump, 1.3) {
            return Some(Owner::Jump);
        }
        if near(self.hook, 1.3) {
            return Some(Owner::Hook);
        }
        if near(self.dig, 1.3) {
            return Some(Owner::Dig);
        }
        if near(self.paint, 1.3) {
            return Some(Owner::Paint);
        }
        let [left, top, right, bottom] = self.stick_zone;
        (pos[0] >= left && pos[0] < right && pos[1] >= top && pos[1] <= bottom)
            .then_some(Owner::Stick)
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
        Layout::new(self.viewport, self.scale)
    }

    /// 部品全体の大きさの倍率 (UI の大きさ)。
    pub(crate) fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// 描画のために、拡大率とプレイヤーの中心の画面の位置 (画素) を知らせる。
    pub(crate) fn set_view(&mut self, zoom: f32, player_screen: Option<[f32; 2]>) {
        self.zoom = zoom;
        self.player_screen = player_screen;
    }

    /// ポインターが動ける半径 (セル)。
    pub(crate) fn pointer_radius(&self, zoom: f32) -> f32 {
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        self.viewport[0].min(self.viewport[1]) * POINTER_REACH / zoom
    }

    /// ポインターの位置 (プレイヤーの中心からのセル)。まだ動かしていなければ `None`。
    pub(crate) fn pointer_offset(&self) -> Option<[f32; 2]> {
        self.pointer
    }

    /// ポインターの画面の位置 (画素、描画用)。
    pub(crate) fn pointer_screen(&self) -> Option<[f32; 2]> {
        let (offset, player) = (self.pointer?, self.player_screen?);
        Some([player[0] + offset[0] * self.zoom, player[1] + offset[1] * self.zoom])
    }

    /// ポインターを動かしている指があるか。
    pub(crate) fn pointer_active(&self) -> bool {
        self.pointer_touch.is_some()
    }

    /// ポインターへの向き (1/16 セル単位のベクトル、長さ 1000)。近すぎる・まだ動かしていないときは既定の向き。
    pub(crate) fn aim(&self) -> (i32, i32) {
        let Some([x, y]) = self.pointer else { return DEFAULT_AIM };
        let length = x.hypot(y);
        if length < AIM_MIN {
            return DEFAULT_AIM;
        }
        ((x / length * 1000.0) as i32, (y / length * 1000.0) as i32)
    }

    /// 1 本指のポインター操作。部品 (パッド・ボタン) の指は渡さない。`camera_gesture` が真 (2 本指以上) なら動かさない。
    pub(crate) fn pointer_update(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: [f32; 2],
        zoom: f32,
        camera_gesture: bool,
    ) {
        if !pos[0].is_finite() || !pos[1].is_finite() {
            return;
        }
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        if camera_gesture {
            self.pointer_cancel();
            return;
        }
        match phase {
            TouchPhase::Started => {
                if self.pointer_touch.is_none() {
                    self.visible = true;
                    let base = self.pointer.unwrap_or([0.0, 0.0]);
                    self.pointer_touch = Some(PointerTouch { id, start: pos, base });
                }
            }
            TouchPhase::Moved => {
                let Some(touch) = self.pointer_touch.filter(|t| t.id == id) else { return };
                let reach = self.pointer_radius(zoom);
                let wanted = [
                    touch.base[0] + (pos[0] - touch.start[0]) / zoom,
                    touch.base[1] + (pos[1] - touch.start[1]) / zoom,
                ];
                let length = wanted[0].hypot(wanted[1]);
                self.pointer = Some(if length > reach {
                    [wanted[0] / length * reach, wanted[1] / length * reach]
                } else {
                    wanted
                });
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.pointer_touch.is_some_and(|t| t.id == id) {
                    self.pointer_touch = None;
                }
            }
        }
    }

    /// ポインターを動かしている指をやめる (2 本指になったとき)。ポインターはその指に触れる前の位置にもどる。
    pub(crate) fn pointer_cancel(&mut self) {
        if let Some(touch) = self.pointer_touch.take() {
            self.pointer = (touch.base != [0.0, 0.0]).then_some(touch.base);
        }
    }

    /// タッチのイベントを反映する。この指が部品 (移動パッド・ボタン) のものなら true。
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
        self.pointer_touch = None;
        self.toggle = false;
    }

    /// 移動パッドに触れている指と、その押し込み (描画用)。`(触れた場所 = 中心, 指の位置)`。
    pub(crate) fn stick_touch(&self) -> Option<([f32; 2], [f32; 2])> {
        self.touches.values().find(|t| t.owner == Owner::Stick).map(|t| (t.start, t.now))
    }

    pub(crate) fn held(&self, button: &str) -> bool {
        let owner = match button {
            "use" => Owner::Use,
            "jump" => Owner::Jump,
            "hook" => Owner::Hook,
            "dig" => Owner::Dig,
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
                Owner::Jump => out.jump = true,
                Owner::Hook => out.tool = true,
                Owner::Dig => out.dig = true,
                Owner::Use => out.use_item = true,
                Owner::Paint => {}
            }
        }
        if out.use_item || out.tool || out.dig {
            out.aim = Some(self.aim());
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

    /// 移動パッドは左下の 1/4 のどこに触れても、触れた場所が中心になる。横で歩き、上へずらすとジャンプ。
    #[test]
    fn floating_stick_walks_and_swipe_up_jumps() {
        let mut pad = pad();
        let layout = pad.layout();
        let r = layout.radius();
        let start = [300.0, 600.0];
        assert!(pad.update(1, TouchPhase::Started, start));
        assert_eq!(pad.output(), PadOutput::default(), "触れただけでは動かない");
        assert_eq!(pad.stick_touch(), Some((start, start)), "触れた場所が中心");
        pad.update(1, TouchPhase::Moved, [start[0] + r, start[1]]);
        assert_eq!(pad.output().horizontal, 1);
        pad.update(1, TouchPhase::Moved, [start[0] - r, start[1]]);
        assert_eq!(pad.output().horizontal, -1);
        pad.update(1, TouchPhase::Moved, [start[0], start[1] - r]);
        let up = pad.output();
        assert!(up.jump && up.horizontal == 0);
        assert!(pad.update(1, TouchPhase::Ended, start));
        assert_eq!(pad.output(), PadOutput::default(), "離したら止まる");
    }

    /// 領域は画面の左下の 1/4 だけ。右半分や上半分は、パッドではない (ポインター・カメラの指)。
    #[test]
    fn the_stick_zone_is_the_lower_left_quarter() {
        let mut pad = pad();
        assert!(pad.update(1, TouchPhase::Started, [799.0, 451.0]), "角の内側");
        assert!(!pad.update(2, TouchPhase::Started, [801.0, 700.0]), "右半分");
        assert!(!pad.update(3, TouchPhase::Started, [400.0, 449.0]), "上半分");
        assert!(!pad.update(4, TouchPhase::Started, [800.0, 100.0]));
        assert!(!pad.update(2, TouchPhase::Moved, [810.0, 710.0]));
        assert!(!pad.update(2, TouchPhase::Ended, [810.0, 710.0]));
        assert!(pad.visible(), "タッチがあれば表示する");
    }

    /// 1 本指でなぞると、ずれ ÷ 拡大率 (セル) だけポインターが動く。範囲は円で、離しても残り、次のなぞりはそこから続く。
    #[test]
    fn the_pointer_moves_relative_to_the_drag_inside_a_circle_and_stays() {
        let mut pad = pad();
        let zoom = 4.0;
        assert_eq!(pad.pointer_offset(), None);
        pad.pointer_update(1, TouchPhase::Started, [1000.0, 300.0], zoom, false);
        assert!(pad.pointer_active());
        pad.pointer_update(1, TouchPhase::Moved, [1040.0, 280.0], zoom, false);
        assert_eq!(pad.pointer_offset(), Some([10.0, -5.0]), "40 画素 ÷ 4 = 10 セル");
        pad.pointer_update(1, TouchPhase::Ended, [1040.0, 280.0], zoom, false);
        assert!(!pad.pointer_active());
        assert_eq!(pad.pointer_offset(), Some([10.0, -5.0]), "離しても残る");
        // 次のなぞりは、残った位置から。
        pad.pointer_update(2, TouchPhase::Started, [500.0, 300.0], zoom, false);
        pad.pointer_update(2, TouchPhase::Moved, [460.0, 300.0], zoom, false);
        assert_eq!(pad.pointer_offset(), Some([0.0, -5.0]));
        // 遠くまでなぞっても、円の中 (半径) で止まる。
        pad.pointer_update(2, TouchPhase::Moved, [-5000.0, 3000.0], zoom, false);
        let [x, y] = pad.pointer_offset().unwrap();
        let reach = pad.pointer_radius(zoom);
        assert!((x.hypot(y) - reach).abs() < 1e-3, "半径 {reach} の円の縁");
        assert!(x < 0.0 && y > 0.0, "向きは保たれる");
        // 拡大率が違えば、同じ指のずれでも、動くセルの数が違う。
        let mut zoomed = self::pad();
        zoomed.pointer_update(1, TouchPhase::Started, [1000.0, 300.0], 8.0, false);
        zoomed.pointer_update(1, TouchPhase::Moved, [1040.0, 300.0], 8.0, false);
        assert_eq!(zoomed.pointer_offset(), Some([5.0, 0.0]));
    }

    /// 2 本指 (カメラ操作) になったら、ポインターを動かさず、その前の位置にもどる。不正な値は無視する。
    #[test]
    fn a_second_finger_cancels_the_pointer_and_bad_values_are_ignored() {
        let mut pad = pad();
        pad.pointer_update(1, TouchPhase::Started, [1000.0, 300.0], 4.0, false);
        pad.pointer_update(1, TouchPhase::Moved, [1040.0, 300.0], 4.0, false);
        pad.pointer_update(1, TouchPhase::Ended, [1040.0, 300.0], 4.0, false);
        pad.pointer_update(2, TouchPhase::Started, [1000.0, 300.0], 4.0, false);
        pad.pointer_update(2, TouchPhase::Moved, [1100.0, 300.0], 4.0, false);
        assert_eq!(pad.pointer_offset(), Some([35.0, 0.0]));
        pad.pointer_update(2, TouchPhase::Moved, [1100.0, 300.0], 4.0, true);
        assert_eq!(pad.pointer_offset(), Some([10.0, 0.0]), "なぞる前にもどる");
        assert!(!pad.pointer_active());
        pad.pointer_update(3, TouchPhase::Started, [f32::NAN, 0.0], 4.0, false);
        assert!(!pad.pointer_active(), "NaN は無視する");
        pad.pointer_update(4, TouchPhase::Started, [10.0, 10.0], f32::NAN, false);
        pad.pointer_update(4, TouchPhase::Moved, [14.0, 10.0], f32::NAN, false);
        assert_eq!(pad.pointer_offset(), Some([14.0, 0.0]), "拡大率が不正なら 1 とみなす");
    }

    /// ボタンは、ポインターのある方へ使う。まだ動かしていなければ既定の向き。押している間、押し続ける。
    #[test]
    fn buttons_use_toward_the_pointer_and_paint_toggles_once() {
        let mut pad = pad();
        let layout = pad.layout();
        assert!(pad.update(1, TouchPhase::Started, layout.use_button));
        let used = pad.output();
        assert!(used.use_item && !used.fire && used.aim == Some(DEFAULT_AIM), "{used:?}");
        // ポインターを左へ動かすと、向きが変わる。
        pad.pointer_update(9, TouchPhase::Started, [1000.0, 300.0], 4.0, false);
        pad.pointer_update(9, TouchPhase::Moved, [900.0, 300.0], 4.0, false);
        assert_eq!(pad.output().aim, Some((-1000, 0)));
        pad.update(1, TouchPhase::Ended, layout.use_button);
        assert!(!pad.output().use_item && pad.output().aim.is_none());
        assert!(pad.update(2, TouchPhase::Started, layout.hook));
        let hook = pad.output();
        assert!(hook.tool && !hook.use_item);
        assert_eq!(hook.aim, Some((-1000, 0)));
        assert!(pad.update(3, TouchPhase::Started, layout.dig));
        assert!(pad.output().dig && pad.held("dig") && pad.held("hook"));
        assert!(pad.update(4, TouchPhase::Started, layout.jump));
        assert!(pad.output().jump);
        assert!(pad.update(5, TouchPhase::Started, layout.paint));
        assert!(pad.output().toggle_paint);
        assert!(!pad.output().toggle_paint, "切り替えは 1 度だけ");
        pad.update(4, TouchPhase::Cancelled, layout.jump);
        assert!(!pad.output().jump);
    }

    /// ポインターが真上・真下・近すぎるときの向きと、描画用の位置。
    #[test]
    fn aim_and_screen_position_follow_the_pointer() {
        let mut pad = pad();
        assert_eq!(pad.aim(), DEFAULT_AIM);
        pad.set_view(4.0, Some([800.0, 450.0]));
        assert_eq!(pad.pointer_screen(), None, "動かすまで出さない");
        pad.pointer_update(1, TouchPhase::Started, [100.0, 100.0], 4.0, false);
        pad.pointer_update(1, TouchPhase::Moved, [100.0, 60.0], 4.0, false);
        assert_eq!(pad.aim(), (0, -1000), "真上");
        assert_eq!(
            pad.pointer_screen(),
            Some([800.0, 450.0 - 40.0]),
            "10 セル × 拡大率 4 = 40 画素"
        );
        pad.pointer_update(1, TouchPhase::Moved, [100.0, 100.0], 4.0, false);
        assert_eq!(pad.aim(), DEFAULT_AIM, "近すぎるときは既定の向き");
        pad.set_view(4.0, None);
        assert_eq!(pad.pointer_screen(), None, "プレイヤーがいなければ出さない");
    }

    /// UI の大きさの倍率で、ボタンの半径とパッドの半径が変わる。
    #[test]
    fn scale_resizes_the_pad() {
        let mut pad = pad();
        let normal = pad.layout();
        pad.set_scale(2.0);
        let big = pad.layout();
        assert!((big.unit - normal.unit * 2.0).abs() < 1e-3);
        assert!((big.radius() - normal.radius() * 2.0).abs() < 1e-3);
        pad.set_scale(0.0);
        assert_eq!(pad.layout(), normal, "0 は 1 とみなす");
        pad.set_scale(f32::NAN);
        assert_eq!(pad.layout(), normal, "有限でない値は 1 とみなす");
    }

    /// ボタンは右下に並び、画面の外へ出ず、互いに重ならない。
    #[test]
    fn buttons_sit_in_the_lower_right_without_overlapping() {
        for view in [[1600.0, 900.0], [800.0, 400.0], [2400.0, 1080.0]] {
            let layout = Layout::new(view, 1.0);
            let buttons = [layout.use_button, layout.jump, layout.hook, layout.dig, layout.paint];
            for (i, a) in buttons.iter().enumerate() {
                assert!(a[0] > view[0] * 0.5 && a[1] > 0.0 && a[0] < view[0] && a[1] < view[1]);
                for b in &buttons[i + 1..] {
                    assert!((a[0] - b[0]).hypot(a[1] - b[1]) > layout.unit * 1.5, "{view:?}");
                }
            }
        }
    }
}
