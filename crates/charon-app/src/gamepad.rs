//! 仕様 10 §11.2: デスクトップのゲームパッド (`gilrs`)。値を `PadOutput` にそろえる。写像は純粋な関数で、画面なしで試せる。

use gilrs::{Axis, Button, Gilrs};

use crate::touch_pad::PadOutput;

/// 軸の値がこれより大きければ「倒した」とみなす (歩く)。
const WALK_THRESHOLD: f32 = 0.35;
/// 左スティックを上に倒してジャンプするしきい値。
const JUMP_THRESHOLD: f32 = 0.7;
/// 右スティックで狙いが変わるしきい値。
const AIM_THRESHOLD: f32 = 0.3;

/// ゲームパッドの 1 フレームぶんの状態 (`gilrs` の値のうち使うもの)。スティックは上が正。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PadState {
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub south: bool,
    pub east: bool,
    pub west: bool,
    pub north: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub dpad_up: bool,
    pub left_shoulder: bool,
    pub right_shoulder: bool,
    pub left_trigger: bool,
    pub right_trigger: bool,
}

/// 写像: 左スティック・十字キーで歩く (上でジャンプ)、`A` (South) でジャンプ、`X` (West)・`R1`・`R2` で武器、
/// `B` (East)・`L1` で道具、`L2` で掘る道具、右スティックで狙う、`Y` (North) で武器 / ブラシの切り替え (`north_pressed` は押した瞬間)。
pub(crate) fn map_state(state: PadState, north_pressed: bool) -> PadOutput {
    let mut out = PadOutput::default();
    if state.left_x > WALK_THRESHOLD || state.dpad_right {
        out.horizontal = 1;
    }
    if state.left_x < -WALK_THRESHOLD || state.dpad_left {
        out.horizontal = if out.horizontal == 1 { 0 } else { -1 };
    }
    out.jump = state.south || state.dpad_up || state.left_y > JUMP_THRESHOLD;
    out.fire = state.west || state.right_shoulder || state.right_trigger;
    out.tool = state.east || state.left_shoulder;
    out.dig = state.left_trigger;
    let length = state.right_x.hypot(state.right_y);
    if length > AIM_THRESHOLD {
        // 画面の y は下が正なので、スティックの上を負にする。
        out.aim = Some((
            (state.right_x / length * 1000.0) as i32,
            (-state.right_y / length * 1000.0) as i32,
        ));
    }
    out.toggle_paint = north_pressed;
    out
}

/// 接続されたゲームパッドを毎フレーム読む。
pub(crate) struct Gamepad {
    gilrs: Gilrs,
    north_was_down: bool,
}

impl std::fmt::Debug for Gamepad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gamepad").finish_non_exhaustive()
    }
}

impl Gamepad {
    /// ゲームパッドの入力を使えない環境 (対応するバックエンドがない) では `None`。
    pub(crate) fn new() -> Option<Self> {
        Gilrs::new().ok().map(|gilrs| Self { gilrs, north_was_down: false })
    }

    /// 最初に見つかったゲームパッドの今の操作。つながっていなければ何も操作しない。
    pub(crate) fn poll(&mut self) -> PadOutput {
        // イベントを読み切って、内部の状態を更新する。
        while self.gilrs.next_event().is_some() {}
        let Some((_, pad)) = self.gilrs.gamepads().next() else { return PadOutput::default() };
        let state = PadState {
            left_x: pad.value(Axis::LeftStickX),
            left_y: pad.value(Axis::LeftStickY),
            right_x: pad.value(Axis::RightStickX),
            right_y: pad.value(Axis::RightStickY),
            south: pad.is_pressed(Button::South),
            east: pad.is_pressed(Button::East),
            west: pad.is_pressed(Button::West),
            north: pad.is_pressed(Button::North),
            dpad_left: pad.is_pressed(Button::DPadLeft),
            dpad_right: pad.is_pressed(Button::DPadRight),
            dpad_up: pad.is_pressed(Button::DPadUp),
            left_shoulder: pad.is_pressed(Button::LeftTrigger),
            right_shoulder: pad.is_pressed(Button::RightTrigger),
            left_trigger: pad.is_pressed(Button::LeftTrigger2),
            right_trigger: pad.is_pressed(Button::RightTrigger2),
        };
        let north_pressed = state.north && !self.north_was_down;
        self.north_was_down = state.north;
        map_state(state, north_pressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticks_and_buttons_map_to_actions() {
        let walk = map_state(PadState { left_x: 0.8, ..Default::default() }, false);
        assert_eq!(walk.horizontal, 1);
        let left = map_state(PadState { dpad_left: true, ..Default::default() }, false);
        assert_eq!(left.horizontal, -1);
        let both =
            map_state(PadState { dpad_left: true, dpad_right: true, ..Default::default() }, false);
        assert_eq!(both.horizontal, 0, "左右が同時なら中立");
        assert!(map_state(PadState { south: true, ..Default::default() }, false).jump);
        assert!(
            map_state(PadState { left_y: 0.9, ..Default::default() }, false).jump,
            "上に倒してジャンプ"
        );
        let fire = map_state(PadState { right_trigger: true, ..Default::default() }, false);
        assert!(fire.fire && !fire.tool);
        let tool = map_state(PadState { east: true, ..Default::default() }, false);
        assert!(tool.tool && !tool.fire && !tool.dig);
        let dig = map_state(PadState { left_trigger: true, ..Default::default() }, false);
        assert!(dig.dig && !dig.tool, "L2 は掘る道具");
        assert_eq!(
            map_state(PadState::default(), false),
            PadOutput::default(),
            "触れなければ何もしない"
        );
        assert!(map_state(PadState::default(), true).toggle_paint);
    }

    #[test]
    fn right_stick_aims_with_screen_down_positive() {
        // スティックを右上へ倒すと、画面では右 (x 正)・上 (y 負)。
        let up_right =
            map_state(PadState { right_x: 0.7, right_y: 0.7, ..Default::default() }, false);
        let (x, y) = up_right.aim.unwrap_or_default();
        assert!(x > 0 && y < 0 && (x + y).abs() < 5, "{x}, {y}");
        let small = map_state(PadState { right_x: 0.1, ..Default::default() }, false);
        assert_eq!(small.aim, None, "小さな傾きでは狙いを変えない");
    }
}
