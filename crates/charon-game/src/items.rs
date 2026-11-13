//! アイテム (仕様 19): 材質のセル・ブロック・定義 (`items.ron`) のアイテム、装備。整数だけ。

use charon_core::cell::MaterialId;

/// アイテムの識別子 (仕様 19 §1)。保存は名前で行う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u32);

const BLOCK_BASE: u32 = 0x1_0000;
const DEF_BASE: u32 = 0x2_0000;

/// `ItemId` の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemClass {
    /// 材質のセル。
    Cells(MaterialId),
    /// ブロックのアイテム。
    Block(MaterialId),
    /// `items.ron` の定義 (表の添字)。
    Def(usize),
}

impl ItemId {
    /// 材質のセルのアイテム。
    pub const fn cells(material: MaterialId) -> Self {
        Self(material.0 as u32)
    }

    /// ブロックのアイテム。
    pub const fn block(material: MaterialId) -> Self {
        Self(BLOCK_BASE | material.0 as u32)
    }

    /// 定義のアイテム (`index` は定義の表の添字)。
    pub const fn def(index: u16) -> Self {
        Self(DEF_BASE | index as u32)
    }

    pub const fn class(self) -> ItemClass {
        let low = (self.0 & 0xFFFF) as u16;
        match self.0 >> 16 {
            0 => ItemClass::Cells(MaterialId(low)),
            1 => ItemClass::Block(MaterialId(low)),
            _ => ItemClass::Def(low as usize),
        }
    }
}

/// 武器の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Melee,
    Ranged,
    Wand,
}

/// 防具の枠。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArmorSlot {
    Head,
    Body,
    Legs,
    Accessory,
}

/// 属性への耐性 (%)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resist {
    pub fire: u8,
    pub poison: u8,
    pub cold: u8,
}

/// アイテムの種類 (仕様 19 §2)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Misc,
    /// つるはしの力。
    Tool {
        pick: u16,
    },
    Weapon {
        class: WeaponClass,
        damage: u16,
        wand: Option<String>,
    },
    Armor {
        slot: ArmorSlot,
        defense: u16,
        resist: Resist,
    },
    Consumable {
        heal: u16,
        mana: u16,
    },
}

/// `items.ron` の 1 つの定義。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemDef {
    pub name: String,
    pub kind: ItemKind,
}

impl ItemDef {
    /// 強さの点数 (自動装備・並べ替え用、仕様 19 §2)。
    pub fn rating(&self) -> u32 {
        match &self.kind {
            ItemKind::Weapon { damage, .. } => u32::from(*damage),
            ItemKind::Armor { defense, resist, .. } => {
                u32::from(*defense) * 2
                    + u32::from(resist.fire)
                    + u32::from(resist.poison)
                    + u32::from(resist.cold)
            }
            ItemKind::Tool { pick } => u32::from(*pick),
            ItemKind::Misc | ItemKind::Consumable { .. } => 0,
        }
    }

    /// 装備できる枠。装備できないもの (道具・消耗品・その他) は `None`。
    pub fn equip_slot(&self) -> Option<EquipSlot> {
        match &self.kind {
            ItemKind::Weapon { .. } => Some(EquipSlot::Weapon),
            ItemKind::Armor { slot, .. } => Some(match slot {
                ArmorSlot::Head => EquipSlot::Head,
                ArmorSlot::Body => EquipSlot::Body,
                ArmorSlot::Legs => EquipSlot::Legs,
                ArmorSlot::Accessory => EquipSlot::Accessory,
            }),
            _ => None,
        }
    }
}

/// 装備の枠 (仕様 19 §5)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EquipSlot {
    Weapon,
    Head,
    Body,
    Legs,
    Accessory,
}

/// 装備の枠の数。
pub const EQUIP_SLOTS: usize = 5;

impl EquipSlot {
    pub const ALL: [EquipSlot; EQUIP_SLOTS] =
        [Self::Weapon, Self::Head, Self::Body, Self::Legs, Self::Accessory];

    pub const fn index(self) -> usize {
        match self {
            Self::Weapon => 0,
            Self::Head => 1,
            Self::Body => 2,
            Self::Legs => 3,
            Self::Accessory => 4,
        }
    }
}

/// キャラクターの装備 (仕様 19 §5)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Equipment {
    slots: [Option<ItemId>; EQUIP_SLOTS],
}

/// 装備の能力値 (仕様 19 §5)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EquipStats {
    /// 防具の防御の合計。
    pub defense: u32,
    /// 耐性 (%) の合計。それぞれ 100 を上限にする。
    pub resist: Resist,
    /// 武器の基本のダメージ (装備していなければ 0)。
    pub damage: u32,
    pub weapon_class: Option<WeaponClass>,
}

impl Equipment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: EquipSlot) -> Option<ItemId> {
        self.slots[slot.index()]
    }

    /// 枠のアイテムを置き換える (前のものを返す)。持ち物との出し入れは、`GameState` の `equip` が行う。
    pub fn set(&mut self, slot: EquipSlot, item: Option<ItemId>) -> Option<ItemId> {
        std::mem::replace(&mut self.slots[slot.index()], item)
    }

    /// `(枠, アイテム)` を枠の順に。
    pub fn iter(&self) -> impl Iterator<Item = (EquipSlot, ItemId)> + '_ {
        EquipSlot::ALL.into_iter().filter_map(|slot| self.get(slot).map(|item| (slot, item)))
    }

    /// 状態を FNV-1a 64-bit で流す (同期の照合用)。
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for slot in self.slots {
            let value = slot.map_or(u32::MAX, |item| item.0);
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        hash
    }

    /// 能力値。`lookup` は、定義のアイテムの `ItemDef` を引く。
    pub fn stats<'a>(&self, lookup: impl Fn(ItemId) -> Option<&'a ItemDef>) -> EquipStats {
        let mut stats = EquipStats::default();
        let (mut fire, mut poison, mut cold) = (0u32, 0u32, 0u32);
        for (_, item) in self.iter() {
            let Some(def) = lookup(item) else { continue };
            match &def.kind {
                ItemKind::Armor { defense, resist, .. } => {
                    stats.defense += u32::from(*defense);
                    fire += u32::from(resist.fire);
                    poison += u32::from(resist.poison);
                    cold += u32::from(resist.cold);
                }
                ItemKind::Weapon { class, damage, .. } => {
                    stats.damage = u32::from(*damage);
                    stats.weapon_class = Some(*class);
                }
                _ => {}
            }
        }
        // 上限 100 に収まる値なので u8 に収まる。
        let cap = |v: u32| v.min(100) as u8;
        stats.resist = Resist { fire: cap(fire), poison: cap(poison), cold: cap(cold) };
        stats
    }
}
