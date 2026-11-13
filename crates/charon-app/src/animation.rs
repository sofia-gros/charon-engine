//! 仕様 06 §10.4: スプライトシートのアニメーション (コマ送り) と、エンティティへの画像の割り当て。
//!
//! 画像 (`SpriteAsset`) は 1 枚のシート。1 行が 1 つの動き (クリップ) で、左から右へコマが並ぶ。
//! エンティティの状態 (立つ・歩く・跳ぶ・落ちる) からクリップを選び、世界の tick からコマを選ぶ。
//! 描画に依存しない純粋な計算で、画面なしで試せる。

use std::collections::BTreeMap;
use std::sync::Arc;

use charon_game::game::Enemy;
use charon_game::{Entity, EntityId, EntityWorld};
use charon_render::{Sprite, SpriteFilter};

/// シートの 1 行 = 1 つの動き。`frames` 枚のコマを `ticks_per_frame` tick ごとに送り、最後まで行ったら最初へ戻る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub row: u16,
    pub frames: u16,
    pub ticks_per_frame: u16,
}

impl Clip {
    /// 1 コマだけの静止 (`row` 行目の左端)。
    pub const fn still(row: u16) -> Self {
        Self { row, frames: 1, ticks_per_frame: 1 }
    }

    /// `ticks` (世界の tick) で見せるコマの番号 (0 始まり)。
    pub fn frame_at(&self, ticks: u64) -> u16 {
        if self.frames <= 1 || self.ticks_per_frame == 0 {
            return 0;
        }
        // `frames` は 2 以上なので、剰余は u16 に収まる。
        ((ticks / u64::from(self.ticks_per_frame)) % u64::from(self.frames)) as u16
    }
}

/// エンティティの見た目の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Idle,
    Walk,
    Jump,
    Fall,
}

impl Pose {
    /// 接地していなければ、上向きの速度なら跳ぶ・そうでなければ落ちる。接地して横に動いていれば歩く。
    pub fn of(entity: &Entity) -> Self {
        if !entity.grounded {
            if entity.vy < 0 { Self::Jump } else { Self::Fall }
        } else if entity.vx != 0 {
            Self::Walk
        } else {
            Self::Idle
        }
    }
}

/// 状態ごとのクリップ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clips {
    pub idle: Clip,
    pub walk: Clip,
    pub jump: Clip,
    pub fall: Clip,
}

impl Clips {
    /// すべての状態が、シートの 1 行目の左端 1 コマ (アニメーションのない静止画)。
    pub const STILL: Self = Self {
        idle: Clip::still(0),
        walk: Clip::still(0),
        jump: Clip::still(0),
        fall: Clip::still(0),
    };

    pub fn clip(&self, pose: Pose) -> Clip {
        match pose {
            Pose::Idle => self.idle,
            Pose::Walk => self.walk,
            Pose::Jump => self.jump,
            Pose::Fall => self.fall,
        }
    }
}

/// ゲームがエンジンに渡す、キャラクターなどの画像 (仕様 06 §10)。画像は大きく作り、`scale` で大きさを決める
/// (例: 32×64 のコマを `scale = 10 / 64` で 5×10 セルに描く)。シートなら `frame_size` で 1 コマの大きさを、`clips` で動きを決める。
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteAsset {
    /// 画像 (シート) 全体の大きさ。
    pub width: u16,
    pub height: u16,
    /// 行優先の RGBA8 (sRGB、ストレートアルファ)。長さは `width * height * 4`。
    pub rgba: Arc<[u8]>,
    /// 1 コマの大きさ (画素)。シートでなければ画像の全体と同じ。
    pub frame_size: [u16; 2],
    /// 基準点のコマの中の位置 (画素)。足元の中央なら `[フレームの幅 / 2, フレームの高さ]`。エンティティの足元の中央に置く。
    pub pivot: [f32; 2],
    /// 画像の 1 画素あたりのセル数。
    pub scale: f32,
    pub filter: SpriteFilter,
    pub clips: Clips,
}

