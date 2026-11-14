//! 仕様 10 §13・仕様 19: 持ち物。アイテム (材質のセル・ブロック・定義のアイテム) の個数を持つ。集める・使う・捨てるのはゲームが決める。
//!
//! 保存は **名前** で行う (`Definitions::item_name`)。持つ数は 1 種類につき `STACK_MAX` まで。整数だけで決まる。
//!
//! 今までの材質のセルの API (`add`・`take`・`count`・`iter`) とブロックの API (`add_block`・`take_block`・`block_count`・
//! `block_iter`・`craft_block`) は、そのまま使える (`ItemId` に写して動く)。アイテム全般は `add_item`・`take_item`・
//! `item_count`・`item_iter`。**ホットバー**は 10 枠の「アイテムを指すだけの印」と、選んでいる枠。

use std::collections::BTreeMap;

use charon_core::cell::MaterialId;

use crate::items::{ItemClass, ItemId};

/// 1 つのアイテムを持てる数の上限。
pub const STACK_MAX: u32 = 99_999;
/// 持てるアイテムの種類の上限。
pub const KINDS_MAX: usize = 256;
/// ホットバーの枠の数。
pub const HOTBAR_SLOTS: usize = 10;

/// アイテムごとの数と、ホットバー。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    counts: BTreeMap<u32, u32>,
    hotbar: [Option<ItemId>; HOTBAR_SLOTS],
    selected: u8,
}

