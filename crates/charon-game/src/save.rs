//! 仕様 10 §13: ゲーム状態 (プレイヤーの体力・息・マナ・位置・持ち物) の保存。
//!
//! 世界 (セル・剛体・背景) の保存 (仕様 04) とは別の `game.ron`。敵や飛んでいる弾は保存せず、読み込んだあとの世界で
//! 再び生まれる。材質は **名前** で保存するので、材質の表が変わっても読める (知らない名前は読み飛ばす)。

use std::path::Path;

use charon_core::material::MaterialTable;
use charon_core::world::World;
use serde::{Deserialize, Serialize};

use crate::game::GameState;
use crate::vitals::BREATH_MAX;
use crate::{EntityId, EntityWorld, collides};

/// 保存形式の版。読めない版は拒否する。
pub const GAME_SAVE_VERSION: u32 = 1;
/// 保存ファイルの名前 (世界の保存先のディレクトリに置く)。
pub const GAME_SAVE_FILE: &str = "game.ron";
/// 読み込む保存ファイルの大きさの上限 (バイト)。
const MAX_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    Io(String),
    Parse(String),
    /// 知らない版。
    Version(u32),
    /// 値が範囲外。
    Invalid(&'static str),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "ゲーム状態の読み書きに失敗: {e}"),
            Self::Parse(e) => write!(f, "game.ron を読めません: {e}"),
            Self::Version(v) => write!(f, "game.ron の版 {v} は読めません"),
            Self::Invalid(why) => write!(f, "game.ron の値が不正です: {why}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// プレイヤー 1 人ぶん。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSave {
    pub hp: i32,
    pub breath: u16,
    pub mana: u32,
    /// 左上の位置 (1/16 セル)。
    pub x16: i64,
    pub y16: i64,
    /// 持ち物: `(材質の名前, 数)`。
    pub inventory: Vec<(String, u32)>,
    /// 持ち物のブロック: `(材質の名前, 数)` (仕様 17 §4)。古い保存にはない項目で、なければ空。
    #[serde(default)]
    pub blocks: Vec<(String, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSave {
    pub version: u32,
    /// 保存したときの世界の tick (参考)。
    pub tick: u64,
    pub player: Option<PlayerSave>,
}

/// `apply` の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// 位置を戻したか (保存した位置が固体と重なるときは戻さない)。
    pub moved: bool,
    /// 持ち物に戻した材質の種類の数。
    pub kinds: usize,
}

impl GameSave {
    /// `id` のキャラクターの状態を写す。体力と杖のないキャラクターは `player: None`。
    pub fn capture(
        game: &GameState,
        entities: &EntityWorld,
        id: EntityId,
        materials: &MaterialTable,
        tick: u64,
    ) -> Self {
        let player = entities.get(id).zip(game.vitals.get(id)).map(|(entity, vitals)| {
            let inventory = game.inventories.get(&id).map_or_else(Vec::new, |inventory| {
                inventory
                    .iter()
                    .map(|(material, count)| (materials.get(material).name.clone(), count))
                    .collect()
            });
            let blocks = game.inventories.get(&id).map_or_else(Vec::new, |inventory| {
                inventory
                    .block_iter()
                    .map(|(material, count)| (materials.get(material).name.clone(), count))
                    .collect()
            });
            PlayerSave {
                hp: vitals.hp,
                breath: vitals.breath,
                mana: game.wands.get(&id).map_or(0, |wand| wand.mana),
                x16: entity.x16,
                y16: entity.y16,
                inventory,
                blocks,
            }
        });
        Self { version: GAME_SAVE_VERSION, tick, player }
    }

    /// 保存した状態を `id` のキャラクターへ戻す。倒れていた (体力 0 の) プレイヤーは、体力を満タンで戻す。
    /// 位置は、そこが固体と重ならないときだけ戻す。体力・マナ・息は、上限に収める。
    pub fn apply(
        &self,
        game: &mut GameState,
        entities: &mut EntityWorld,
        world: &World,
        id: EntityId,
    ) -> Applied {
        let mut applied = Applied::default();
        let Some(saved) = &self.player else { return applied };
        if let Some(vitals) = game.vitals.get_mut(id) {
            vitals.hp = if saved.hp <= 0 { vitals.max_hp } else { saved.hp.min(vitals.max_hp) };
            vitals.breath = saved.breath.min(BREATH_MAX);
        }
        if let Some(wand) = game.wands.get_mut(&id) {
            wand.mana = saved.mana.min(wand.mana_max);
        }
        if let Some(entity) = entities.get_mut(id)
            && !collides(saved.x16, saved.y16, entity.width, entity.height, world)
        {
            entity.x16 = saved.x16;
            entity.y16 = saved.y16;
            entity.vx = 0;
            entity.vy = 0;
            applied.moved = true;
        }
        if let Some(inventory) = game.inventories.get_mut(&id) {
            for (name, count) in &saved.inventory {
                if let Some(material) = world.materials().id(name)
                    && inventory.add(material, *count) > 0
                {
                    applied.kinds += 1;
                }
            }
            for (name, count) in &saved.blocks {
                if let Some(material) = world.materials().id(name) {
                    inventory.add_block(material, *count);
                }
            }
        }
        applied
    }

    pub fn to_ron(&self) -> Result<String, SaveError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| SaveError::Parse(e.to_string()))
    }

    pub fn from_ron(text: &str) -> Result<Self, SaveError> {
        let save: Self = ron::from_str(text).map_err(|e| SaveError::Parse(e.to_string()))?;
        if save.version != GAME_SAVE_VERSION {
            return Err(SaveError::Version(save.version));
        }
        if let Some(player) = &save.player
            && (player.inventory.len() > crate::inventory::KINDS_MAX
                || player.blocks.len() > crate::inventory::KINDS_MAX)
        {
            return Err(SaveError::Invalid("持ち物の種類が多すぎます"));
        }
        Ok(save)
    }

    /// `root/game.ron` に書く (先に一時ファイルへ書いて入れ替える)。
    pub fn write(&self, root: &Path) -> Result<(), SaveError> {
        let text = self.to_ron()?;
        std::fs::create_dir_all(root).map_err(|e| SaveError::Io(e.to_string()))?;
        let temporary = root.join(format!("{GAME_SAVE_FILE}.tmp"));
        std::fs::write(&temporary, text).map_err(|e| SaveError::Io(e.to_string()))?;
        std::fs::rename(&temporary, root.join(GAME_SAVE_FILE))
            .map_err(|e| SaveError::Io(e.to_string()))
    }

    /// `root/game.ron` を読む。なければ `Ok(None)`。
    pub fn read(root: &Path) -> Result<Option<Self>, SaveError> {
        let path = root.join(GAME_SAVE_FILE);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_BYTES => return Err(SaveError::Invalid("大きすぎます")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SaveError::Io(e.to_string())),
        }
        let text = std::fs::read_to_string(&path).map_err(|e| SaveError::Io(e.to_string()))?;
        Self::from_ron(&text).map(Some)
    }
}
