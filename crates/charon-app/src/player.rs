//! 仕様 09 §4: キーボードからプレイヤー入力と自由カメラへ。追従カメラの位置とプレイヤーの生成。

use charon_core::coords::CellPos;
use charon_core::world::World;
use charon_game::game::GameInput;
use charon_game::{Entity, EntityError, EntityId, EntityWorld, PlayerInput};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// キーが押されて起きる操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCommand {
    /// `F`: 自由カメラの切り替え。
    ToggleFreeCamera,
    /// `B`: 左ボタンの割り当て (武器 / ブラシ) の切り替え。
    TogglePaint,
}

/// 押されているキー。`A`/`D`/`←`/`→` が左右、`W`/`↑`/`Space`/`C` がジャンプ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    space: bool,
    shift: bool,
}

impl KeyState {
    /// 対応するキーなら状態を更新して true。対応しないキーは何もしない。
    pub fn set(&mut self, code: KeyCode, pressed: bool) -> bool {
        match code {
            KeyCode::KeyA | KeyCode::ArrowLeft => self.left = pressed,
            KeyCode::KeyD | KeyCode::ArrowRight => self.right = pressed,
            KeyCode::KeyW | KeyCode::ArrowUp => self.up = pressed,
            KeyCode::KeyS | KeyCode::ArrowDown => self.down = pressed,
            KeyCode::Space | KeyCode::KeyC => self.space = pressed,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.shift = pressed,
            _ => return false,
        }
        true
    }

    /// 全部のキーを離した状態に戻す (ウィンドウがフォーカスを失ったとき)。
    pub fn release_all(&mut self) {
        *self = Self::default();
    }

    /// ウィンドウのイベントを反映する。`allow_press` が偽のときは押下を無視する (UI の文字入力中)。
    /// `F` / `B` が押された (繰り返しでない) とき、対応する操作を返す。
    pub fn event(&mut self, event: &WindowEvent, allow_press: bool) -> Option<KeyCommand> {
        let WindowEvent::KeyboardInput { event, .. } = event else { return None };
        let PhysicalKey::Code(code) = event.physical_key else { return None };
        let pressed = event.state == ElementState::Pressed;
        if pressed && !allow_press {
            return None;
        }
        let command = match code {
            KeyCode::KeyF => Some(KeyCommand::ToggleFreeCamera),
            KeyCode::KeyB => Some(KeyCommand::TogglePaint),
            _ => {
                self.set(code, pressed);
                return None;
            }
        };
        if pressed && !event.repeat { command } else { None }
    }

    /// 1 tick の入力。左右が同時なら中立。
    pub fn player_input(&self) -> PlayerInput {
        PlayerInput {
            horizontal: i8::from(self.right) - i8::from(self.left),
            jump: self.up || self.space,
        }
    }

    /// 自由カメラの 1 フレームの移動量 (セル)。`Shift` で 12、通常は 4。
    pub fn camera_delta(&self) -> [i32; 2] {
        let speed = if self.shift { 12 } else { 4 };
        [
            (i32::from(self.right) - i32::from(self.left)) * speed,
            (i32::from(self.down) - i32::from(self.up)) * speed,
        ]
    }
}

/// プレイヤーの中心がシーンの中央になるカメラの原点 (セル、整数に切り捨て)。
pub fn follow_origin(entity: &Entity, viewport: [u32; 2], zoom: u32) -> [f32; 2] {
    let zoom = zoom.max(1);
    let scene = [i64::from(viewport[0].div_ceil(zoom)), i64::from(viewport[1].div_ceil(zoom))];
    let center_x = entity.x16.div_euclid(16) + i64::from(entity.width) / 2;
    let center_y = entity.y16.div_euclid(16) + i64::from(entity.height) / 2;
    [(center_x - scene[0] / 2) as f32, (center_y - scene[1] / 2) as f32]
}

/// 仕様 10 §9.5: マウスのボタンをゲームの入力へ。**左手 = 武器 (左ボタン)、右手 = 道具 (右ボタン)**。
/// 描画モード (`paint`) のあいだは左ボタンがブラシなので武器は撃たない。自由カメラのあいだは何も使わない。
/// `aim` は 2 つで共通の狙い (プレイヤーがいなければ `None`)。
pub(crate) fn game_input(
    left: bool,
    right: bool,
    paint: bool,
    free_camera: bool,
    aim: Option<(i32, i32)>,
) -> GameInput {
    if free_camera {
        return GameInput::default();
    }
    GameInput { fire: left && !paint, tool: right, aim: aim.unwrap_or((0, 0)) }
}

