//! 仕様 18: 画面のタッチ操作 (Terraria Android 型)。ゲームパッドと同じ `PadOutput` を作る。
//!
//! - 左下の隅に**固定の左スティック** (横で歩く。上へ倒してもジャンプしない)。
//! - 右下の**使うボタン**: 大きな円で、**スティック**として動く。倒したずれでカーソルが動き、触れている間、選んでいるアイテムを使う。
//! - 使うボタンの左隣の**フックのスティック**: 傾けて離した向きへ、フックを飛ばす。傾けない (タップ) と、切り離す。
//! - **ジャンプ** (`^^`)・**ズーム** (拡大・縮小)・**インベントリ**の格子・**スマートカーソルの切り替え** (`+`)・**ホットバー** (横一列)。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

/// 1 フレームぶんの操作 (タッチ・ゲームパッド共通)。`aim` は 1/16 セル単位の向き (長さは意味を持たない)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PadOutput {
    pub horizontal: i8,
    pub jump: bool,
    /// 左手の武器 (ゲームパッド)。
    pub fire: bool,
    /// 右手の道具 (ワイヤーフック)。
    pub tool: bool,
    /// 掘る道具 (仕様 10 §12)。
    pub dig: bool,
    /// 使うボタンに触れている: 選んでいるアイテムを、カーソルの場所へ使う。
    pub use_item: bool,
    pub aim: Option<(i32, i32)>,
    /// このフレームに、武器 / ブラシの切り替えが押された (ゲームパッド)。
    pub toggle_paint: bool,
    /// ズームのボタン: 1 = 拡大、-1 = 縮小 (押している間)。
    pub zoom: i8,
    /// 使うボタンのつまみの倒れ (半径の千分率、-1000..=1000)。使うボタンに触れている間だけ。
    pub cursor_tilt: Option<(i32, i32)>,
    /// フックのスティックを離した: 離した向きへ飛ばす (このフレームだけ)。
    pub hook_throw: Option<(i32, i32)>,
    /// フックのスティックを傾けずに離した (タップ): 切り離す (このフレームだけ)。
    pub hook_detach: bool,
    /// フックのスティックを傾けている間の向き (矢印の描画用)。
    pub hook_aim: Option<(i32, i32)>,
    /// ホットバーの枠がタップされた (このフレームだけ)。
    pub select_slot: Option<u8>,
    /// インベントリの格子が押された (このフレームだけ)。
    pub toggle_inventory: bool,
    /// 世界を直接さわっている 1 本の指の位置 (画面の画素、仕様 28 §5)。使うボタンの指があるとき・ピンチ中は `None`。
    pub world_touch: Option<[f32; 2]>,
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
            zoom: if other.zoom != 0 { other.zoom } else { self.zoom },
            cursor_tilt: other.cursor_tilt.or(self.cursor_tilt),
            hook_throw: other.hook_throw.or(self.hook_throw),
            hook_detach: self.hook_detach || other.hook_detach,
            hook_aim: other.hook_aim.or(self.hook_aim),
            select_slot: other.select_slot.or(self.select_slot),
            toggle_inventory: self.toggle_inventory || other.toggle_inventory,
            world_touch: other.world_touch.or(self.world_touch),
        }
    }
}

/// 歩き始める倒れ (スティックの半径に対する割合)。
const WALK_ZONE: f32 = 0.3;
/// フックのスティックを「傾けた」とみなす倒れ (半径に対する割合)。
const HOOK_TILT: f32 = 0.3;
/// 狙いの既定の向き (右上)。使うボタンをまだ倒していないとき。
const DEFAULT_AIM: (i32, i32) = (1000, -400);
/// ホットバーに見える枠の数 (残りは `...` からインベントリで)。
pub(crate) const HOTBAR_VISIBLE: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Stick,
    Use,
    Hook,
    Jump,
    ZoomIn,
    ZoomOut,
    Inventory,
    Smart,
    Slot(u8),
    More,
    /// どの部品にも当たらない指: 世界を直接さわる (仕様 28 §5)。
    World,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    owner: Owner,
    now: [f32; 2],
    /// 世界の指が 2 本以上になった: ピンチなので、アイテムを使わない。1 本に戻っても、離すまで使わない。
    no_use: bool,
}

/// 世界の指の本数の上限。これを超えて新しく触れたら、古い指は離したイベントを取りこぼしたものとして捨てる。
const MAX_WORLD_FINGERS: usize = 2;
/// 1 回の動きで変わる、ピンチの拡大率の範囲。
const PINCH_STEP: (f32, f32) = (0.5, 2.0);

