//! 仕様 18: 画面のタッチ操作 (Terraria Android 型)。ゲームパッドと同じ `PadOutput` を作る。
//!
//! - 左下の隅に**固定の左スティック** (横で歩く。上へ倒してもジャンプしない)。
//! - 右下の**使うボタン**: 大きな円で、**スティック**として動く。倒したずれでカーソルが動き、触れている間、選んでいるアイテムを使う。
//! - 使うボタンの左隣の**フックのスティック**: 傾けて離した向きへ、フックを飛ばす。傾けない (タップ) と、切り離す。
//! - **ジャンプ** (`^^`)・**ズーム** (拡大・縮小)・**インベントリ**の格子・**スマートカーソルの切り替え** (`+`)・**ホットバー** (横一列)。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

use crate::settings::{AutoSelectMode, HotbarOrientation, UpToJump};

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
    /// オートセレクトが有効 (押している間、または入っている間)。
    pub auto_select: bool,
    /// ロックオン・インタラクトのボタンが押された (このフレームだけ)。
    pub lock_on: bool,
    pub interact: bool,
    /// クイック回復・クイックマナのボタンが押された (このフレームだけ)。
    pub quick_heal: bool,
    pub quick_mana: bool,
    /// ホットバーの枠がタップされた (このフレームだけ)。
    pub select_slot: Option<u8>,
    /// ホットバーの枠から外へ引いて離した: その枠のアイテムを投げる (このフレームだけ、仕様 28 §9)。
    pub throw_slot: Option<u8>,
    /// インベントリの格子が押された (このフレームだけ)。
    pub toggle_inventory: bool,
    /// 世界を直接さわっている 1 本の指の位置 (画面の画素、仕様 28 §5)。使うボタンの指があるとき・ピンチ中は `None`。
    /// 世界の指は、狙う方式に関係なく、触れている間、使う。
    pub world_touch: Option<[f32; 2]>,
    /// サブ攻撃ボタンを押している (仕様 28 §6)。
    pub action: bool,
    /// 狙う方式の切り替えボタンが押された (このフレームだけ)。
    pub toggle_aim: bool,
    /// スマートカーソルの `+` が押された (このフレームだけ)。
    pub toggle_smart: bool,
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
            auto_select: self.auto_select || other.auto_select,
            lock_on: self.lock_on || other.lock_on,
            interact: self.interact || other.interact,
            quick_heal: self.quick_heal || other.quick_heal,
            quick_mana: self.quick_mana || other.quick_mana,
            select_slot: other.select_slot.or(self.select_slot),
            throw_slot: other.throw_slot.or(self.throw_slot),
            toggle_inventory: self.toggle_inventory || other.toggle_inventory,
            world_touch: other.world_touch.or(self.world_touch),
            action: self.action || other.action,
            toggle_aim: self.toggle_aim || other.toggle_aim,
            toggle_smart: self.toggle_smart || other.toggle_smart,
        }
    }
}

/// 歩き始める倒れ (スティックの半径に対する割合)。
const WALK_ZONE: f32 = 0.3;
/// フックのスティックを「傾けた」とみなす倒れ (半径に対する割合)。
const HOOK_TILT: f32 = 0.3;
/// 狙いの既定の向き (右上)。使うボタンをまだ倒していないとき。
const DEFAULT_AIM: (i32, i32) = (1000, -400);
/// ホットバー (横) に見える枠の数 (残りは `...` からインベントリで)。
pub(crate) const HOTBAR_VISIBLE: usize = 5;
/// ホットバー (縦) の枠の数と、1 列の行数。
pub(crate) const HOTBAR_VERTICAL_SLOTS: usize = 10;
const HOTBAR_VERTICAL_ROWS: usize = 5;
/// 枠をこの枠の数より外へ引いて離すと、投げる (仕様 28 §9)。
const THROW_SLOTS: f32 = 1.5;

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
    /// サブ攻撃ボタン (仕様 28 §6)。
    Action,
    /// 狙う方式の切り替えボタン。
    AimToggle,
    QuickHeal,
    QuickMana,
    AutoSelect,
    Interact,
    LockOn,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    owner: Owner,
    now: [f32; 2],
    /// 世界の指が 2 本以上になった: ピンチなので、アイテムを使わない。1 本に戻っても、離すまで使わない。
    no_use: bool,
}

/// 「上へ倒した」とみなす倒れ (半径に対する割合)。歩く (`WALK_ZONE`) より大きい。
const UP_TILT: f32 = 0.5;
/// ダブルタップとみなす間隔 (秒)。
const DOUBLE_TAP_SECS: f32 = 0.3;
/// スワイプとみなす、上への動きの速さ: この時間 (秒) で `UP_TILT` を超える。
const SWIPE_SECS: f32 = 0.15;

