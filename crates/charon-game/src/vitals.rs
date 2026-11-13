//! 仕様 10 §3: 体力・息と、熱・溺れによる環境ダメージ。

use std::collections::BTreeMap;

use charon_core::cell::{FLAG_BODY, FLAG_BURNING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

use crate::{Entity, EntityId};

pub const MAX_HP: i32 = 100;
/// 息の最大値 (tick)。60 tick / 秒で 10 秒。
pub const BREATH_MAX: u16 = 600;
/// この温度以上のセルに触れるとダメージを受ける。
pub const HEAT_THRESHOLD: i16 = 150;
/// 息が尽きたあと、この tick ごとに 1 ダメージ。
pub const DROWN_INTERVAL: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VitalsError {
    InvalidMaxHp,
}

impl std::fmt::Display for VitalsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "最大体力は 1..=10000 です")
    }
}

impl std::error::Error for VitalsError {}

/// ダメージの種類 (仕様 20)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageKind {
    /// 防御で減る。
    Physical,
    /// 火・毒・冷気: 耐性 (%) で減る。
    Fire,
    Poison,
    Cold,
}

/// 防具による軽減 (装備から写す、仕様 20 §2)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Armor {
    /// 物理ダメージを減らす量。
    pub defense: i32,
    /// 属性の耐性 (%、0..=100)。
    pub fire: u8,
    pub poison: u8,
    pub cold: u8,
}

/// エンティティ 1 つぶんの体力と息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vitals {
    pub hp: i32,
    pub max_hp: i32,
    pub breath: u16,
    drown_clock: u8,
    /// 防具による軽減。`GameState` が装備から毎 tick 写す。
    pub armor: Armor,
}

/// `amount` を耐性 `resist` (%) で減らす。四捨五入。
fn scale(amount: i32, resist: u8) -> i32 {
    let keep = 100 - i64::from(resist.min(100));
    i32::try_from((i64::from(amount) * keep + 50) / 100).unwrap_or(i32::MAX)
}

impl Vitals {
    pub fn new(max_hp: i32) -> Result<Self, VitalsError> {
        if !(1..=10_000).contains(&max_hp) {
            return Err(VitalsError::InvalidMaxHp);
        }
        Ok(Self { hp: max_hp, max_hp, breath: BREATH_MAX, drown_clock: 0, armor: Armor::default() })
    }

    pub fn alive(&self) -> bool {
        self.hp > 0
    }

    /// `amount` が 0 以下なら何もしない。`hp` は 0 で止まる。
    pub fn damage(&mut self, amount: i32) {
        if amount > 0 {
            self.hp = self.hp.saturating_sub(amount).max(0);
        }
    }

    /// 防具で軽減したダメージを与える (仕様 20 §2)。実際に与えた量を返す。
    /// 物理: `amount − defense`。ただし、元の 1/4 (1 以上) までしか減らない。属性: `amount × (100 − 耐性) / 100` (四捨五入)。
    pub fn damage_kind(&mut self, amount: i32, kind: DamageKind) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let applied = match kind {
            DamageKind::Physical => {
                let floor = (amount / 4).max(1);
                amount.saturating_sub(self.armor.defense.max(0)).max(floor)
            }
            DamageKind::Fire => scale(amount, self.armor.fire),
            DamageKind::Poison => scale(amount, self.armor.poison),
            DamageKind::Cold => scale(amount, self.armor.cold),
        };
        let before = self.hp;
        self.damage(applied);
        before - self.hp
    }

    /// `amount` が 0 以下なら何もしない。`hp` は `max_hp` で止まる。
    pub fn heal(&mut self, amount: i32) {
        if amount > 0 {
            self.hp = self.hp.saturating_add(amount).min(self.max_hp);
        }
    }
}

/// `EntityId` の昇順で保持する。
#[derive(Clone, Debug, Default)]
pub struct VitalsTable {
    entries: BTreeMap<EntityId, Vitals>,
}

impl VitalsTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: EntityId, vitals: Vitals) -> Option<Vitals> {
        self.entries.insert(id, vitals)
    }

    pub fn get(&self, id: EntityId) -> Option<&Vitals> {
        self.entries.get(&id)
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Vitals> {
        self.entries.get_mut(&id)
    }

    pub fn remove(&mut self, id: EntityId) -> Option<Vitals> {
        self.entries.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &Vitals)> {
        self.entries.iter().map(|(&id, vitals)| (id, vitals))
    }

    /// 仕様 10 §3: FNV-1a 64-bit。整数はリトルエンディアン。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        for (id, v) in self.iter() {
            for byte in
                id.0.to_le_bytes()
                    .into_iter()
                    .chain(v.hp.to_le_bytes())
                    .chain(v.max_hp.to_le_bytes())
                    .chain(v.breath.to_le_bytes())
                    .chain([v.drown_clock])
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        }
        hash
    }
}

/// 当たり判定が覆う整数セルの範囲 `(left, top, right, bottom)` (両端を含む)。
pub(crate) fn covered_cells(entity: &Entity) -> (i64, i64, i64, i64) {
    (
        entity.x16.div_euclid(16),
        entity.y16.div_euclid(16),
        (entity.x16 + i64::from(entity.width) * 16 - 1).div_euclid(16),
        (entity.y16 + i64::from(entity.height) * 16 - 1).div_euclid(16),
    )
}

/// 1 セルから受ける熱ダメージ。熱くなければ 0。
fn heat_damage(temp: i16, burning: bool) -> i32 {
    if !burning && temp < HEAT_THRESHOLD {
        return 0;
    }
    let base = ((i32::from(temp) - 100) / 50).clamp(1, 20);
    if burning { base.max(2) } else { base }
}

/// 1 tick に 1 回呼ぶ。セルもエンティティも書き換えない。
pub fn step_environment(entity: &Entity, vitals: &mut Vitals, world: &World) {
    if !vitals.alive() {
        return;
    }
    let (left, top, right, bottom) = covered_cells(entity);
    let mut hot = 0;
    let mut liquid_in_head = 0u32;
    for y in top..=bottom {
        for x in left..=right {
            let (Ok(cx), Ok(cy)) = (i32::try_from(x), i32::try_from(y)) else {
                continue;
            };
            let cell = world.get(CellPos::new(cx, cy));
            if cell.material == MaterialId::BOUNDARY || cell.has(FLAG_BODY) {
                continue;
            }
            hot = hot.max(heat_damage(cell.temp, cell.has(FLAG_BURNING)));
            if y == top && world.materials().get(cell.material).phase == Phase::Liquid {
                liquid_in_head += 1;
            }
        }
    }

    let mut drown = 0;
    if 2 * liquid_in_head >= u32::from(entity.width) {
        vitals.breath = vitals.breath.saturating_sub(1);
        if vitals.breath == 0 {
            vitals.drown_clock += 1;
            if vitals.drown_clock >= DROWN_INTERVAL {
                vitals.drown_clock = 0;
                drown = 1;
            }
        }
    } else {
        vitals.breath = vitals.breath.saturating_add(4).min(BREATH_MAX);
        vitals.drown_clock = 0;
    }
    vitals.damage_kind(hot, DamageKind::Fire);
    vitals.damage(drown);
}