#[derive(Debug, Default)]
pub(crate) struct TouchPad {
    viewport: [f32; 2],
    visible: bool,
    touches: BTreeMap<u64, Touch>,
    /// 部品全体の大きさの倍率 (0 は 1 とみなす)。
    scale: f32,
    /// スマートカーソルが入っているか。
    smart: bool,
    /// このフレームだけ立つ出来事。
    slot_tap: Option<u8>,
    inventory_tap: bool,
    hook_release: Option<HookRelease>,
    /// 使うボタンを最後に倒した向き (離してもカーソルの向きとして残る)。
    last_aim: Option<(i32, i32)>,
    /// 描画用のカーソルの枠。
    cursor: Option<CursorView>,
    /// 世界の指が 2 本あるときの、直前の距離。
    pinch_distance: Option<f32>,
    /// まだ使われていない、ピンチの拡大率 (積)。1 = 変化なし。
    pinch_ratio: f32,
}

/// カーソルの枠 (画面の画素)と状態。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CursorView {
    /// `[左, 上, 右, 下]`。
    pub rect: [f32; 4],
    pub state: charon_game::cursor::CursorState,
}

#[derive(Clone, Copy, Debug)]
struct HookRelease {
    tilt: [f32; 2],
}

/// 画面の大きさ (ピクセル) から決まる配置 (仕様 18 §1)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// 基準の長さ `u`。
    pub unit: f32,
    /// 左スティックの中心と半径 (`3u`)。
    pub stick: [f32; 2],
    pub stick_radius: f32,
    /// 使うボタン (中心、半径 `2.2u`)。
    pub use_button: [f32; 2],
    pub use_radius: f32,
    /// フックのスティック (中心、半径 `1.1u`)。
    pub hook: [f32; 2],
    pub hook_radius: f32,
    pub jump: [f32; 2],
    pub jump_radius: f32,
    /// ズームのボタン。半径は `0.8u`。
    pub zoom_in: [f32; 2],
    pub zoom_out: [f32; 2],
    /// インベントリの格子と、スマートカーソルの `+`。半径は `0.9u`。
    pub inventory: [f32; 2],
    pub smart: [f32; 2],
}

impl Layout {
    /// `scale` は部品全体の大きさの倍率 (UI の大きさの設定)。0 以下・有限でない値は 1 とみなす。
    pub(crate) fn new(viewport: [f32; 2], scale: f32) -> Self {
        let (w, h) = (viewport[0], viewport[1]);
        let scale = if scale.is_finite() && scale > 0.0 { scale.clamp(0.5, 3.0) } else { 1.0 };
        let unit = (w.min(h) * 0.07).max(20.0) * scale;
        let use_button = [w - 2.6 * unit, h - 2.6 * unit];
        Self {
            unit,
            stick: [3.6 * unit, h - 3.6 * unit],
            stick_radius: 3.0 * unit,
            use_button,
            use_radius: 2.2 * unit,
            hook: [use_button[0] - 3.6 * unit, use_button[1] + 0.3 * unit],
            hook_radius: 1.1 * unit,
            jump: [w - 1.5 * unit, h - 6.2 * unit],
            jump_radius: 1.2 * unit,
            zoom_out: [w * 0.5 - 1.2 * unit, h - 1.2 * unit],
            zoom_in: [w * 0.5 + 1.2 * unit, h - 1.2 * unit],
            inventory: [1.2 * unit, h - 8.0 * unit],
            smart: [3.4 * unit, h - 8.0 * unit],
        }
    }

    /// 左スティックのずらせる最大の距離。
    pub(crate) fn radius(&self) -> f32 {
        self.stick_radius
    }

    /// ホットバーの枠 `slot` (`0..HOTBAR_VISIBLE`) の矩形 `[左, 上, 右, 下]`。`slot == HOTBAR_VISIBLE` は `...`。
    pub(crate) fn slot_rect(&self, slot: usize) -> [f32; 4] {
        let size = 1.5 * self.unit;
        let left = 0.4 * self.unit + slot as f32 * (size + 0.15 * self.unit);
        let top = 0.4 * self.unit;
        [left, top, left + size, top + size]
    }

    fn owner_at(&self, pos: [f32; 2]) -> Option<Owner> {
        let near = |center: [f32; 2], radius: f32| {
            (pos[0] - center[0]).hypot(pos[1] - center[1]) <= radius
        };
        if near(self.use_button, self.use_radius * 1.1) {
            return Some(Owner::Use);
        }
        if near(self.hook, self.hook_radius * 1.3) {
            return Some(Owner::Hook);
        }
        if near(self.jump, self.jump_radius * 1.2) {
            return Some(Owner::Jump);
        }
        if near(self.zoom_in, 0.8 * self.unit * 1.2) {
            return Some(Owner::ZoomIn);
        }
        if near(self.zoom_out, 0.8 * self.unit * 1.2) {
            return Some(Owner::ZoomOut);
        }
        if near(self.inventory, 0.9 * self.unit * 1.2) {
            return Some(Owner::Inventory);
        }
        if near(self.smart, 0.9 * self.unit * 1.2) {
            return Some(Owner::Smart);
        }
        if near(self.stick, self.stick_radius * 1.15) {
            return Some(Owner::Stick);
        }
        for slot in 0..=HOTBAR_VISIBLE {
            let [l, t, r, b] = self.slot_rect(slot);
            if pos[0] >= l && pos[0] <= r && pos[1] >= t && pos[1] <= b {
                return Some(if slot == HOTBAR_VISIBLE {
                    Owner::More
                } else {
                    Owner::Slot(slot as u8)
                });
            }
        }
        None
    }
}

