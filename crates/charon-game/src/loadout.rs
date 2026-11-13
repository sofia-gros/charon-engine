//! 仕様 19 §4〜§6: クラフト・装備・自動装備。持ち物と装備の出し入れ。失敗したときは何も変えない。

use crate::EntityId;
use charon_core::world::World;

use crate::cursor::{Cursor, CursorState, CursorUse, center_of, cursor_state, usage_of};
use crate::defs::Definitions;
use crate::game::{BuildError, GameState};
use crate::inventory::Inventory;
use crate::items::{EquipSlot, Equipment, ItemClass, ItemId, ItemKind};

/// 持ち物・装備の操作の失敗。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemError {
    /// そのキャラクターは持ち物を持たない。
    NoInventory,
    /// 持ち物にそのアイテムがない。
    NotOwned,
    /// 装備できないアイテム (武器・防具以外、または定義にない)。
    NotEquippable,
    /// 持ち物に入りきらない。
    NoRoom,
    /// 定義にないレシピ。
    UnknownRecipe,
    /// 材料が足りない。
    MissingIngredients,
    /// 作業台が近くにない。
    MissingStation,
}

impl std::fmt::Display for ItemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoInventory => "持ち物がありません",
            Self::NotOwned => "そのアイテムを持っていません",
            Self::NotEquippable => "装備できないアイテムです",
            Self::NoRoom => "持ち物に入りきりません",
            Self::UnknownRecipe => "定義にないレシピです",
            Self::MissingIngredients => "材料が足りません",
            Self::MissingStation => "作業台が近くにありません",
        })
    }
}

impl std::error::Error for ItemError {}

/// 各枠について、持ち物と今の装備の中で強さの点数が最も高いものを装備する (仕様 19 §6)。同点は `ItemId` の小さい方。
/// 今より強いものがある (または枠が空で装備できるものがある) ときだけ変える。変えた枠の数を返す。
pub fn auto_equip_best(
    inventory: &mut Inventory,
    equipment: &mut Equipment,
    defs: &Definitions,
) -> usize {
    let mut changed = 0;
    for slot in EquipSlot::ALL {
        let current = equipment.get(slot);
        let current_rating = current.and_then(|item| defs.item_def(item)).map(|d| d.rating());
        // 持ち物の中で、この枠に入る最良のもの (点数が高い順、同点は ID が小さい順)。
        let mut best: Option<(u32, ItemId)> = None;
        for (item, _) in inventory.item_iter() {
            let Some(def) = defs.item_def(item) else { continue };
            if def.equip_slot() != Some(slot) {
                continue;
            }
            let rating = def.rating();
            // item_iter は ID の昇順なので、同点なら先に見たもの (小さい ID) を残す。
            if best.is_none_or(|(r, _)| rating > r) {
                best = Some((rating, item));
            }
        }
        let Some((rating, item)) = best else { continue };
        if current_rating.is_some_and(|r| r >= rating) {
            continue;
        }
        // 前の装備を持ち物へ返せるかを、取り出したあとの状態で確かめる。
        let mut trial = inventory.clone();
        if !trial.take_item(item, 1) {
            continue;
        }
        if let Some(old) = current
            && trial.add_item(old, 1) != 1
        {
            continue;
        }
        *inventory = trial;
        equipment.set(slot, Some(item));
        changed += 1;
    }
    changed
}

impl GameState {
    /// 持ち物から 1 個を、そのアイテムの枠へ移す。枠に入っていたものは持ち物へもどる。
    pub fn equip(
        &mut self,
        id: EntityId,
        item: ItemId,
        defs: &Definitions,
    ) -> Result<(), ItemError> {
        let slot =
            defs.item_def(item).and_then(|def| def.equip_slot()).ok_or(ItemError::NotEquippable)?;
        let inventory = self.inventories.get(&id).ok_or(ItemError::NoInventory)?;
        let mut trial = inventory.clone();
        if !trial.take_item(item, 1) {
            return Err(ItemError::NotOwned);
        }
        let equipment = self.equipment.entry(id).or_default();
        if let Some(old) = equipment.get(slot)
            && trial.add_item(old, 1) != 1
        {
            return Err(ItemError::NoRoom);
        }
        equipment.set(slot, Some(item));
        self.inventories.insert(id, trial);
        // 杖の武器を装備したら、そのキャラクターの杖が入れ替わる (仕様 20 §1)。
        if let Some(ItemKind::Weapon {
            class: crate::items::WeaponClass::Wand,
            wand: Some(name),
            ..
        }) = defs.item_def(item).map(|d| &d.kind)
            && let Ok(wand) = defs.wand(name)
            && self.wands.contains_key(&id)
        {
            self.wands.insert(id, wand);
        }
        Ok(())
    }

