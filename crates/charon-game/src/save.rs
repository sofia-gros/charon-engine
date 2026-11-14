//! 仕様 10 §13: ゲーム状態 (プレイヤーの体力・息・マナ・位置・持ち物) の保存。
//!
//! 世界 (セル・剛体・背景) の保存 (仕様 04) とは別の `game.ron`。敵や飛んでいる弾は保存せず、読み込んだあとの世界で
//! 再び生まれる。材質は **名前** で保存するので、材質の表が変わっても読める (知らない名前は読み飛ばす)。

use std::path::Path;

use charon_core::material::MaterialTable;
use charon_core::world::World;
use serde::{Deserialize, Serialize};

use crate::defs::Definitions;
use crate::game::GameState;
use crate::inventory::HOTBAR_SLOTS;
use crate::items::{EquipSlot, ItemClass, ItemId};
use crate::progression::{
    NAME_MAX, PROGRESS_MAX, ProgressSave, Progression, canonical_region, insert_name,
};
use crate::vitals::BREATH_MAX;
use crate::{EntityId, EntityWorld, collides};

/// 保存形式の版。読めない版は拒否する。
pub const GAME_SAVE_VERSION: u32 = 2;
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
    /// 定義のアイテム: `(名前, 数)` (仕様 19 §7)。古い保存にはなく、なければ空。
    #[serde(default)]
    pub items: Vec<(String, u32)>,
    /// ホットバー 10 枠のアイテムの名前。
    #[serde(default)]
    pub hotbar: Vec<Option<String>>,
    /// 選んでいるホットバーの枠。
    #[serde(default)]
    pub selected: u8,
    /// 装備 5 枠 (`EquipSlot::ALL` の順) のアイテムの名前。
    #[serde(default)]
    pub equipment: Vec<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSave {
    pub version: u32,
    /// 保存したときの世界の tick (参考)。
    pub tick: u64,
    pub player: Option<PlayerSave>,
    /// 世界共有の進行。旧版には存在しない。
    #[serde(default)]
    pub progress: ProgressSave,
}