/// セルのアイテムにも、ブロックのアイテムにもできない材質 (空・境界)。
fn invalid_material(item: ItemId) -> bool {
    match item.class() {
        ItemClass::Cells(m) | ItemClass::Block(m) => {
            m == MaterialId::EMPTY || m == MaterialId::BOUNDARY
        }
        ItemClass::Def(_) => false,
    }
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- アイテム全般 ----

    /// `item` を `amount` 個足す。上限を超えた分と、種類の上限を超える新しいアイテムは入らない。入った数を返す。
    pub fn add_item(&mut self, item: ItemId, amount: u32) -> u32 {
        if amount == 0 || invalid_material(item) {
            return 0;
        }
        if !self.counts.contains_key(&item.0) && self.counts.len() >= KINDS_MAX {
            return 0;
        }
        let have = self.counts.entry(item.0).or_insert(0);
        let added = amount.min(STACK_MAX - *have);
        *have += added;
        added
    }

    /// `item` を `amount` 個取り出す。足りないときは何も取り出さず `false`。
    pub fn take_item(&mut self, item: ItemId, amount: u32) -> bool {
        let Some(have) = self.counts.get_mut(&item.0) else { return amount == 0 };
        if *have < amount {
            return false;
        }
        *have -= amount;
        if *have == 0 {
            self.counts.remove(&item.0);
        }
        true
    }

    pub fn item_count(&self, item: ItemId) -> u32 {
        self.counts.get(&item.0).copied().unwrap_or(0)
    }

    /// すべてのアイテムを `ItemId` の昇順で返す。
    pub fn item_iter(&self) -> impl Iterator<Item = (ItemId, u32)> + '_ {
        self.counts.iter().map(|(&id, &n)| (ItemId(id), n))
    }

    /// `amount` 個を入れる余地があるか (上限と種類の上限)。
    pub fn can_hold(&self, item: ItemId, amount: u32) -> bool {
        if invalid_material(item) {
            return false;
        }
        match self.counts.get(&item.0) {
            Some(have) => have.saturating_add(amount) <= STACK_MAX,
            None => self.counts.len() < KINDS_MAX && amount <= STACK_MAX,
        }
    }

    // ---- 材質のセル (今までの API) ----

    /// `material` のセルを `amount` 個足す。入った数を返す。
    pub fn add(&mut self, material: MaterialId, amount: u32) -> u32 {
        self.add_item(ItemId::cells(material), amount)
    }

    /// `material` のセルを `amount` 個取り出す。足りないときは何も取り出さず `false`。
    pub fn take(&mut self, material: MaterialId, amount: u32) -> bool {
        self.take_item(ItemId::cells(material), amount)
    }

    pub fn count(&self, material: MaterialId) -> u32 {
        self.item_count(ItemId::cells(material))
    }

    /// 全部のアイテムの数の合計 (セル・ブロック・定義)。
    pub fn total(&self) -> u64 {
        self.counts.values().map(|&n| u64::from(n)).sum()
    }

    /// `(材質, 数)` を材質 ID の昇順で返す (材質のセルだけ)。
    pub fn iter(&self) -> impl Iterator<Item = (MaterialId, u32)> + '_ {
        self.class_iter(0)
    }

    fn class_iter(&self, class: u32) -> impl Iterator<Item = (MaterialId, u32)> + '_ {
        self.counts
            .iter()
            .filter(move |&(&id, _)| id >> 16 == class)
            // 下位 16 ビットは材質 ID。
            .map(|(&id, &n)| (MaterialId((id & 0xFFFF) as u16), n))
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    // ---- ブロック (仕様 17 §4) ----

    /// ブロックのアイテムを `amount` 個足す。入った数を返す。
    pub fn add_block(&mut self, material: MaterialId, amount: u32) -> u32 {
        self.add_item(ItemId::block(material), amount)
    }

    /// ブロックのアイテムを `amount` 個取り出す。足りなければ何も取り出さず `false`。
    pub fn take_block(&mut self, material: MaterialId, amount: u32) -> bool {
        self.take_item(ItemId::block(material), amount)
    }

    pub fn block_count(&self, material: MaterialId) -> u32 {
        self.item_count(ItemId::block(material))
    }

    /// `(材質, 数)` を材質 ID の昇順で返す (ブロックのアイテム)。
    pub fn block_iter(&self) -> impl Iterator<Item = (MaterialId, u32)> + '_ {
        self.class_iter(1)
    }

    /// 材質のセル 64 個 (`BLOCK_CELLS`) を使って、その材質のブロックを 1 個作る。セルが足りない・ブロックが持ち切れない
    /// (上限) ときは、何も変えずに `false`。作ってよい材質かは、呼ぶ側 (固体か粉体か) が決める。
    pub fn craft_block(&mut self, material: MaterialId) -> bool {
        const COST: u32 = crate::blocks::BLOCK_CELLS as u32;
        let block = ItemId::block(material);
        if self.count(material) < COST || !self.can_hold(block, 1) {
            return false;
        }
        self.take(material, COST) && self.add_item(block, 1) == 1
    }

    // ---- ホットバー (仕様 19 §3) ----

    /// 枠 `slot` (0..10) に、アイテムを指す印を付ける。持っていないアイテム・範囲外の枠は `false`。
    /// 同じアイテムを指している別の枠があれば、そちらは外す (1 つのアイテムは 1 枠だけ)。
    pub fn pin(&mut self, slot: usize, item: ItemId) -> bool {
        if slot >= HOTBAR_SLOTS || self.item_count(item) == 0 {
            return false;
        }
        for other in &mut self.hotbar {
            if *other == Some(item) {
                *other = None;
            }
        }
        self.hotbar[slot] = Some(item);
        true
    }

    /// 枠の印を外す。
    pub fn unpin(&mut self, slot: usize) {
        if let Some(entry) = self.hotbar.get_mut(slot) {
            *entry = None;
        }
    }

    /// 二つのホットバーの印を交換する。範囲外では何も変えない。
    pub fn swap_hotbar(&mut self, a: usize, b: usize) -> bool {
        if a >= HOTBAR_SLOTS || b >= HOTBAR_SLOTS {
            return false;
        }
        self.hotbar.swap(a, b);
        true
    }

    /// 枠が指しているアイテム (持っていなくても、印は残っている)。
    pub fn hotbar(&self, slot: usize) -> Option<ItemId> {
        self.hotbar.get(slot).copied().flatten()
    }

    /// 枠が指しているアイテム。持ち物から無くなっていれば、空と同じ (`None`)。
    pub fn hotbar_active(&self, slot: usize) -> Option<ItemId> {
        self.hotbar(slot).filter(|&item| self.item_count(item) > 0)
    }

    /// 枠を選ぶ。範囲外は `false`。
    pub fn select(&mut self, slot: usize) -> bool {
        match u8::try_from(slot) {
            Ok(s) if usize::from(s) < HOTBAR_SLOTS => {
                self.selected = s;
                true
            }
            _ => false,
        }
    }

    /// 選んでいる枠 (0..10)。
    pub fn selected(&self) -> usize {
        usize::from(self.selected)
    }

    /// 選んでいる枠のアイテム (持っていなければ `None`)。
    pub fn selected_item(&self) -> Option<ItemId> {
        self.hotbar_active(self.selected())
    }

    /// 状態を FNV-1a 64-bit で流す (同期の照合用)。セルだけの持ち物は、これまでと同じ値。
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        };
        for (id, n) in self.class_iter(0) {
            feed(&id.0.to_le_bytes());
            feed(&n.to_le_bytes());
        }
        // ブロック・定義・ホットバーがなければ、印も流さない。
        if self.class_iter(1).next().is_some() {
            feed(&[0xB1, 0x0C]);
            for (id, n) in self.class_iter(1) {
                feed(&id.0.to_le_bytes());
                feed(&n.to_le_bytes());
            }
        }
        if self.counts.keys().any(|&id| id >> 16 >= 2) {
            feed(&[0xD0, 0x1E]);
            for (&id, &n) in self.counts.iter().filter(|&(&id, _)| id >> 16 >= 2) {
                feed(&id.to_le_bytes());
                feed(&n.to_le_bytes());
            }
        }
        if self.hotbar.iter().any(Option::is_some) || self.selected != 0 {
            feed(&[0x80, 0x7B]);
            for slot in self.hotbar {
                feed(&slot.map_or(u32::MAX, |item| item.0).to_le_bytes());
            }
            feed(&[self.selected]);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: MaterialId = MaterialId(2);
    const SAND: MaterialId = MaterialId(5);

    #[test]
    fn add_take_and_count() {
        let mut inv = Inventory::new();
        assert!(inv.is_empty());
        assert_eq!(inv.add(STONE, 10), 10);
        assert_eq!(inv.add(SAND, 3), 3);
        assert_eq!((inv.count(STONE), inv.count(SAND), inv.total()), (10, 3, 13));
        assert!(inv.take(STONE, 4));
        assert_eq!(inv.count(STONE), 6);
        assert!(!inv.take(STONE, 7), "足りないときは何も取らない");
        assert_eq!(inv.count(STONE), 6);
        assert!(inv.take(STONE, 6));
        assert_eq!(inv.count(STONE), 0);
        assert!(inv.iter().all(|(m, _)| m == SAND), "0 になったら表から消える");
        assert!(inv.take(MaterialId(9), 0), "0 個は取れる");
        assert!(!inv.take(MaterialId(9), 1));
    }

    #[test]
    fn stacks_and_kinds_are_limited() {
        let mut inv = Inventory::new();
        assert_eq!(inv.add(STONE, STACK_MAX - 1), STACK_MAX - 1);
        assert_eq!(inv.add(STONE, 10), 1, "上限までしか入らない");
        assert_eq!(inv.count(STONE), STACK_MAX);
        assert_eq!(inv.add(STONE, 1), 0);
        for id in 10..10 + KINDS_MAX as u16 {
            inv.add(MaterialId(id), 1);
        }
        assert_eq!(inv.iter().count(), KINDS_MAX, "種類の上限");
        assert_eq!(inv.add(MaterialId(9999), 1), 0, "新しい種類は入らない");
        assert_eq!(inv.add(STONE, 0), 0);
    }

    #[test]
    fn empty_and_boundary_are_never_kept() {
        let mut inv = Inventory::new();
        assert_eq!(inv.add(MaterialId::EMPTY, 5), 0);
        assert_eq!(inv.add(MaterialId::BOUNDARY, 5), 0);
        assert_eq!(inv.add_block(MaterialId::EMPTY, 5), 0);
        assert!(inv.is_empty());
    }

    /// 材質のセル 64 個でブロック 1 個。足りなければ何も変えない。ブロックは別に数え、ハッシュにも入る。
    #[test]
    fn craft_block_needs_64_cells() {
        let mut inv = Inventory::new();
        let plain = inv.state_hash();
        inv.add(SAND, 63);
        assert!(!inv.craft_block(SAND), "63 個では作れない");
        assert_eq!((inv.count(SAND), inv.block_count(SAND)), (63, 0));
        inv.add(SAND, 66);
        assert!(inv.craft_block(SAND));
        assert_eq!((inv.count(SAND), inv.block_count(SAND)), (65, 1), "64 個を使って 1 個");
        assert!(inv.craft_block(SAND));
        assert_eq!((inv.count(SAND), inv.block_count(SAND)), (1, 2));
        assert!(!inv.craft_block(SAND));
        assert!(!inv.is_empty());
        assert_ne!(inv.state_hash(), plain);
        assert!(inv.take_block(SAND, 2) && !inv.take_block(SAND, 1));
        assert_eq!(inv.block_iter().count(), 0);
        // ブロックのアイテムだけでも、持ち物は空ではなく、なければこれまでと同じハッシュ。
        let mut empty = Inventory::new();
        assert_eq!(empty.state_hash(), plain);
        assert_eq!(empty.add_block(STONE, 2), 2);
        assert!(!empty.is_empty());
        assert_ne!(empty.state_hash(), plain);
        assert_eq!(empty.add_block(MaterialId::EMPTY, 1), 0);
        assert_eq!(empty.add_block(STONE, STACK_MAX), STACK_MAX - 2, "上限");
    }

    #[test]
    fn hash_tracks_contents() {
        let mut a = Inventory::new();
        let mut b = Inventory::new();
        a.add(STONE, 2);
        b.add(STONE, 2);
        assert_eq!(a.state_hash(), b.state_hash());
        b.add(SAND, 1);
        assert_ne!(a.state_hash(), b.state_hash());
    }

    /// 定義のアイテムも、セル・ブロックと同じように持てる。セルだけの持ち物のハッシュは、これまでと同じ (固定の式で確かめる)。
    #[test]
    fn inventory_holds_items_and_keeps_old_hashes() {
        let mut cells = Inventory::new();
        cells.add(STONE, 5);
        cells.add(SAND, 3);
        let mut expected: u64 = 0xcbf29ce484222325;
        for (id, n) in [(2u16, 5u32), (5, 3)] {
            for byte in id.to_le_bytes().into_iter().chain(n.to_le_bytes()) {
                expected = (expected ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        assert_eq!(cells.state_hash(), expected, "これまでと同じ値");

        let pick = ItemId::def(3);
        let mut inv = cells.clone();
        assert_eq!(inv.add_item(pick, 2), 2);
        assert_eq!(inv.item_count(pick), 2);
        assert_ne!(inv.state_hash(), expected, "定義のアイテムはハッシュに入る");
        assert_eq!(inv.count(STONE), 5, "セルは変わらない");
        assert_eq!(inv.iter().count(), 2, "iter はセルだけ");
        assert_eq!(inv.item_iter().count(), 3);
        assert_eq!(inv.total(), 10);
        assert!(inv.take_item(pick, 2) && inv.item_count(pick) == 0);
        assert_eq!(inv.state_hash(), expected, "なくなれば、もとの値にもどる");
        assert_eq!(inv.add_item(pick, STACK_MAX + 5), STACK_MAX, "上限");
        assert!(!inv.can_hold(pick, 1));
        assert!(inv.can_hold(ItemId::def(4), 1));
        assert!(!inv.can_hold(ItemId::block(MaterialId::EMPTY), 1));
    }

    /// ホットバーは、アイテムを指すだけ: 持っていないものは指せず、無くなれば空扱い。選んだ枠のアイテムを引ける。
    #[test]
    fn hotbar_pins_selects_and_ignores_missing_items() {
        let mut inv = Inventory::new();
        let (a, b) = (ItemId::def(0), ItemId::def(1));
        assert!(!inv.pin(0, a), "持っていないアイテムは指せない");
        inv.add_item(a, 1);
        inv.add_item(b, 1);
        assert!(inv.pin(0, a) && inv.pin(9, b));
        assert!(!inv.pin(10, a), "範囲外の枠");
        // 同じアイテムは 1 枠だけ: 別の枠に付け直すと、前の枠は外れる。
        assert!(inv.pin(4, a));
        assert_eq!((inv.hotbar(0), inv.hotbar(4)), (None, Some(a)));
        assert!(inv.select(4) && inv.selected() == 4);
        assert_eq!(inv.selected_item(), Some(a));
        assert!(!inv.select(10) && inv.selected() == 4, "範囲外は選べない");
        // 持ち物から無くなったら、印は残るが、空と同じ。もどれば、また使える。
        assert!(inv.take_item(a, 1));
        assert_eq!(
            (inv.hotbar(4), inv.hotbar_active(4), inv.selected_item()),
            (Some(a), None, None)
        );
        inv.add_item(a, 1);
        assert_eq!(inv.selected_item(), Some(a));
        inv.unpin(4);
        assert_eq!(inv.hotbar(4), None);
        inv.unpin(99);
        // ホットバーの状態は、ハッシュに入る (ないときは入らない)。
        let mut plain = Inventory::new();
        plain.add_item(a, 1);
        let before = plain.state_hash();
        plain.pin(2, a);
        assert_ne!(plain.state_hash(), before);
        plain.unpin(2);
        assert_eq!(plain.state_hash(), before);
    }
}
