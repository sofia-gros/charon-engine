//! 仕様 10 §13: 持ち物。掘ったセルの材質を数えて持つ。集める・使う・捨てるのはゲームが決める。
//!
//! 材質は **名前** で保存する (材質の表の順序が変わっても読める)。持つ数は材質ごとに `STACK_MAX` まで。
//! 整数だけで決まる。

use std::collections::BTreeMap;

use charon_core::cell::MaterialId;

/// 1 つの材質を持てる数の上限。
pub const STACK_MAX: u32 = 99_999;
/// 持てる材質の種類の上限。
pub const KINDS_MAX: usize = 256;

/// 材質ごとの数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    counts: BTreeMap<u16, u32>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    /// `material` を `amount` 個足す。上限を超えた分と、種類の上限を超える新しい材質は入らない。入った数を返す。
    pub fn add(&mut self, material: MaterialId, amount: u32) -> u32 {
        if material == MaterialId::EMPTY || material == MaterialId::BOUNDARY || amount == 0 {
            return 0;
        }
        if !self.counts.contains_key(&material.0) && self.counts.len() >= KINDS_MAX {
            return 0;
        }
        let have = self.counts.entry(material.0).or_insert(0);
        let added = amount.min(STACK_MAX - *have);
        *have += added;
        added
    }

    /// `material` を `amount` 個取り出す。足りないときは何も取り出さず `false`。
    pub fn take(&mut self, material: MaterialId, amount: u32) -> bool {
        let Some(have) = self.counts.get_mut(&material.0) else { return amount == 0 };
        if *have < amount {
            return false;
        }
        *have -= amount;
        if *have == 0 {
            self.counts.remove(&material.0);
        }
        true
    }

    pub fn count(&self, material: MaterialId) -> u32 {
        self.counts.get(&material.0).copied().unwrap_or(0)
    }

    /// 全部の数の合計。
    pub fn total(&self) -> u64 {
        self.counts.values().map(|&n| u64::from(n)).sum()
    }

    /// `(材質, 数)` を材質 ID の昇順で返す。
    pub fn iter(&self) -> impl Iterator<Item = (MaterialId, u32)> + '_ {
        self.counts.iter().map(|(&id, &n)| (MaterialId(id), n))
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// 状態を FNV-1a 64-bit で流す (同期の照合用)。
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for (&id, &n) in &self.counts {
            for byte in id.to_le_bytes().into_iter().chain(n.to_le_bytes()) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
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
        assert!(inv.is_empty());
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
}