/// 版 1 には進行欄がなく、空で開始する。旧保存の名前付き持ち物は保持する。
fn migrate_v1_to_v2(mut save: GameSave) -> GameSave {
    save.progress = ProgressSave::default();
    save.version = GAME_SAVE_VERSION;
    save
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
    /// プレイヤーがいない世界の進行だけを保存する。
    pub fn capture_progress_only(game: &GameState, tick: u64) -> Self {
        Self { version: GAME_SAVE_VERSION, tick, player: None, progress: game.progress.capture() }
    }

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
                items: Vec::new(),
                hotbar: Vec::new(),
                selected: 0,
                equipment: Vec::new(),
            }
        });
        Self { version: GAME_SAVE_VERSION, tick, player, progress: game.progress.capture() }
    }

    /// 定義に存在する名前だけを新しい世界の進行へ戻す。再適用で増殖しない。
    pub fn apply_progress(&self, game: &mut GameState, defs: &Definitions) {
        let mut progress = Progression::default();
        for name in &self.progress.bosses {
            if defs.enemy(name).is_some_and(|enemy| enemy.boss) {
                insert_name(&mut progress.bosses, name);
            }
        }
        for name in &self.progress.recipes {
            if defs.recipe(name).is_some() {
                insert_name(&mut progress.recipes, name);
            }
        }
        for name in &self.progress.regions {
            if canonical_region(name) {
                insert_name(&mut progress.regions, name);
            }
        }
        game.progress = progress;
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

    /// 定義のアイテム・ホットバー・装備を写す (仕様 19 §7)。`capture` のあとに呼ぶ。名前は定義から引く。
    #[must_use]
    pub fn with_items(
        mut self,
        game: &GameState,
        id: EntityId,
        defs: &Definitions,
        materials: &MaterialTable,
    ) -> Self {
        let Some(player) = &mut self.player else { return self };
        let name = |item: ItemId| defs.item_name(item, materials);
        if let Some(inventory) = game.inventories.get(&id) {
            player.items = inventory
                .item_iter()
                .filter(|(item, _)| matches!(item.class(), ItemClass::Def(_)))
                .filter_map(|(item, count)| Some((name(item)?, count)))
                .collect();
            player.hotbar =
                (0..HOTBAR_SLOTS).map(|slot| inventory.hotbar(slot).and_then(name)).collect();
            // 選んでいる枠は 0..10 に収まる。
            player.selected = inventory.selected() as u8;
        }
        if let Some(equipment) = game.equipment.get(&id) {
            player.equipment =
                EquipSlot::ALL.map(|slot| equipment.get(slot).and_then(name)).to_vec();
        }
        self
    }

    /// `with_items` で写したものを戻す。`apply` のあとに呼ぶ (ホットバーは、戻した持ち物にあるアイテムだけを指す)。
    /// 知らない名前は読み飛ばす。戻した装備の数を返す。
    pub fn apply_items(
        &self,
        game: &mut GameState,
        id: EntityId,
        defs: &Definitions,
        materials: &MaterialTable,
    ) -> usize {
        let Some(saved) = &self.player else { return 0 };
        let lookup = |name: &String| defs.item_id(name, materials);
        if let Some(inventory) = game.inventories.get_mut(&id) {
            for (name, count) in &saved.items {
                if let Some(item) = lookup(name) {
                    inventory.add_item(item, *count);
                }
            }
            for (slot, name) in saved.hotbar.iter().take(HOTBAR_SLOTS).enumerate() {
                if let Some(item) = name.as_ref().and_then(lookup) {
                    inventory.pin(slot, item);
                }
            }
            inventory.select(usize::from(saved.selected));
        }
        let mut restored = 0;
        if !saved.equipment.is_empty() {
            let equipment = game.equipment.entry(id).or_default();
            for (slot, name) in EquipSlot::ALL.into_iter().zip(&saved.equipment) {
                // 枠に合わないアイテムは戻さない。
                let item = name.as_ref().and_then(lookup).filter(|&item| {
                    defs.item_def(item).and_then(|def| def.equip_slot()) == Some(slot)
                });
                if let Some(item) = item {
                    equipment.set(slot, Some(item));
                    restored += 1;
                }
            }
        }
        restored
    }

    pub fn to_ron(&self) -> Result<String, SaveError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| SaveError::Parse(e.to_string()))
    }

    pub fn from_ron(text: &str) -> Result<Self, SaveError> {
        let save: Self = ron::from_str(text).map_err(|e| SaveError::Parse(e.to_string()))?;
        let save = match save.version {
            1 => migrate_v1_to_v2(save),
            GAME_SAVE_VERSION => save,
            other => return Err(SaveError::Version(other)),
        };
        for names in [&save.progress.bosses, &save.progress.recipes, &save.progress.regions] {
            if names.len() > PROGRESS_MAX || names.iter().any(|name| name.len() > NAME_MAX) {
                return Err(SaveError::Invalid("進行の名前が多すぎるか長すぎます"));
            }
        }
        if let Some(player) = &save.player
            && (player.inventory.len() > crate::inventory::KINDS_MAX
                || player.blocks.len() > crate::inventory::KINDS_MAX
                || player.items.len() > crate::inventory::KINDS_MAX
                || player.hotbar.len() > crate::inventory::HOTBAR_SLOTS
                || player.equipment.len() > crate::items::EQUIP_SLOTS)
        {
            return Err(SaveError::Invalid("持ち物の種類が多すぎます"));
        }
        let mut save = save;
        save.progress.bosses.sort();
        save.progress.bosses.dedup();
        save.progress.recipes.sort();
        save.progress.recipes.dedup();
        save.progress.regions.sort();
        save.progress.regions.dedup();
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