impl SpriteAsset {
    /// アニメーションのない 1 枚の画像。
    pub fn still(
        width: u16,
        height: u16,
        rgba: Arc<[u8]>,
        pivot: [f32; 2],
        scale: f32,
        filter: SpriteFilter,
    ) -> Self {
        Self {
            width,
            height,
            rgba,
            frame_size: [width, height],
            pivot,
            scale,
            filter,
            clips: Clips::STILL,
        }
    }

    /// 状態 `pose`・時刻 `ticks` で描くコマ `[x, y, 幅, 高さ]` (画素)。
    pub fn frame_for(&self, pose: Pose, ticks: u64) -> [u16; 4] {
        let clip = self.clips.clip(pose);
        let [fw, fh] = self.frame_size;
        [clip.frame_at(ticks).saturating_mul(fw), clip.row.saturating_mul(fh), fw, fh]
    }
}

/// 描画に渡すスプライトと、箱を描かないエンティティ。
pub(crate) struct Drawn<'a> {
    pub sprites: Vec<Sprite<'a>>,
    pub skip: Vec<EntityId>,
}

/// 画像を割り当てたエンティティ (プレイヤーと、種類ごとの敵) のスプライトを作る。
/// `facing` はエンティティごとの向き (-1 か 1) で、横に動いたときだけ更新する。`multiplier` は `scale` に掛ける倍率。
pub(crate) fn entity_sprites<'a>(
    player: Option<(EntityId, &'a SpriteAsset)>,
    enemy_assets: &'a BTreeMap<String, SpriteAsset>,
    entities: &EntityWorld,
    enemies: &BTreeMap<EntityId, Enemy>,
    facing: &mut BTreeMap<EntityId, i8>,
    ticks: u64,
    multiplier: f32,
) -> Drawn<'a> {
    let mut drawn = Drawn { sprites: Vec::new(), skip: Vec::new() };
    let mut add =
        |id: u64, asset: &'a SpriteAsset, entity: &Entity, facing: &mut BTreeMap<EntityId, i8>| {
            let direction = facing.entry(entity.id).or_insert(1);
            match entity.vx.signum() {
                1 => *direction = 1,
                -1 => *direction = -1,
                _ => {}
            }
            drawn.sprites.push(Sprite {
                id,
                width: asset.width,
                height: asset.height,
                rgba: &asset.rgba,
                position: [
                    (entity.x16 as f32 + f32::from(entity.width) * 8.0) / 16.0,
                    (entity.y16 as f32 + f32::from(entity.height) * 16.0) / 16.0,
                ],
                pivot: asset.pivot,
                scale: asset.scale * multiplier,
                angle: 0.0,
                flip_x: *direction < 0,
                filter: asset.filter,
                lit: true,
                frame: Some(asset.frame_for(Pose::of(entity), ticks)),
            });
            drawn.skip.push(entity.id);
        };
    if let Some((id, asset)) = player
        && let Some(entity) = entities.get(id)
    {
        add(1, asset, entity, facing);
    }
    for (&id, enemy) in enemies {
        if let (Some(asset), Some(entity)) = (enemy_assets.get(&enemy.def.name), entities.get(id)) {
            add(1_000 + id.0, asset, entity, facing);
        }
    }
    drawn
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use super::*;

    const WALK: Clip = Clip { row: 1, frames: 4, ticks_per_frame: 6 };

    fn entity(vx: i16, vy: i16, grounded: bool) -> Entity {
        Entity {
            id: EntityId(1),
            x16: 0,
            y16: 0,
            vx,
            vy,
            width: 5,
            height: 10,
            grounded,
            ext_vx: 0,
        }
    }

    fn sheet() -> SpriteAsset {
        SpriteAsset {
            width: 128,
            height: 256,
            rgba: vec![0u8; 128 * 256 * 4].into(),
            frame_size: [32, 64],
            pivot: [16.0, 64.0],
            scale: 10.0 / 64.0,
            filter: SpriteFilter::Nearest,
            clips: Clips {
                idle: Clip { row: 0, frames: 2, ticks_per_frame: 30 },
                walk: WALK,
                jump: Clip::still(2),
                fall: Clip::still(3),
            },
        }
    }

    #[test]
    fn clips_advance_and_loop() {
        let frames: Vec<u16> = (0..30).step_by(6).map(|t| WALK.frame_at(t)).collect();
        assert_eq!(frames, [0, 1, 2, 3, 0]);
        assert_eq!(WALK.frame_at(5), 0, "コマの途中は変わらない");
        assert_eq!(WALK.frame_at(24), 0, "最後まで行ったら最初へ");
        assert_eq!(Clip::still(2).frame_at(1_000), 0);
        assert_eq!(
            Clip { row: 0, frames: 3, ticks_per_frame: 0 }.frame_at(9),
            0,
            "間隔 0 は止める"
        );
    }

    #[test]
    fn pose_follows_the_entity() {
        assert_eq!(Pose::of(&entity(0, 0, true)), Pose::Idle);
        assert_eq!(Pose::of(&entity(32, 0, true)), Pose::Walk);
        assert_eq!(Pose::of(&entity(0, -40, false)), Pose::Jump);
        assert_eq!(Pose::of(&entity(0, 20, false)), Pose::Fall);
        assert_eq!(Pose::of(&entity(32, -40, false)), Pose::Jump, "空中は歩きにならない");
    }

    #[test]
    fn frames_pick_the_row_and_column_from_the_sheet() {
        let sheet = sheet();
        assert_eq!(sheet.frame_for(Pose::Idle, 0), [0, 0, 32, 64]);
        assert_eq!(sheet.frame_for(Pose::Idle, 30), [32, 0, 32, 64]);
        assert_eq!(sheet.frame_for(Pose::Walk, 12), [64, 64, 32, 64]);
        assert_eq!(sheet.frame_for(Pose::Jump, 99), [0, 128, 32, 64]);
        assert_eq!(sheet.frame_for(Pose::Fall, 99), [0, 192, 32, 64]);
        let still = SpriteAsset::still(
            32,
            64,
            vec![0u8; 32 * 64 * 4].into(),
            [16.0, 64.0],
            0.1,
            SpriteFilter::Nearest,
        );
        assert_eq!(still.frame_for(Pose::Walk, 77), [0, 0, 32, 64], "静止画は全体");
    }

    #[test]
    fn sprites_are_assigned_to_the_player_and_to_enemy_kinds() {
        let mut entities = EntityWorld::new();
        let player = entities.spawn(0, 0, 5, 10).unwrap();
        let other = entities.spawn(16 * 20, 0, 5, 10).unwrap();
        entities.get_mut(player).unwrap().grounded = true; // 立っている (空中だと「落ちる」のコマ)
        let assets = BTreeMap::new();
        let asset = sheet();
        let mut facing = BTreeMap::new();
        let drawn = entity_sprites(
            Some((player, &asset)),
            &assets,
            &entities,
            &BTreeMap::new(),
            &mut facing,
            0,
            2.0,
        );
        assert_eq!(drawn.sprites.len(), 1);
        assert_eq!(drawn.skip, [player], "画像のエンティティの箱は描かない (他は描く: {other:?})");
        let sprite = drawn.sprites[0];
        assert_eq!(sprite.position, [2.5, 10.0], "足元の中央");
        assert!((sprite.scale - 10.0 / 64.0 * 2.0).abs() < 1e-6, "倍率が掛かる");
        assert_eq!(sprite.frame, Some([0, 0, 32, 64]));
        assert!(!sprite.flip_x);
    }

    #[test]
    fn facing_flips_when_walking_left_and_is_remembered() {
        let mut entities = EntityWorld::new();
        let id = entities.spawn(0, 0, 5, 10).unwrap();
        let asset = sheet();
        let assets = BTreeMap::new();
        let mut facing = BTreeMap::new();
        let flipped = |entities: &EntityWorld, facing: &mut BTreeMap<EntityId, i8>| {
            entity_sprites(Some((id, &asset)), &assets, entities, &BTreeMap::new(), facing, 0, 1.0)
                .sprites[0]
                .flip_x
        };
        assert!(!flipped(&entities, &mut facing));
        entities.get_mut(id).unwrap().vx = -32;
        assert!(flipped(&entities, &mut facing), "左へ歩くと反転する");
        entities.get_mut(id).unwrap().vx = 0;
        assert!(flipped(&entities, &mut facing), "止まっても向きを覚えている");
        entities.get_mut(id).unwrap().vx = 32;
        assert!(!flipped(&entities, &mut facing));
    }
}