/// 中心からのずれ (画素) を、半径の千分率 (-1000..=1000、長さは 1000 まで) にする。
fn tilt_permille(now: [f32; 2], center: [f32; 2], radius: f32) -> (i32, i32) {
    let (dx, dy) = (now[0] - center[0], now[1] - center[1]);
    let length = dx.hypot(dy);
    let k = if length > radius { radius / length } else { 1.0 };
    ((dx * k / radius * 1000.0).round() as i32, (dy * k / radius * 1000.0).round() as i32)
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

    /// 描画のために、カーソルの枠を知らせる (なければ `None`)。
    pub(crate) fn set_cursor(&mut self, cursor: Option<CursorView>) {
        self.cursor = cursor;
    }

    pub(crate) fn cursor_view(&self) -> Option<CursorView> {
        self.cursor
    }

    /// 使うボタンを最後に倒した向き。
    pub(crate) fn last_aim(&self) -> Option<(i32, i32)> {
        self.last_aim
    }

    /// スマートカーソルが入っているか。
    pub(crate) fn smart(&self) -> bool {
        self.smart
    }

    /// タッチのイベントを反映する。この指が部品のものなら true。
    pub(crate) fn update(&mut self, id: u64, phase: TouchPhase, pos: [f32; 2]) -> bool {
        match phase {
            TouchPhase::Started => {
                self.visible = true;
                if !pos[0].is_finite() || !pos[1].is_finite() {
                    return false;
                }
                // どの部品にも当たらない指は、世界をさわる (仕様 28 §5)。
                let owner = self.layout().owner_at(pos).unwrap_or(Owner::World);
                let mut no_use = false;
                if owner == Owner::World {
                    // 世界の指は同時に 2 本まで。それを超えるなら、古い指は取りこぼしたものとして捨てる。
                    if self.world_count() >= MAX_WORLD_FINGERS {
                        self.touches.retain(|_, t| t.owner != Owner::World);
                    }
                    // 2 本目が触れた: ピンチ。使うのをやめる。
                    if self.world_count() >= 1 {
                        for touch in self.touches.values_mut().filter(|t| t.owner == Owner::World) {
                            touch.no_use = true;
                        }
                        no_use = true;
                    }
                } else {
                    // 同じ部品に新しく触れたら、前の指は古いもの (離したイベントを取りこぼした) とみなして捨てる。
                    self.touches.retain(|_, t| t.owner != owner);
                }
                match owner {
                    Owner::Smart => self.smart = !self.smart,
                    Owner::Inventory | Owner::More => self.inventory_tap = true,
                    Owner::Slot(n) => self.slot_tap = Some(n),
                    _ => {}
                }
                self.touches.insert(id, Touch { owner, now: pos, no_use });
                if owner == Owner::World {
                    self.pinch_distance = self.world_distance();
                }
                true
            }
            TouchPhase::Moved => match self.touches.get_mut(&id) {
                Some(touch) => {
                    if pos[0].is_finite() && pos[1].is_finite() {
                        touch.now = pos;
                    }
                    if touch.owner == Owner::World {
                        self.update_pinch();
                    }
                    true
                }
                None => false,
            },
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(touch) = self.touches.remove(&id) else { return false };
                if touch.owner == Owner::World {
                    self.pinch_distance = self.world_distance();
                }
                if touch.owner == Owner::Hook && phase == TouchPhase::Ended {
                    let layout = self.layout();
                    let (dx, dy) = (touch.now[0] - layout.hook[0], touch.now[1] - layout.hook[1]);
                    self.hook_release = Some(HookRelease {
                        tilt: [dx / layout.hook_radius, dy / layout.hook_radius],
                    });
                }
                true
            }
        }
    }

    /// 全部の指を離した状態に戻す (ウィンドウがフォーカスを失ったとき)。
    pub(crate) fn release_all(&mut self) {
        self.touches.clear();
        self.slot_tap = None;
        self.inventory_tap = false;
        self.hook_release = None;
        self.pinch_distance = None;
        self.pinch_ratio = 1.0;
    }

    /// 世界をさわっている指の数。
    fn world_count(&self) -> usize {
        self.touches.values().filter(|t| t.owner == Owner::World).count()
    }

    /// 世界の指がちょうど 2 本のとき、その距離。
    fn world_distance(&self) -> Option<f32> {
        let mut fingers = self.touches.values().filter(|t| t.owner == Owner::World);
        let (a, b) = (fingers.next()?, fingers.next()?);
        if fingers.next().is_some() {
            return None;
        }
        Some((a.now[0] - b.now[0]).hypot(a.now[1] - b.now[1])).filter(|d| d.is_finite())
    }

    /// 2 本指の距離の変化を、拡大率に積む (1 回の動きで 0.5〜2 倍まで)。
    fn update_pinch(&mut self) {
        let Some(distance) = self.world_distance() else {
            self.pinch_distance = None;
            return;
        };
        if let Some(previous) = self.pinch_distance
            && previous > 0.0
            && distance > 0.0
        {
            let step = (distance / previous).clamp(PINCH_STEP.0, PINCH_STEP.1);
            let ratio = if self.pinch_ratio > 0.0 { self.pinch_ratio } else { 1.0 };
            self.pinch_ratio = ratio * step;
        }
        self.pinch_distance = Some(distance);
    }

    /// ピンチで、まだ反映していない拡大率 (1 = 変化なし)。呼ぶと 1 に戻る。
    pub(crate) fn take_pinch_ratio(&mut self) -> f32 {
        let ratio = if self.pinch_ratio > 0.0 && self.pinch_ratio.is_finite() {
            self.pinch_ratio
        } else {
            1.0
        };
        self.pinch_ratio = 1.0;
        ratio
    }

    /// 世界の指がピンチ中か (テスト用)。
    #[cfg(test)]
    pub(crate) fn pinching(&self) -> bool {
        self.world_count() >= 2
    }

    /// 左スティックに触れている指のずれ (描画用)。`(中心, 指の位置)`。
    pub(crate) fn stick_touch(&self) -> Option<([f32; 2], [f32; 2])> {
        let center = self.layout().stick;
        self.touches.values().find(|t| t.owner == Owner::Stick).map(|t| (center, t.now))
    }

    /// 使うボタンのつまみ (描画用): 倒れた先の画素。
    pub(crate) fn use_knob(&self) -> Option<[f32; 2]> {
        let layout = self.layout();
        self.touches.values().find(|t| t.owner == Owner::Use).map(|t| {
            let (dx, dy) = (t.now[0] - layout.use_button[0], t.now[1] - layout.use_button[1]);
            let length = dx.hypot(dy);
            let k = if length > layout.use_radius { layout.use_radius / length } else { 1.0 };
            [layout.use_button[0] + dx * k, layout.use_button[1] + dy * k]
        })
    }

    /// フックを傾けている間の向き。描画は `output` を消費せずに読む。
    pub(crate) fn hook_aim(&self) -> Option<(i32, i32)> {
        let layout = self.layout();
        self.touches.values().find(|t| t.owner == Owner::Hook).and_then(|touch| {
            let tilt = tilt_permille(touch.now, layout.hook, layout.hook_radius);
            ((tilt.0 as f32).hypot(tilt.1 as f32) >= HOOK_TILT * 1000.0).then_some(tilt)
        })
    }

    pub(crate) fn held(&self, button: &str) -> bool {
        let owner = match button {
            "use" => Owner::Use,
            "jump" => Owner::Jump,
            "hook" => Owner::Hook,
            "zoom_in" => Owner::ZoomIn,
            "zoom_out" => Owner::ZoomOut,
            "inventory" => Owner::Inventory,
            "world" => Owner::World,
            "smart" => return self.smart,
            _ => return false,
        };
        self.touches.values().any(|t| t.owner == owner)
    }

    /// ズームのボタンの向き: 拡大のボタンだけなら 1、縮小だけなら -1、どちらでもない・両方なら 0。
    pub(crate) fn zoom_direction(&self) -> i8 {
        i8::from(self.held("zoom_in")) - i8::from(self.held("zoom_out"))
    }

    /// 今の操作。1 度だけの出来事は、ここで消える。
    pub(crate) fn output(&mut self) -> PadOutput {
        let layout = self.layout();
        let mut out = PadOutput::default();
        for touch in self.touches.values() {
            match touch.owner {
                Owner::Stick => {
                    let dx = touch.now[0] - layout.stick[0];
                    if dx.abs() > layout.stick_radius * WALK_ZONE {
                        out.horizontal = if dx > 0.0 { 1 } else { -1 };
                    }
                    // 上へ倒してもジャンプしない (仕様 18 §1)。
                }
                Owner::Jump => out.jump = true,
                Owner::Use => {
                    out.use_item = true;
                    let tilt = tilt_permille(touch.now, layout.use_button, layout.use_radius);
                    out.cursor_tilt = Some(tilt);
                    // 狙いの向き: 倒れが小さいときは、最後の向き。
                    if tilt.0.abs().max(tilt.1.abs()) > 150 {
                        self.last_aim = Some(tilt);
                    }
                }
                Owner::Hook => {}
                Owner::ZoomIn
                | Owner::ZoomOut
                | Owner::Inventory
                | Owner::Smart
                | Owner::Slot(_)
                | Owner::More
                | Owner::World => {}
            }
        }
        // 世界を直接さわる指 (仕様 28 §5): 使うボタンの指がなく、ピンチでもない 1 本の指だけが、使う。
        let use_finger = self.touches.values().any(|t| t.owner == Owner::Use);
        let mut world = self.touches.values().filter(|t| t.owner == Owner::World);
        if let (Some(touch), None, false) = (world.next(), world.next(), use_finger)
            && !touch.no_use
        {
            out.world_touch = Some(touch.now);
            out.use_item = true;
        }
        out.hook_aim = self.hook_aim();
        if out.use_item {
            out.aim = Some(self.last_aim.unwrap_or(DEFAULT_AIM));
        }
        if let Some(release) = self.hook_release.take() {
            let length = release.tilt[0].hypot(release.tilt[1]);
            if length >= HOOK_TILT {
                let k = 1000.0 / length;
                let dir = ((release.tilt[0] * k) as i32, (release.tilt[1] * k) as i32);
                out.hook_throw = Some(dir);
                out.aim = Some(dir);
            } else {
                out.hook_detach = true;
            }
        }
        out.select_slot = self.slot_tap.take();
        out.toggle_inventory = std::mem::take(&mut self.inventory_tap);
        out.zoom = self.zoom_direction();
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

    /// 固定のスティック: 横へ倒すと歩き、離すと止まる。中心は動かない。
    #[test]
    fn left_stick_is_fixed_and_walks() {
        let mut pad = pad();
        let l = pad.layout();
        let r = l.radius();
        assert!(pad.update(1, TouchPhase::Started, l.stick));
        assert_eq!(pad.output(), PadOutput::default(), "触れただけでは動かない");
        assert_eq!(pad.stick_touch(), Some((l.stick, l.stick)), "中心は固定");
        pad.update(1, TouchPhase::Moved, [l.stick[0] + r, l.stick[1]]);
        assert_eq!(pad.output().horizontal, 1);
        pad.update(1, TouchPhase::Moved, [l.stick[0] - r, l.stick[1]]);
        assert_eq!(pad.output().horizontal, -1);
        assert!(pad.update(1, TouchPhase::Ended, l.stick));
        assert_eq!(pad.output(), PadOutput::default(), "離したら止まる");
    }

    #[test]
    fn left_stick_up_does_not_jump() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, l.stick);
        pad.update(1, TouchPhase::Moved, [l.stick[0], l.stick[1] - l.radius()]);
        let out = pad.output();
        assert!(!out.jump && out.horizontal == 0, "{out:?}");
        // ジャンプは右の `^^` ボタンだけ。
        assert!(pad.update(2, TouchPhase::Started, l.jump));
        assert!(pad.output().jump);
        pad.update(2, TouchPhase::Ended, l.jump);
        assert!(!pad.output().jump);
    }

    /// 使うボタンは、倒したずれ (千分率) でカーソルを動かし、触れている間、使う。
    #[test]
    fn use_button_tilts_the_cursor_and_holds() {
        let mut pad = pad();
        let l = pad.layout();
        assert!(pad.update(1, TouchPhase::Started, l.use_button));
        let out = pad.output();
        assert!(out.use_item && out.cursor_tilt == Some((0, 0)));
        assert_eq!(out.aim, Some(DEFAULT_AIM), "まだ倒していないときは既定の向き");
        pad.update(1, TouchPhase::Moved, [l.use_button[0] - l.use_radius, l.use_button[1]]);
        let out = pad.output();
        assert_eq!(out.cursor_tilt, Some((-1000, 0)));
        assert_eq!(out.aim, Some((-1000, 0)));
        // 半径を超えて倒しても、千分率は 1000 まで。
        pad.update(1, TouchPhase::Moved, [l.use_button[0], l.use_button[1] + l.use_radius * 5.0]);
        assert_eq!(pad.output().cursor_tilt, Some((0, 1000)));
        // つまみの描画位置は、円の縁に収まる。
        let knob = pad.use_knob().unwrap();
        assert!((knob[1] - (l.use_button[1] + l.use_radius)).abs() < 1.0);
        pad.update(1, TouchPhase::Ended, l.use_button);
        let out = pad.output();
        assert!(!out.use_item && out.cursor_tilt.is_none() && out.aim.is_none());
    }

    /// フックのスティック: 傾けて離すと、離した向きへ 1 フレームだけ出る。傾けない離しは切り離し。使うボタンと同時に使える。
    #[test]
    fn hook_stick_throws_toward_the_release_direction() {
        let mut pad = pad();
        let l = pad.layout();
        assert!(pad.update(1, TouchPhase::Started, l.hook));
        assert!(pad.output().hook_throw.is_none() && pad.hook_aim().is_none());
        pad.update(1, TouchPhase::Moved, [l.hook[0], l.hook[1] - l.hook_radius]);
        assert_eq!(pad.hook_aim(), Some((0, -1000)), "描画は入力を消費しない");
        assert_eq!(pad.output().hook_aim, pad.hook_aim(), "描画と入力の向きが一致する");
        // 向きを変えて離す: 離した瞬間の傾きが向きになる。
        pad.update(1, TouchPhase::Moved, [l.hook[0] + l.hook_radius, l.hook[1]]);
        pad.update(1, TouchPhase::Ended, [l.hook[0] + l.hook_radius, l.hook[1]]);
        assert_eq!(pad.hook_aim(), None, "離した後は矢印を消す");
        let out = pad.output();
        assert_eq!(out.hook_throw, Some((1000, 0)));
        assert!(!out.hook_detach && out.aim == Some((1000, 0)));
        assert_eq!(pad.output().hook_throw, None, "1 フレームだけ");
    }

    #[test]
    fn hook_stick_tap_detaches_and_is_independent_of_the_use_button() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, l.use_button);
        pad.update(2, TouchPhase::Started, l.hook);
        let out = pad.output();
        assert!(out.use_item, "同時に使える");
        pad.update(2, TouchPhase::Moved, [l.hook[0] + 0.1 * l.hook_radius, l.hook[1]]);
        pad.update(2, TouchPhase::Ended, l.hook);
        let out = pad.output();
        assert!(out.hook_detach && out.hook_throw.is_none() && out.use_item);
        // キャンセル (指が取り上げられた) では、飛ばさない。
        pad.update(3, TouchPhase::Started, l.hook);
        pad.update(3, TouchPhase::Moved, [l.hook[0], l.hook[1] - l.hook_radius]);
        pad.update(3, TouchPhase::Cancelled, l.hook);
        let out = pad.output();
        assert!(out.hook_throw.is_none() && !out.hook_detach);
    }

    /// ホットバーの枠のタップ・格子 (インベントリ)・`+` (スマートカーソル) の切り替えは、触れたフレームに 1 度だけ出る。
    #[test]
    fn hotbar_selection_and_inventory_toggle() {
        let mut pad = pad();
        let l = pad.layout();
        let center = |s: usize| {
            let r = l.slot_rect(s);
            [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
        };
        assert!(pad.update(1, TouchPhase::Started, center(3)));
        pad.update(1, TouchPhase::Ended, center(3));
        assert_eq!(pad.output().select_slot, Some(3));
        assert_eq!(pad.output().select_slot, None, "1 度だけ");
        // `...` と格子は、インベントリの開閉。
        pad.update(2, TouchPhase::Started, center(HOTBAR_VISIBLE));
        pad.update(2, TouchPhase::Ended, center(HOTBAR_VISIBLE));
        assert!(pad.output().toggle_inventory);
        pad.update(3, TouchPhase::Started, l.inventory);
        pad.update(3, TouchPhase::Ended, l.inventory);
        assert!(pad.output().toggle_inventory && !pad.output().toggle_inventory);
        // `+` はスマートカーソルの入り切り。
        assert!(!pad.smart());
        pad.update(4, TouchPhase::Started, l.smart);
        pad.update(4, TouchPhase::Ended, l.smart);
        assert!(pad.smart());
        pad.update(5, TouchPhase::Started, l.smart);
        assert!(!pad.smart());
    }

    /// ズームのボタン: 押している間、拡大 (+1)・縮小 (-1)。離すと止まる。
    #[test]
    fn zoom_buttons_hold_and_release() {
        let mut pad = pad();
        let layout = pad.layout();
        assert_eq!(pad.output().zoom, 0);
        assert!(pad.update(1, TouchPhase::Started, layout.zoom_in));
        assert_eq!(pad.output().zoom, 1);
        assert!(pad.held("zoom_in") && !pad.held("zoom_out"));
        assert!(pad.update(2, TouchPhase::Started, layout.zoom_out));
        assert_eq!(pad.output().zoom, 0, "両方押したら、打ち消し合う");
        pad.update(1, TouchPhase::Ended, layout.zoom_in);
        assert_eq!((pad.output().zoom, pad.zoom_direction()), (-1, -1), "縮小だけ");
        pad.update(2, TouchPhase::Ended, layout.zoom_out);
        assert_eq!(pad.output().zoom, 0);
    }

    /// 3 本の指を使っても、離したイベントを取りこぼしても、押されたままにならない。
    #[test]
    fn multi_touch_does_not_stick() {
        let mut pad = pad();
        let l = pad.layout();
        assert!(pad.update(1, TouchPhase::Started, l.jump));
        assert!(pad.output().jump);
        assert!(pad.update(7, TouchPhase::Started, l.jump));
        assert!(pad.update(7, TouchPhase::Ended, l.jump));
        assert!(!pad.output().jump, "押されたままにならない");
        let r = l.radius();
        pad.update(2, TouchPhase::Started, l.stick);
        pad.update(2, TouchPhase::Moved, [l.stick[0] + r, l.stick[1]]);
        assert_eq!(pad.output().horizontal, 1);
        pad.update(8, TouchPhase::Started, l.stick);
        assert_eq!(pad.output().horizontal, 0, "新しい指が置き換わり、古い指は捨てられる");
        pad.release_all();
        for (id, at) in [(10, l.use_button), (11, l.hook), (12, l.stick)] {
            assert!(pad.update(id, TouchPhase::Started, at));
        }
        for (id, at) in [(11, l.hook), (12, l.stick), (10, l.use_button)] {
            assert!(pad.update(id, TouchPhase::Ended, at));
        }
        let out = pad.output();
        assert!(!out.use_item && out.horizontal == 0 && out.cursor_tilt.is_none());
    }

    const WORLD: [f32; 2] = [800.0, 300.0];

    /// 部品に当たらない新しい指だけが、世界の指になる (仕様 28 §5)。
    #[test]
    fn world_touch_is_taken_only_outside_controls() {
        let mut pad = pad();
        let l = pad.layout();
        // 部品の上の指は、世界の指にならない。
        assert!(pad.update(1, TouchPhase::Started, l.jump));
        assert!(!pad.held("world") && pad.output().world_touch.is_none());
        pad.update(1, TouchPhase::Ended, l.jump);
        // 何もない場所の指は、世界の指。使う指になり、動かすと追う。
        assert!(pad.update(2, TouchPhase::Started, WORLD));
        assert!(pad.held("world"));
        let out = pad.output();
        assert!(out.use_item && out.world_touch == Some(WORLD) && out.cursor_tilt.is_none());
        pad.update(2, TouchPhase::Moved, [900.0, 350.0]);
        assert_eq!(pad.output().world_touch, Some([900.0, 350.0]));
        // 不正な座標の動きは無視する。不正な座標で触れても世界の指にならない。
        pad.update(2, TouchPhase::Moved, [f32::NAN, 1.0]);
        assert_eq!(pad.output().world_touch, Some([900.0, 350.0]));
        assert!(!pad.update(3, TouchPhase::Started, [f32::NAN, 0.0]));
        pad.update(2, TouchPhase::Ended, WORLD);
        assert!(!pad.held("world") && pad.output().world_touch.is_none());
        assert!(!pad.output().use_item);
    }

    /// 使うボタンの指があるとき、世界の指は動かさない。離すと、また動く。
    #[test]
    fn use_button_wins_over_world_touch() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, WORLD);
        pad.update(2, TouchPhase::Started, l.use_button);
        let out = pad.output();
        assert!(out.world_touch.is_none(), "{out:?}");
        assert!(out.use_item && out.cursor_tilt == Some((0, 0)), "使うボタンが優先");
        pad.update(2, TouchPhase::Ended, l.use_button);
        assert_eq!(pad.output().world_touch, Some(WORLD), "使うボタンを離したら、また動く");
    }

    /// 世界の指が 2 本: ピンチでズームし、使うのをやめる。1 本に戻っても、離して触れ直すまで使わない。
    #[test]
    fn two_world_fingers_pinch_zoom_and_stop_using() {
        let mut pad = pad();
        pad.update(1, TouchPhase::Started, [700.0, 300.0]);
        assert!(pad.output().use_item);
        pad.update(2, TouchPhase::Started, [800.0, 300.0]);
        assert!(pad.pinching());
        let out = pad.output();
        assert!(!out.use_item && out.world_touch.is_none(), "ピンチ中は使わない");
        assert_eq!(pad.take_pinch_ratio(), 1.0, "動くまでは変化なし");
        // 100 → 150: 1.5 倍。続けて 150 → 120: 0.8 倍。積は 1.2 倍。
        pad.update(2, TouchPhase::Moved, [850.0, 300.0]);
        pad.update(2, TouchPhase::Moved, [820.0, 300.0]);
        assert!((pad.take_pinch_ratio() - 1.2).abs() < 1e-4);
        assert_eq!(pad.take_pinch_ratio(), 1.0, "取ると 1 に戻る");
        // 一度に大きく動いても、1 回の変化は 2 倍まで。
        pad.update(2, TouchPhase::Moved, [3000.0, 300.0]);
        assert!((pad.take_pinch_ratio() - 2.0).abs() < 1e-4);
        // 1 本に戻っても使わない。触れ直せば使う。
        pad.update(2, TouchPhase::Ended, [3000.0, 300.0]);
        assert!(!pad.pinching());
        assert!(!pad.output().use_item, "ピンチの名残の指は使わない");
        pad.update(1, TouchPhase::Ended, [700.0, 300.0]);
        pad.update(3, TouchPhase::Started, WORLD);
        assert!(pad.output().use_item);
    }

    /// 世界の指も、離す・取り消し・全解除で、押されたままにならない。取りこぼしても、3 本目で古い指を捨てる。
    #[test]
    fn world_touch_never_sticks() {
        let mut pad = pad();
        pad.update(1, TouchPhase::Started, WORLD);
        pad.update(1, TouchPhase::Cancelled, WORLD);
        assert!(!pad.held("world") && !pad.output().use_item);
        pad.update(2, TouchPhase::Started, WORLD);
        pad.release_all();
        assert!(!pad.held("world") && !pad.output().use_item);
        assert_eq!(pad.take_pinch_ratio(), 1.0);
        // 離したイベントを取りこぼした古い 2 本があっても、新しい指でやり直せる。
        pad.update(3, TouchPhase::Started, [100.0, 300.0]);
        pad.update(4, TouchPhase::Started, [200.0, 300.0]);
        pad.update(5, TouchPhase::Started, WORLD);
        assert!(!pad.pinching(), "古い指は捨てられた");
        assert_eq!(pad.output().world_touch, Some(WORLD));
        // 知らない指の動き・終わりは、部品のものではない。
        assert!(!pad.update(99, TouchPhase::Moved, WORLD));
        assert!(!pad.update(99, TouchPhase::Ended, WORLD));
    }

    /// UI の大きさの倍率で、ボタンの半径とスティックの半径が変わる。
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

    /// 仕様 18 §1 の配置: 各部品が表のとおりの位置・大きさで、画面の中に収まり、主な部品は重ならない。
    #[test]
    fn terraria_layout_matches_the_reference_positions() {
        for view in [[1600.0, 900.0], [800.0, 400.0], [2400.0, 1080.0]] {
            let l = Layout::new(view, 1.0);
            let u = l.unit;
            let (w, h) = (view[0], view[1]);
            assert_eq!(l.stick, [3.6 * u, h - 3.6 * u]);
            assert_eq!(l.stick_radius, 3.0 * u);
            assert_eq!(l.use_radius, 2.2 * u);
            assert!(
                (l.hook[0] - (l.use_button[0] - 3.6 * u)).abs() < 1e-3,
                "フックは使うボタンの左隣"
            );
            assert!((l.hook[1] - (l.use_button[1] + 0.3 * u)).abs() < 1e-3);
            assert_eq!(l.hook_radius, 1.1 * u);
            assert!(
                l.jump[0] > l.use_button[0] && l.jump[1] < l.use_button[1],
                "ジャンプは使うボタンの右上"
            );
            assert!(l.zoom_out[0] < w * 0.5 && l.zoom_in[0] > w * 0.5, "ズームは下の中央");
            let parts = [
                (l.stick, l.stick_radius),
                (l.use_button, l.use_radius),
                (l.hook, l.hook_radius),
                (l.jump, l.jump_radius),
                (l.zoom_in, 0.8 * u),
                (l.zoom_out, 0.8 * u),
                (l.inventory, 0.9 * u),
                (l.smart, 0.9 * u),
            ];
            for (i, (a, ra)) in parts.iter().enumerate() {
                assert!(a[0] - ra >= -0.1 && a[0] + ra <= w + 0.1, "{view:?}: 横に収まる {a:?}");
                assert!(a[1] - ra >= -0.1 && a[1] + ra <= h + 0.1, "{view:?}: 縦に収まる {a:?}");
                for (b, rb) in &parts[i + 1..] {
                    assert!(
                        (a[0] - b[0]).hypot(a[1] - b[1]) >= ra + rb - 0.1,
                        "{view:?}: 重ならない"
                    );
                }
            }
            // ホットバーは左上、横一列。
            let first = l.slot_rect(0);
            let last = l.slot_rect(HOTBAR_VISIBLE);
            assert!(first[0] < u && first[1] < u);
            assert_eq!(first[1], last[1], "横一列");
        }
    }
}