/// 左スティックの上でジャンプする方式の状態 (仕様 28 §7)。
#[derive(Clone, Copy, Debug, Default)]
struct JumpState {
    /// 前のフレームで、上へ倒していたか。
    up: bool,
    /// 直前に上へ倒し始めた時刻 (秒)。
    last_up_at: Option<f32>,
    /// ダブルタップ・スワイプが成立した (指を離すまで)。
    armed: bool,
    /// スワイプの判定のための、倒していない状態になった時刻と倒れ。
    rest: Option<(f32, f32)>,
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
    /// 描画用: 今の種類の狙う方式が Aim Only か、スマートカーソルが入っているか (`build_frame` が知らせる)。
    aim_only: bool,
    smart: bool,
    /// 描画用: クイック回復・クイックマナで使えるものがあるか。
    quick_ready: [bool; 2],
    /// ホットバーの並び (設定)。
    hotbar: HotbarOrientation,
    /// 選んだアイテムの名前を出しておく時間 (設定、秒)。描画が使う。
    tooltip_fade: Option<f32>,
    /// 左スティックの上でジャンプする方式と、その状態。
    up_to_jump: UpToJump,
    jump: JumpState,
    /// 時計 (秒)。`advance` で進める。
    clock: f32,
    quick_heal_tap: bool,
    quick_mana_tap: bool,
    lock_tap: bool,
    interact_tap: bool,
    /// オートセレクトのボタンの動きと、`Toggle` のときの入り切り。
    auto_mode: AutoSelectMode,
    auto_on: bool,
    /// 描画用: ロック中の敵と、ロックできる最優先の敵。
    lock_view: (Option<charon_game::EntityId>, Option<charon_game::EntityId>),
    /// このフレームだけ立つ出来事。
    smart_tap: bool,
    aim_tap: bool,
    slot_tap: Option<u8>,
    throw_tap: Option<u8>,
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
    /// 狙う方式の切り替えボタン (半径 `0.9u`)。
    pub aim_toggle: [f32; 2],
    /// サブ攻撃ボタン (使うボタンの左上、半径 `1.0u`)。
    pub action: [f32; 2],
    pub action_radius: f32,
    /// クイック回復・クイックマナのボタン (半径 `1.1u`・`1.0u`)。
    pub quick_heal: [f32; 2],
    pub quick_mana: [f32; 2],
    /// ホットバーの並び。
    pub hotbar: HotbarOrientation,
    /// オートセレクト・インタラクト・ロックオンのボタン (半径 `0.9u`・`1.0u`・`0.9u`)。
    pub auto_select: [f32; 2],
    pub interact: [f32; 2],
    pub lock_on: [f32; 2],
}

impl Layout {
    /// `scale` は部品全体の大きさの倍率 (UI の大きさの設定)。0 以下・有限でない値は 1 とみなす。
    pub(crate) fn new(viewport: [f32; 2], scale: f32) -> Self {
        Self::with_hotbar(viewport, scale, HotbarOrientation::Horizontal)
    }