    /// 枠の装備を持ち物へもどす。持ち切れないときは失敗。枠が空なら何もしない。
    pub fn unequip(&mut self, id: EntityId, slot: EquipSlot) -> Result<(), ItemError> {
        let inventory = self.inventories.get_mut(&id).ok_or(ItemError::NoInventory)?;
        let Some(equipment) = self.equipment.get_mut(&id) else { return Ok(()) };
        let Some(item) = equipment.get(slot) else { return Ok(()) };
        if inventory.add_item(item, 1) != 1 {
            return Err(ItemError::NoRoom);
        }
        equipment.set(slot, None);
        Ok(())
    }

    /// 自動装備 (仕様 19 §6)。変えた枠の数を返す。
    pub fn auto_equip(&mut self, id: EntityId, defs: &Definitions) -> Result<usize, ItemError> {
        let inventory = self.inventories.get_mut(&id).ok_or(ItemError::NoInventory)?;
        let equipment = self.equipment.entry(id).or_default();
        Ok(auto_equip_best(inventory, equipment, defs))
    }

    /// レシピ `recipe` で作る。`nearby` は、近くにあるブロックのアイテム (作業台の判定用)。
    pub fn craft(
        &mut self,
        id: EntityId,
        recipe: &str,
        nearby: &[ItemId],
        defs: &Definitions,
    ) -> Result<(), ItemError> {
        let recipe = defs.recipe(recipe).ok_or(ItemError::UnknownRecipe)?;
        if let Some(station) = recipe.station
            && !nearby.contains(&station)
        {
            return Err(ItemError::MissingStation);
        }
        let inventory = self.inventories.get_mut(&id).ok_or(ItemError::NoInventory)?;
        let mut trial = inventory.clone();
        for &(item, count) in &recipe.inputs {
            if !trial.take_item(item, count) {
                return Err(ItemError::MissingIngredients);
            }
        }
        let (out, n) = recipe.output;
        if !trial.can_hold(out, n) {
            return Err(ItemError::NoRoom);
        }
        trial.add_item(out, n);
        *inventory = trial;
        Ok(())
    }
}

/// `use_selected` の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseOutcome {
    /// 使えるものを選んでいない (武器は `GameInput` で撃つ)。
    Nothing,
    Placed,
    Broken,
    /// 消耗品を使った。
    Consumed,
}

/// `use_selected` の失敗。何も変わらない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseError {
    NoCharacter,
    OutOfReach,
    /// 置けない・壊せない場所。
    Invalid,
    Build(BuildError),
}

impl GameState {
    /// ホットバーで選んでいるアイテムを、カーソルの場所に使う (仕様 18 §2)。
    /// ブロックは置く、つるはしはブロックを壊す、消耗品は体力・マナを回復して 1 個使う。
    pub fn use_selected(
        &mut self,
        world: &mut World,
        entities: &mut crate::EntityWorld,
        id: crate::EntityId,
        defs: &Definitions,
        cursor: Cursor,
    ) -> Result<UseOutcome, UseError> {
        let inventory = self.inventories.get(&id).ok_or(UseError::NoCharacter)?;
        let Some(item) = inventory.selected_item() else { return Ok(UseOutcome::Nothing) };
        let usage = usage_of(Some(item), defs);
        if matches!(usage, CursorUse::Place | CursorUse::Mine) {
            let center = center_of(entities, id).ok_or(UseError::NoCharacter)?;
            match cursor_state(world, center, cursor, usage) {
                CursorState::Ok => {}
                CursorState::OutOfReach => return Err(UseError::OutOfReach),
                CursorState::Invalid => return Err(UseError::Invalid),
            }
        }
        match item.class() {
            ItemClass::Block(material) => self
                .place_block_item(world, entities, id, cursor.block, material)
                .map(|_| UseOutcome::Placed)
                .map_err(UseError::Build),
            ItemClass::Def(_) => match defs.item_def(item).map(|d| d.kind.clone()) {
                Some(ItemKind::Tool { .. }) => self
                    .break_block_item(world, id, cursor.block)
                    .map(|_| UseOutcome::Broken)
                    .map_err(UseError::Build),
                Some(ItemKind::Consumable { heal, mana }) => {
                    if let Some(vitals) = self.vitals.get_mut(id) {
                        vitals.heal(i32::from(heal));
                    }
                    if let Some(wand) = self.wands.get_mut(&id) {
                        wand.mana = wand.mana.saturating_add(u32::from(mana)).min(wand.mana_max);
                    }
                    if let Some(inventory) = self.inventories.get_mut(&id) {
                        inventory.take_item(item, 1);
                    }
                    Ok(UseOutcome::Consumed)
                }
                _ => Ok(UseOutcome::Nothing),
            },
            ItemClass::Cells(_) => Ok(UseOutcome::Nothing),
        }
    }
}
