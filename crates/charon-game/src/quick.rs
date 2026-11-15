//! 仕様 28 §7: クイック回復・クイックマナ。持ち物の消耗品から、体力・マナの回復に使うものを選んで 1 個使う。整数だけ。
//!
//! 使うアイテムの選び方は、ボタンの表示 (使われるアイテムの名前) と、実際に使うときで同じ関数を使う。

use crate::EntityId;
use crate::defs::Definitions;
use crate::game::GameState;
use crate::items::{ItemId, ItemKind};

/// 足りない体力 `missing` を回復するのに使う消耗品を、`options` (アイテム・回復量) から選ぶ。
///
/// - 足りない分を**回復しきれる**ものがあれば、そのうち回復量が**最小**のもの (むだが最も少ない)。
/// - なければ、回復量が**最大**のもの。
/// - 同じ回復量なら、`ItemId` の小さいほう。回復量 0 のもの・`missing` が 0 以下のときは選ばない。
pub fn pick_quick_heal(missing: i32, options: &[(ItemId, u16)]) -> Option<ItemId> {
    if missing <= 0 {
        return None;
    }
    let usable = || options.iter().filter(|(_, heal)| *heal > 0);
    let enough = usable()
        .filter(|(_, heal)| i32::from(*heal) >= missing)
        .min_by_key(|(item, heal)| (*heal, *item));
    enough
        .or_else(|| usable().max_by_key(|(item, heal)| (*heal, std::cmp::Reverse(*item))))
        .map(|(item, _)| *item)
}

/// マナの回復に使う消耗品: `ItemId` が最小のもの (持ち物の左上から最初のもの)。回復量 0 のものは選ばない。
pub fn pick_quick_mana(options: &[(ItemId, u16)]) -> Option<ItemId> {
    options.iter().filter(|(_, mana)| *mana > 0).map(|(item, _)| *item).min()
}

impl GameState {
    /// 持ち物の消耗品のうち、`pick` が選ぶ値 (`heal` か `mana`) を取り出す。
    fn consumable_options(
        &self,
        id: EntityId,
        defs: &Definitions,
        value: impl Fn(u16, u16) -> u16,
    ) -> Vec<(ItemId, u16)> {
        let Some(inventory) = self.inventories.get(&id) else { return Vec::new() };
        inventory
            .item_iter()
            .filter_map(|(item, count)| {
                if count == 0 {
                    return None;
                }
                match defs.item_def(item).map(|d| &d.kind) {
                    Some(ItemKind::Consumable { heal, mana }) => Some((item, value(*heal, *mana))),
                    _ => None,
                }
            })
            .collect()
    }

    /// クイック回復で使うアイテム。満タン・使えるものがない・キャラクターがいなければ `None`。
    pub fn quick_heal_item(&self, id: EntityId, defs: &Definitions) -> Option<ItemId> {
        let vitals = self.vitals.get(id)?;
        if !vitals.alive() {
            return None;
        }
        let missing = vitals.max_hp.saturating_sub(vitals.hp);
        pick_quick_heal(missing, &self.consumable_options(id, defs, |heal, _| heal))
    }

    /// クイックマナで使うアイテム。杖のマナが満タン・使えるものがない・杖がなければ `None`。
    pub fn quick_mana_item(&self, id: EntityId, defs: &Definitions) -> Option<ItemId> {
        let wand = self.wands.get(&id)?;
        if wand.mana >= wand.mana_max || !self.vitals.get(id).is_some_and(|v| v.alive()) {
            return None;
        }
        pick_quick_mana(&self.consumable_options(id, defs, |_, mana| mana))
    }

    /// 消耗品 `item` を 1 個使う (体力・マナを回復して、持ち物から 1 個減らす)。消耗品でない・持っていない・死んでいるなら
    /// 何もせず `false`。
    pub fn use_consumable(&mut self, id: EntityId, item: ItemId, defs: &Definitions) -> bool {
        let Some(ItemKind::Consumable { heal, mana }) = defs.item_def(item).map(|d| d.kind.clone())
        else {
            return false;
        };
        if !self.vitals.get(id).is_some_and(|v| v.alive()) {
            return false;
        }
        let Some(inventory) = self.inventories.get_mut(&id) else { return false };
        if !inventory.take_item(item, 1) {
            return false;
        }
        if let Some(vitals) = self.vitals.get_mut(id) {
            vitals.heal(i32::from(heal));
        }
        if let Some(wand) = self.wands.get_mut(&id) {
            wand.mana = wand.mana.saturating_add(u32::from(mana)).min(wand.mana_max);
        }
        true
    }
}