    /// ホットバーの並びを指定した配置。縦型は左上に 2 列 × 5 行を使うので、その下の段のボタンを右へずらす。
    pub(crate) fn with_hotbar(viewport: [f32; 2], scale: f32, hotbar: HotbarOrientation) -> Self {
        let (w, h) = (viewport[0], viewport[1]);
        let shift = if hotbar == HotbarOrientation::Vertical { 3.6 } else { 0.0 };
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
            inventory: [(1.2 + shift) * unit, h - 8.0 * unit],
            smart: [(3.4 + shift) * unit, h - 8.0 * unit],
            aim_toggle: [(5.6 + shift) * unit, h - 8.0 * unit],
            action: [use_button[0] - 1.9 * unit, use_button[1] - 3.6 * unit],
            action_radius: unit,
            quick_heal: [8.2 * unit, h - 5.6 * unit],
            quick_mana: [8.2 * unit, h - 3.2 * unit],
            hotbar,
            auto_select: [(7.8 + shift) * unit, h - 8.0 * unit],
            interact: [10.4 * unit, h - 4.4 * unit],
            lock_on: [w * 0.5 + 3.6 * unit, h - 1.2 * unit],
        }
    }

    /// 左スティックのずらせる最大の距離。
    pub(crate) fn radius(&self) -> f32 {
        self.stick_radius
    }

    /// 見えるホットバーの枠の数。横は 5 (そのあとに `...`)、縦は 10。
    pub(crate) fn hotbar_visible(&self) -> usize {
        match self.hotbar {
            HotbarOrientation::Horizontal => HOTBAR_VISIBLE,
            HotbarOrientation::Vertical => HOTBAR_VERTICAL_SLOTS,
        }
    }

    /// `...` (持ち物を開く) の枠があるか。縦型にはない。
    pub(crate) fn has_more(&self) -> bool {
        self.hotbar == HotbarOrientation::Horizontal
    }

    /// ホットバーの枠 `slot` (`0..hotbar_visible()`) の矩形 `[左, 上, 右, 下]`。横型で `slot == HOTBAR_VISIBLE` は `...`。
    /// 縦型は、左の列が 0〜4、右の列が 5〜9 (上から下へ)。
    pub(crate) fn slot_rect(&self, slot: usize) -> [f32; 4] {
        // 縦型は 5 行が左スティックの上に収まるよう、少し小さくする。
        let (size, gap) = match self.hotbar {
            HotbarOrientation::Horizontal => (1.5 * self.unit, 0.15 * self.unit),
            HotbarOrientation::Vertical => (1.2 * self.unit, 0.1 * self.unit),
        };
        let pitch = size + gap;
        let (left, top) = match self.hotbar {
            HotbarOrientation::Horizontal => {
                (0.4 * self.unit + slot as f32 * pitch, 0.4 * self.unit)
            }
            HotbarOrientation::Vertical => (
                0.4 * self.unit + (slot / HOTBAR_VERTICAL_ROWS) as f32 * pitch,
                0.4 * self.unit + (slot % HOTBAR_VERTICAL_ROWS) as f32 * pitch,
            ),
        };
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
        if near(self.action, self.action_radius * 1.2) {
            return Some(Owner::Action);
        }
        if near(self.auto_select, 0.9 * self.unit * 1.2) {
            return Some(Owner::AutoSelect);
        }
        if near(self.interact, self.unit * 1.2) {
            return Some(Owner::Interact);
        }
        if near(self.lock_on, 0.9 * self.unit * 1.2) {
            return Some(Owner::LockOn);
        }
        if near(self.quick_heal, 1.1 * self.unit * 1.2) {
            return Some(Owner::QuickHeal);
        }
        if near(self.quick_mana, 1.0 * self.unit * 1.2) {
            return Some(Owner::QuickMana);
        }
        if near(self.aim_toggle, 0.9 * self.unit * 1.2) {
            return Some(Owner::AimToggle);
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
        let visible = self.hotbar_visible();
        let last = if self.has_more() { visible } else { visible - 1 };
        for slot in 0..=last {
            let [l, t, r, b] = self.slot_rect(slot);
            if pos[0] >= l && pos[0] <= r && pos[1] >= t && pos[1] <= b {
                return Some(if self.has_more() && slot == visible {
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
        Layout::with_hotbar(self.viewport, self.scale, self.hotbar)
    }

    /// ホットバーの並び (設定)。
    pub(crate) fn set_hotbar(&mut self, hotbar: HotbarOrientation) {
        self.hotbar = hotbar;
    }

    /// 選んだアイテムの名前を出しておく時間 (設定、秒)。
    pub(crate) fn set_tooltip_fade(&mut self, secs: f32) {
        self.tooltip_fade = Some(secs);
    }

    /// 選んだアイテムの名前を出しておく時間 (秒)。まだ設定されていなければ既定。
    pub(crate) fn tooltip_fade(&self) -> f32 {
        self.tooltip_fade.unwrap_or(crate::hud_fade::NAME_FADE_DEFAULT)
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

    /// スマートカーソルが入っているか (今の種類、`build_frame` が知らせた値)。
    pub(crate) fn smart(&self) -> bool {
        self.smart
    }

    /// 左スティックの上でジャンプする方式 (設定)。
    pub(crate) fn set_up_to_jump(&mut self, mode: UpToJump) {
        self.up_to_jump = mode;
    }

    /// 時計を進める (秒)。ダブルタップなどの時間の判定に使う。不正な値は無視する。
    pub(crate) fn advance(&mut self, dt: f32) {
        if dt.is_finite() && dt > 0.0 {
            self.clock += dt.min(1.0);
        }
    }

    /// オートセレクトのボタンの動き (設定)。切り替えたときは、入り切りを戻す。
    pub(crate) fn set_auto_mode(&mut self, mode: AutoSelectMode) {
        if self.auto_mode != mode {
            self.auto_mode = mode;
            self.auto_on = false;
        }
    }

    /// オートセレクトが有効か (押している間、または入っている間)。
    pub(crate) fn auto_select(&self) -> bool {
        match self.auto_mode {
            AutoSelectMode::Hold => self.held("auto_select"),
            AutoSelectMode::Toggle => self.auto_on,
        }
    }

    /// 描画のために、ロック中の敵と、ロックできる最優先の敵を知らせる。
    pub(crate) fn set_lock(
        &mut self,
        target: Option<charon_game::EntityId>,
        candidate: Option<charon_game::EntityId>,
    ) {
        self.lock_view = (target, candidate);
    }

    /// ロック中の敵と、ロックできる最優先の敵 (描画用)。
    pub(crate) fn lock_view(
        &self,
    ) -> (Option<charon_game::EntityId>, Option<charon_game::EntityId>) {
        self.lock_view
    }

    /// 描画のために、クイック回復・クイックマナで使えるものがあるかを知らせる。
    pub(crate) fn set_quick(&mut self, heal: bool, mana: bool) {
        self.quick_ready = [heal, mana];
    }

    /// クイック回復・クイックマナのボタンが使えるか (描画用)。
    pub(crate) fn quick_ready(&self) -> [bool; 2] {
        self.quick_ready
    }

    /// 左スティックの倒れ (半径に対する割合) から、上のジャンプを判定する (仕様 28 §7)。
    fn stick_jump(&mut self, tilt: Option<[f32; 2]>) -> bool {
        let now = self.clock;
        let up = tilt.is_some_and(|t| -t[1] > UP_TILT);
        let state = &mut self.jump;
        if tilt.is_none() {
            // 指を離した: 成立は取り消す。次のダブルタップの判定のために、時刻は残す。
            state.armed = false;
            state.rest = None;
            state.up = false;
            return false;
        }
        let rising = up && !state.up;
        state.up = up;
        match self.up_to_jump {
            UpToJump::Disabled => state.armed = false,
            UpToJump::SingleTap => state.armed = true,
            UpToJump::DoubleTap => {
                if rising {
                    if state.last_up_at.is_some_and(|t| now - t <= DOUBLE_TAP_SECS) {
                        state.armed = true;
                    }
                    state.last_up_at = Some(now);
                }
            }
            UpToJump::Swipe => {
                let dy = tilt.map_or(0.0, |t| t[1]);
                // 倒していない状態 (または止まった時点) を覚え、そこから短い時間で `UP_TILT` を超えたらスワイプ。
                match state.rest {
                    Some((t, from)) if !state.armed => {
                        if from - dy > UP_TILT && now - t <= SWIPE_SECS {
                            state.armed = true;
                        } else if now - t > SWIPE_SECS || dy > from {
                            state.rest = Some((now, dy));
                        }
                    }
                    None => state.rest = Some((now, dy)),
                    _ => {}
                }
            }
        }
        // 成立していて、上へ倒している間だけ押す。横・下へ倒して上でなくなると、飛行は一時止まり、上へ戻すと続く。
        state.armed && up
    }

    /// 今の種類の狙う方式が Aim Only か。
    pub(crate) fn aim_only(&self) -> bool {
        self.aim_only
    }

    /// 描画のために、今の種類の狙う方式とスマートカーソルを知らせる。
    pub(crate) fn set_modes(&mut self, aim_only: bool, smart: bool) {
        self.aim_only = aim_only;
        self.smart = smart;
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
                    Owner::Smart => self.smart_tap = true,
                    Owner::AimToggle => self.aim_tap = true,
                    Owner::QuickHeal => self.quick_heal_tap = true,
                    Owner::LockOn => self.lock_tap = true,
                    Owner::Interact => self.interact_tap = true,
                    Owner::AutoSelect => {
                        if self.auto_mode == AutoSelectMode::Toggle {
                            self.auto_on = !self.auto_on;
                        }
                    }
                    Owner::QuickMana => self.quick_mana_tap = true,
                    Owner::Inventory | Owner::More => self.inventory_tap = true,
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
                // ホットバーの枠は、指を離したときに選ぶ。外へ引いて離したら、投げる (仕様 28 §9)。
                if let (Owner::Slot(n), TouchPhase::Ended) = (touch.owner, phase) {
                    let [l, t, r, b] = self.layout().slot_rect(usize::from(n));
                    let reach = THROW_SLOTS * (r - l);
                    let near = (l - reach..=r + reach).contains(&touch.now[0])
                        && (t - reach..=b + reach).contains(&touch.now[1]);
                    if near {
                        self.slot_tap = Some(n);
                    } else {
                        self.throw_tap = Some(n);
                    }
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
        self.throw_tap = None;
        self.inventory_tap = false;
        self.smart_tap = false;
        self.aim_tap = false;
        self.quick_heal_tap = false;
        self.quick_mana_tap = false;
        self.lock_tap = false;
        self.interact_tap = false;
        self.jump.armed = false;
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
            "action" => Owner::Action,
            "auto_select" => Owner::AutoSelect,
            "interact" => Owner::Interact,
            "lock_on" => return self.lock_view.0.is_some(),
            "quick_heal" => Owner::QuickHeal,
            "quick_mana" => Owner::QuickMana,
            "aim_toggle" => return self.aim_only,
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
        let mut stick_tilt = None;
        for touch in self.touches.values() {
            match touch.owner {
                Owner::Stick => {
                    let dx = touch.now[0] - layout.stick[0];
                    if dx.abs() > layout.stick_radius * WALK_ZONE {
                        out.horizontal = if dx > 0.0 { 1 } else { -1 };
                    }
                    // 上のジャンプは、設定の方式で下で判定する (仕様 28 §7)。
                    stick_tilt = Some([
                        dx / layout.stick_radius,
                        (touch.now[1] - layout.stick[1]) / layout.stick_radius,
                    ]);
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
                Owner::Action => out.action = true,
                Owner::Hook => {}
                Owner::ZoomIn
                | Owner::ZoomOut
                | Owner::Inventory
                | Owner::Smart
                | Owner::AimToggle
                | Owner::QuickHeal
                | Owner::QuickMana
                | Owner::AutoSelect
                | Owner::Interact
                | Owner::LockOn
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
        }
        if self.stick_jump(stick_tilt) {
            out.jump = true;
        }
        out.auto_select = self.auto_select();
        out.lock_on = std::mem::take(&mut self.lock_tap);
        out.interact = std::mem::take(&mut self.interact_tap);
        out.quick_heal = std::mem::take(&mut self.quick_heal_tap);
        out.quick_mana = std::mem::take(&mut self.quick_mana_tap);
        out.toggle_smart = std::mem::take(&mut self.smart_tap);
        out.toggle_aim = std::mem::take(&mut self.aim_tap);
        out.hook_aim = self.hook_aim();
        if out.use_item || out.action {
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
        out.throw_slot = self.throw_tap.take();
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
        // `+` はスマートカーソルの切り替え。触れたフレームに 1 度だけ出る (状態は種類ごとの記憶で、`build_frame` が持つ)。
        assert!(!pad.output().toggle_smart);
        pad.update(4, TouchPhase::Started, l.smart);
        pad.update(4, TouchPhase::Ended, l.smart);
        assert!(pad.output().toggle_smart);
        assert!(!pad.output().toggle_smart, "1 度だけ");
        // 狙う方式の切り替えも同じ。
        pad.update(5, TouchPhase::Started, l.aim_toggle);
        pad.update(5, TouchPhase::Ended, l.aim_toggle);
        let out = pad.output();
        assert!(out.toggle_aim && !out.toggle_smart);
        assert!(!pad.output().toggle_aim);
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

    /// 左スティックの指 `id` を、半径に対する割合 `(dx, dy)` へ動かす。
    fn tilt(pad: &mut TouchPad, id: u64, dx: f32, dy: f32) {
        let l = pad.layout();
        pad.update(
            id,
            TouchPhase::Moved,
            [l.stick[0] + dx * l.stick_radius, l.stick[1] + dy * l.stick_radius],
        );
    }

    /// 上へ 2 回すばやく倒すとジャンプし、倒し続けると押し続ける。1 回だけでは跳ばない (仕様 28 §7)。
    #[test]
    fn double_tap_up_jumps_and_hold_keeps_jumping() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, l.stick);
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(!pad.output().jump, "1 回目は跳ばない");
        pad.advance(0.1);
        tilt(&mut pad, 1, 0.0, 0.0);
        assert!(!pad.output().jump);
        pad.advance(0.1);
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(pad.output().jump, "0.2 秒で 2 回目: ジャンプ");
        pad.advance(1.0);
        assert!(pad.output().jump, "倒し続けると、押し続ける (浮遊・高く跳ぶ)");
        // 倒し方が上でなくなると止まり、上へ戻すと続く。斜めの上でも、押し続ける。
        tilt(&mut pad, 1, 0.0, 0.0);
        assert!(!pad.output().jump);
        tilt(&mut pad, 1, 0.6, -0.7);
        let out = pad.output();
        assert!(out.jump && out.horizontal == 1, "斜め上: 飛びながら進む");
        // 指を離すと止まり、触れ直して 1 回だけ倒しても跳ばない。
        pad.update(1, TouchPhase::Ended, l.stick);
        assert!(!pad.output().jump);
        pad.advance(2.0);
        pad.update(2, TouchPhase::Started, l.stick);
        tilt(&mut pad, 2, 0.0, -0.8);
        assert!(!pad.output().jump);
    }

    /// 0.3 秒を超えると、2 回目はダブルタップにならない。
    #[test]
    fn double_tap_window_expires() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, l.stick);
        tilt(&mut pad, 1, 0.0, -0.8);
        pad.output();
        pad.advance(0.1);
        tilt(&mut pad, 1, 0.0, 0.0);
        pad.output();
        pad.advance(0.31);
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(!pad.output().jump, "最初から 0.41 秒: 2 回目ではなく、新しい 1 回目");
        // その新しい 1 回目から 0.3 秒以内の次は、ダブルタップ。
        pad.advance(0.1);
        tilt(&mut pad, 1, 0.0, 0.0);
        pad.output();
        pad.advance(0.1);
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(pad.output().jump);
        // 指を離して触れ直しても、間隔が短ければダブルタップ (タップ、離す、タップ)。
        let mut pad2 = self::pad();
        pad2.update(1, TouchPhase::Started, l.stick);
        tilt(&mut pad2, 1, 0.0, -0.8);
        pad2.output();
        pad2.update(1, TouchPhase::Ended, l.stick);
        pad2.output();
        pad2.advance(0.2);
        pad2.update(2, TouchPhase::Started, l.stick);
        tilt(&mut pad2, 2, 0.0, -0.8);
        assert!(pad2.output().jump, "離して触れ直した 2 回目");
    }

    /// 1 回で跳ぶ方式、すばやいはじき、スティックのジャンプなし。
    #[test]
    fn single_tap_swipe_and_disabled_up_to_jump() {
        let l = pad().layout();
        // SingleTap: 上へ倒すだけでジャンプ。
        let mut pad = pad();
        pad.set_up_to_jump(UpToJump::SingleTap);
        pad.update(1, TouchPhase::Started, l.stick);
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(pad.output().jump);
        tilt(&mut pad, 1, 0.0, 0.0);
        assert!(!pad.output().jump);
        // Disabled: 何回倒しても、ジャンプしない。ジャンプボタンは使える。
        let mut pad = self::pad();
        pad.set_up_to_jump(UpToJump::Disabled);
        pad.update(1, TouchPhase::Started, l.stick);
        for _ in 0..3 {
            tilt(&mut pad, 1, 0.0, -0.8);
            assert!(!pad.output().jump);
            pad.advance(0.05);
            tilt(&mut pad, 1, 0.0, 0.0);
            pad.output();
            pad.advance(0.05);
        }
        pad.update(2, TouchPhase::Started, l.jump);
        assert!(pad.output().jump, "ジャンプボタンは、どの方式でも使える");
        // Swipe: 短い時間で大きく上へ動くとジャンプ。ゆっくり倒してもジャンプしない。
        let mut pad = self::pad();
        pad.set_up_to_jump(UpToJump::Swipe);
        pad.update(1, TouchPhase::Started, l.stick);
        pad.output();
        pad.advance(0.05);
        tilt(&mut pad, 1, 0.0, -0.9);
        assert!(pad.output().jump, "0.05 秒で 0.9 上へ");
        let mut slow = self::pad();
        slow.set_up_to_jump(UpToJump::Swipe);
        slow.update(1, TouchPhase::Started, l.stick);
        slow.output();
        for step in 1..=4 {
            slow.advance(0.1);
            tilt(&mut slow, 1, 0.0, -0.22 * step as f32);
            assert!(!slow.output().jump, "ゆっくり倒す: step {step}");
        }
    }

    /// 横・下へ倒すと飛行は一時止まり、上へ戻すと続く。
    #[test]
    fn side_or_down_pauses_flight_until_up_again() {
        let mut pad = pad();
        let l = pad.layout();
        pad.update(1, TouchPhase::Started, l.stick);
        for _ in 0..2 {
            tilt(&mut pad, 1, 0.0, -0.8);
            pad.output();
            pad.advance(0.1);
            tilt(&mut pad, 1, 0.0, 0.0);
            pad.output();
            pad.advance(0.1);
        }
        tilt(&mut pad, 1, 0.0, -0.8);
        assert!(pad.output().jump, "飛んでいる");
        tilt(&mut pad, 1, 0.9, 0.0);
        let out = pad.output();
        assert!(!out.jump && out.horizontal == 1, "横へ倒すと、止まって歩く");
        tilt(&mut pad, 1, 0.0, 0.9);
        assert!(!pad.output().jump, "下へ倒すと止まる");
        tilt(&mut pad, 1, -0.3, -0.8);
        assert!(pad.output().jump, "上へ戻すと続く");
    }

    /// 縦型のホットバー (2 列 × 5 行) は 10 枠が全部画面に収まり、枠どうしも、仮想コントロールとも重ならない。
    #[test]
    fn hotbar_vertical_layout_fits_without_overlap() {
        for view in [[1600.0, 900.0], [800.0, 400.0], [2400.0, 1080.0]] {
            let l = Layout::with_hotbar(view, 1.0, HotbarOrientation::Vertical);
            let u = l.unit;
            assert_eq!((l.hotbar_visible(), l.has_more()), (10, false));
            // 左の列が 0〜4、右の列が 5〜9。
            assert_eq!(l.slot_rect(0)[0], l.slot_rect(4)[0]);
            assert!(l.slot_rect(5)[0] > l.slot_rect(4)[2], "右の列");
            assert_eq!(l.slot_rect(5)[1], l.slot_rect(0)[1], "同じ行");
            let slots: Vec<[f32; 4]> = (0..10).map(|s| l.slot_rect(s)).collect();
            for (i, a) in slots.iter().enumerate() {
                assert!(
                    a[0] >= 0.0 && a[1] >= 0.0 && a[2] <= view[0] && a[3] <= view[1],
                    "{view:?}: 収まる {a:?}"
                );
                for b in &slots[i + 1..] {
                    assert!(
                        a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1],
                        "枠どうしは重ならない"
                    );
                }
            }
            let parts = [
                (l.stick, l.stick_radius),
                (l.use_button, l.use_radius),
                (l.hook, l.hook_radius),
                (l.jump, l.jump_radius),
                (l.zoom_in, 0.8 * u),
                (l.zoom_out, 0.8 * u),
                (l.inventory, 0.9 * u),
                (l.smart, 0.9 * u),
                (l.aim_toggle, 0.9 * u),
                (l.action, l.action_radius),
                (l.quick_heal, 1.1 * u),
                (l.quick_mana, 1.0 * u),
                (l.auto_select, 0.9 * u),
                (l.interact, u),
                (l.lock_on, 0.9 * u),
            ];
            for (i, (a, ra)) in parts.iter().enumerate() {
                assert!(a[0] - ra >= -0.1 && a[0] + ra <= view[0] + 0.1, "{view:?}: 横 {a:?}");
                for (b, rb) in &parts[i + 1..] {
                    assert!(
                        (a[0] - b[0]).hypot(a[1] - b[1]) >= ra + rb - 0.1,
                        "{view:?}: 部品どうし"
                    );
                }
                // 枠 (矩形) と円: 円の中心から矩形までの最短距離が、半径以上。
                for r in &slots {
                    let nearest = [a[0].clamp(r[0], r[2]), a[1].clamp(r[1], r[3])];
                    assert!(
                        (a[0] - nearest[0]).hypot(a[1] - nearest[1]) >= ra - 0.1,
                        "{view:?}: 枠と部品が重ならない {a:?} {r:?}"
                    );
                }
            }
            // 枠の中の指は、その番号の枠。`...` はない。
            let mut pad = TouchPad::default();
            pad.set_viewport(view);
            pad.set_hotbar(HotbarOrientation::Vertical);
            for slot in 0..10 {
                let r = l.slot_rect(slot);
                let center = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
                assert!(pad.update(1, TouchPhase::Started, center));
                pad.update(1, TouchPhase::Ended, center);
                let out = pad.output();
                assert_eq!((out.select_slot, out.toggle_inventory), (Some(slot as u8), false));
            }
        }
    }

    /// ホットバーの枠を外へ引いて離すと投げ、近いままなら選ぶだけ。取り消しでは何もしない。
    #[test]
    fn dragging_a_hotbar_slot_outside_drops_the_stack() {
        let mut pad = pad();
        let l = pad.layout();
        let r = l.slot_rect(2);
        let size = r[2] - r[0];
        let center = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
        // 外へ 3 枠分引いて離す: 投げる。選ぶだけではない。
        pad.update(1, TouchPhase::Started, center);
        assert_eq!(pad.output().select_slot, None, "触れただけでは選ばない");
        pad.update(1, TouchPhase::Moved, [center[0], center[1] + 3.0 * size]);
        pad.update(1, TouchPhase::Ended, [center[0], center[1] + 3.0 * size]);
        let out = pad.output();
        assert_eq!((out.throw_slot, out.select_slot), (Some(2), None));
        assert_eq!(pad.output().throw_slot, None, "1 度だけ");
        // 枠の近く (1 枠分) までなら、選ぶだけ。
        pad.update(2, TouchPhase::Started, center);
        pad.update(2, TouchPhase::Moved, [center[0] + size, center[1]]);
        pad.update(2, TouchPhase::Ended, [center[0] + size, center[1]]);
        let out = pad.output();
        assert_eq!((out.throw_slot, out.select_slot), (None, Some(2)));
        // 取り消し (指が取り上げられた): 選びも投げもしない。
        pad.update(3, TouchPhase::Started, center);
        pad.update(3, TouchPhase::Moved, [center[0], center[1] + 3.0 * size]);
        pad.update(3, TouchPhase::Cancelled, center);
        let out = pad.output();
        assert_eq!((out.throw_slot, out.select_slot), (None, None));
    }

    /// オートセレクトは、押している間 (Hold) か、押すたびの入り切り (Toggle)。ロックとインタラクトは 1 度だけ。
    #[test]
    fn auto_select_modes_and_one_shot_buttons() {
        let mut pad = pad();
        let l = pad.layout();
        assert!(!pad.auto_select() && !pad.output().auto_select);
        pad.update(1, TouchPhase::Started, l.auto_select);
        assert!(pad.auto_select() && pad.output().auto_select, "押している間");
        pad.update(1, TouchPhase::Ended, l.auto_select);
        assert!(!pad.auto_select(), "離したら無効");
        pad.set_auto_mode(AutoSelectMode::Toggle);
        pad.update(2, TouchPhase::Started, l.auto_select);
        pad.update(2, TouchPhase::Ended, l.auto_select);
        assert!(pad.auto_select() && pad.output().auto_select, "入った");
        pad.update(3, TouchPhase::Started, l.auto_select);
        pad.update(3, TouchPhase::Ended, l.auto_select);
        assert!(!pad.auto_select(), "もう一度で切れる");
        pad.update(4, TouchPhase::Started, l.auto_select);
        pad.update(4, TouchPhase::Ended, l.auto_select);
        assert!(pad.auto_select(), "入っている");
        pad.set_auto_mode(AutoSelectMode::Hold);
        assert!(!pad.auto_select(), "動きを切り替えたら、入り切りは戻る");
        pad.release_all();
        // ロックオンとインタラクトのボタンは、触れたフレームに 1 度だけ。
        pad.update(5, TouchPhase::Started, l.lock_on);
        pad.update(5, TouchPhase::Ended, l.lock_on);
        let out = pad.output();
        assert!(out.lock_on && !out.interact);
        assert!(!pad.output().lock_on, "1 度だけ");
        pad.update(6, TouchPhase::Started, l.interact);
        let out = pad.output();
        assert!(out.interact && !out.lock_on);
        assert!(!pad.output().interact);
        pad.set_lock(None, None);
        assert!(!pad.held("lock_on"));
    }

    /// クイック回復・クイックマナのボタンは、触れたフレームに 1 度だけ出る。
    #[test]
    fn quick_buttons_fire_once() {
        let mut pad = pad();
        let l = pad.layout();
        assert_eq!(pad.quick_ready(), [false, false]);
        pad.set_quick(true, false);
        assert_eq!(pad.quick_ready(), [true, false]);
        pad.update(1, TouchPhase::Started, l.quick_heal);
        pad.update(1, TouchPhase::Ended, l.quick_heal);
        let out = pad.output();
        assert!(out.quick_heal && !out.quick_mana);
        assert!(!pad.output().quick_heal, "1 度だけ");
        pad.update(2, TouchPhase::Started, l.quick_mana);
        let out = pad.output();
        assert!(out.quick_mana && !out.quick_heal);
        pad.advance(f32::NAN);
        pad.advance(-1.0);
    }

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
        assert!(!out.use_item && out.world_touch == Some(WORLD) && out.cursor_tilt.is_none());
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
        assert!(pad.output().world_touch.is_some());
        pad.update(2, TouchPhase::Started, [800.0, 300.0]);
        assert!(pad.pinching());
        let out = pad.output();
        assert!(out.world_touch.is_none(), "ピンチ中は使わない");
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
        assert!(pad.output().world_touch.is_none(), "ピンチの名残の指は使わない");
        pad.update(1, TouchPhase::Ended, [700.0, 300.0]);
        pad.update(3, TouchPhase::Started, WORLD);
        assert!(pad.output().world_touch.is_some());
    }

    /// 世界の指も、離す・取り消し・全解除で、押されたままにならない。取りこぼしても、3 本目で古い指を捨てる。
    #[test]
    fn world_touch_never_sticks() {
        let mut pad = pad();
        pad.update(1, TouchPhase::Started, WORLD);
        pad.update(1, TouchPhase::Cancelled, WORLD);
        assert!(!pad.held("world") && pad.output().world_touch.is_none());
        pad.update(2, TouchPhase::Started, WORLD);
        pad.release_all();
        assert!(!pad.held("world") && pad.output().world_touch.is_none());
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
                (l.aim_toggle, 0.9 * u),
                (l.action, l.action_radius),
                (l.quick_heal, 1.1 * u),
                (l.quick_mana, 1.0 * u),
                (l.auto_select, 0.9 * u),
                (l.interact, u),
                (l.lock_on, 0.9 * u),
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