/// プレイヤーの中心からカーソルのセルの中心への向き (1/16 セル単位)。仕様 10 §6.3。
pub(crate) fn aim_at(entity: &Entity, target: CellPos) -> (i32, i32) {
    let center =
        (entity.x16 + i64::from(entity.width) * 8, entity.y16 + i64::from(entity.height) * 8);
    let delta = |t: i32, c: i64| {
        let v = i64::from(t) * 16 + 8 - c;
        // 狙いの上限 (仕様 10 §5.2) に収める。
        v.clamp(-1_000_000, 1_000_000) as i32
    };
    (delta(target.x, center.0), delta(target.y, center.1))
}

/// 幅 `width`・高さ `height` の箱とその足元 1 行を含む 4 隅のチャンクが、全部読み込まれているか。
pub(crate) fn area_loaded(world: &World, spawn: [i32; 2], width: u8, height: u8) -> bool {
    let (w, h) = (i32::from(width), i32::from(height));
    [(0, 0), (w - 1, 0), (0, h), (w - 1, h)]
        .iter()
        .all(|&(dx, dy)| world.chunk(CellPos::new(spawn[0] + dx, spawn[1] + dy).chunk()).is_some())
}

/// 8×16 のプレイヤーとその足元 1 行を含む 4 隅のチャンクが全部読み込まれていれば生成する。
/// 読み込み前なら `Ok(None)`。固体と重なるなら `Err` (地形は消さない)。
pub fn try_spawn_player(
    entities: &mut EntityWorld,
    world: &World,
    spawn: [i32; 2],
) -> Result<Option<EntityId>, EntityError> {
    if !area_loaded(world, spawn, 8, 16) {
        return Ok(None);
    }
    entities.spawn_player(i64::from(spawn[0]) * 16, i64::from(spawn[1]) * 16, world).map(Some)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use super::*;
    use charon_core::coords::ChunkPos;
    use charon_core::testkit::{box_art, world_from_ascii};

    #[test]
    fn keyboard_maps_to_input() {
        let mut keys = KeyState::default();
        assert_eq!(keys.player_input(), PlayerInput::default());
        assert!(keys.set(KeyCode::KeyD, true));
        assert_eq!(keys.player_input(), PlayerInput { horizontal: 1, jump: false });
        // 左右が同時なら中立。離すと残った方向。
        assert!(keys.set(KeyCode::ArrowLeft, true));
        assert_eq!(keys.player_input().horizontal, 0);
        keys.set(KeyCode::KeyD, false);
        assert_eq!(keys.player_input().horizontal, -1);
        // ジャンプは W / ↑ / Space / C のどれでも。
        for code in [KeyCode::KeyW, KeyCode::ArrowUp, KeyCode::Space, KeyCode::KeyC] {
            let mut keys = KeyState::default();
            keys.set(code, true);
            assert!(keys.player_input().jump, "{code:?}");
            keys.set(code, false);
            assert!(!keys.player_input().jump, "{code:?}");
        }
        // 対応しないキーは無視する。
        assert!(!keys.set(KeyCode::KeyQ, true));
        // 自由カメラ: 4 セル、Shift で 12 セル。上は y が負。
        let mut keys = KeyState::default();
        keys.set(KeyCode::KeyW, true);
        keys.set(KeyCode::KeyD, true);
        assert_eq!(keys.camera_delta(), [4, -4]);
        keys.set(KeyCode::ShiftLeft, true);
        assert_eq!(keys.camera_delta(), [12, -12]);
        keys.release_all();
        assert_eq!(keys, KeyState::default());
        // どのキーの組み合わせでも入力は有効な値 (`step_players` が受け付ける)。
        let all = [
            KeyCode::KeyA,
            KeyCode::KeyD,
            KeyCode::KeyW,
            KeyCode::KeyS,
            KeyCode::Space,
            KeyCode::ShiftLeft,
        ];
        for bits in 0..64u8 {
            let mut keys = KeyState::default();
            for (i, code) in all.into_iter().enumerate() {
                keys.set(code, (bits >> i) & 1 == 1);
            }
            assert!((-1..=1).contains(&keys.player_input().horizontal), "bits {bits}");
        }
    }

    #[test]
    fn player_follows_camera() {
        let mut entities = EntityWorld::new();
        let id = entities.spawn(640, 3968, 8, 16).unwrap();
        let player = entities.get(id).unwrap().clone();
        for (viewport, zoom) in
            [([1024, 768], 4), ([1920, 1016], 4), ([800, 600], 1), ([333, 111], 3)]
        {
            let origin = follow_origin(&player, viewport, zoom);
            let scene = [viewport[0].div_ceil(zoom) / 2, viewport[1].div_ceil(zoom) / 2];
            // プレイヤーの中心 (44, 256) がシーンの中央に来る。
            assert_eq!(
                [origin[0] + scene[0] as f32, origin[1] + scene[1] as f32],
                [44.0, 256.0],
                "viewport {viewport:?}, zoom {zoom}"
            );
        }
        // 負座標でも切り捨てが一貫する: x = -2、y = -3 のセル、中心は (2, 5)。
        let id = entities.spawn(-24, -40, 8, 16).unwrap();
        let negative = entities.get(id).unwrap().clone();
        assert_eq!(follow_origin(&negative, [64, 64], 1), [2.0 - 32.0, 5.0 - 32.0]);
    }

    #[test]
    fn mouse_buttons_map_to_weapon_and_tool() {
        let aim = Some((3, -4));
        // 左ボタン = 武器、右ボタン = 道具。独立していて、同時にも使える。
        assert_eq!(
            game_input(true, false, false, false, aim),
            GameInput { fire: true, tool: false, aim: (3, -4) }
        );
        assert_eq!(
            game_input(false, true, false, false, aim),
            GameInput { fire: false, tool: true, aim: (3, -4) }
        );
        assert_eq!(
            game_input(true, true, false, false, aim),
            GameInput { fire: true, tool: true, aim: (3, -4) }
        );
        assert!(!game_input(false, false, false, false, aim).fire);
        // 描画モードでは左ボタンはブラシで、武器にならない。右ボタンの道具は使える。
        assert_eq!(
            game_input(true, true, true, false, aim),
            GameInput { fire: false, tool: true, aim: (3, -4) }
        );
        // 自由カメラのあいだは、どちらも使わない。
        assert_eq!(game_input(true, true, false, true, aim), GameInput::default());
        // プレイヤーがいなければ狙いは (0, 0)。
        assert_eq!(game_input(true, true, false, false, None).aim, (0, 0));
    }

    #[test]
    fn aim_points_from_player_to_cursor() {
        let mut entities = EntityWorld::new();
        let id = entities.spawn(0, 0, 8, 16).unwrap();
        let player = entities.get(id).unwrap().clone();
        // 中心は (64, 128)。セル (10, 2) の中心は (168, 40)。
        assert_eq!(aim_at(&player, CellPos::new(10, 2)), (104, -88));
        // 自分のいるセルの中心との差は 0 にならない (向きがある)。
        assert_ne!(aim_at(&player, CellPos::new(4, 8)), (0, 0));
        // 遠すぎる点は上限に収める。
        assert_eq!(aim_at(&player, CellPos::new(i32::MAX, i32::MIN)), (1_000_000, -1_000_000));
    }

    #[test]
    fn player_spawns_when_chunks_load() {
        let mut world = world_from_ascii(&box_art(60, 60), 1);
        let mut entities = EntityWorld::new();
        // 足元 (y + 16) が別のチャンクにあり、それが未読み込みなら生成しない。
        assert_eq!(try_spawn_player(&mut entities, &world, [20, 50]), Ok(None));
        world.insert_chunk(ChunkPos::new(0, 1));
        let id = try_spawn_player(&mut entities, &world, [20, 40]).unwrap();
        assert!(id.is_some());
        assert!(entities.get(id.unwrap()).is_some());
        // 固体に重なる位置はエラーで、エンティティも ID も変えない。
        let before = entities.state_hash();
        assert_eq!(try_spawn_player(&mut entities, &world, [0, 0]), Err(EntityError::Occupied));
        assert_eq!(entities.state_hash(), before);
    }
}
