//! 状態異常 (仕様 10 §17): 燃える・濡れる・毒。整数だけで、決定的に進む。

use std::collections::BTreeMap;

use charon_core::cell::{FLAG_BODY, FLAG_BURNING, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

use crate::vitals::{HEAT_THRESHOLD, Vitals, covered_cells};
use crate::{Entity, EntityId};

/// 状態異常の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusKind {
    /// 燃えている: `BURN_INTERVAL` tick ごとに 1 ダメージ。濡れると消える。
    Burning,
    /// 濡れている: ダメージはない。燃えるのを防ぎ、燃えていれば消す。
    Wet,
    /// 毒: `POISON_INTERVAL` tick ごとに 1 ダメージ。
    Poison,
}

/// 燃えているときのダメージの間隔 (tick)。
pub const BURN_INTERVAL: u16 = 15;
/// 毒のダメージの間隔 (tick)。
pub const POISON_INTERVAL: u16 = 30;
/// 火に触れたときに燃える長さ (tick)。火から離れても、これだけ燃え続ける。
pub const BURN_TICKS: u16 = 180;
/// 液体に浸かったときに濡れている長さ (tick)。
pub const WET_TICKS: u16 = 300;
/// 1 つの状態の長さの上限 (tick)。
pub const TICKS_MAX: u16 = 3600;

/// キャラクターごとの状態異常。
#[derive(Clone, Debug, Default)]
pub struct StatusTable {
    map: BTreeMap<EntityId, BTreeMap<StatusKind, u16>>,
}

impl StatusTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ticks` (上限 `TICKS_MAX`) の間、状態にする。すでになっていれば長い方に延ばす。
    /// 濡れているときは燃えない。濡れると燃えているのが消える。0 tick は何もしない。
    /// 状態になったら真。
    pub fn apply(&mut self, id: EntityId, kind: StatusKind, ticks: u16) -> bool {
        if ticks == 0 {
            return false;
        }
        let ticks = ticks.min(TICKS_MAX);
        let entry = self.map.entry(id).or_default();
        match kind {
            StatusKind::Burning if entry.contains_key(&StatusKind::Wet) => return false,
            StatusKind::Wet => {
                entry.remove(&StatusKind::Burning);
            }
            _ => {}
        }
        let slot = entry.entry(kind).or_insert(0);
        *slot = (*slot).max(ticks);
        true
    }

    /// 残りの tick (なっていなければ 0)。
    pub fn remaining(&self, id: EntityId, kind: StatusKind) -> u16 {
        self.map.get(&id).and_then(|m| m.get(&kind)).copied().unwrap_or(0)
    }

    pub fn has(&self, id: EntityId, kind: StatusKind) -> bool {
        self.remaining(id, kind) > 0
    }

    /// 状態をすべて消す。
    pub fn clear(&mut self, id: EntityId) {
        self.map.remove(&id);
    }

    /// 1 tick 進める: 環境 (火・液体) から状態になり、ダメージを与え、残りを減らす。
    /// 材質の名前は使わず、`FLAG_BURNING`・温度・相 (`Phase::Liquid`) で判断する。
    pub fn step(&mut self, id: EntityId, entity: &Entity, vitals: &mut Vitals, world: &World) {
        if !vitals.alive() {
            self.clear(id);
            return;
        }
        let (left, top, right, bottom) = covered_cells(entity);
        let (mut fire, mut liquid) = (false, false);
        for y in top..=bottom {
            for x in left..=right {
                let (Ok(cx), Ok(cy)) = (i32::try_from(x), i32::try_from(y)) else { continue };
                let cell = world.get(CellPos::new(cx, cy));
                if cell.material == MaterialId::BOUNDARY || cell.has(FLAG_BODY) {
                    continue;
                }
                fire |= cell.has(FLAG_BURNING) || cell.temp >= HEAT_THRESHOLD;
                liquid |= world.materials().get(cell.material).phase == Phase::Liquid;
            }
        }
        // 液体は火より先 (水に入れば火は消える)。
        if liquid {
            self.apply(id, StatusKind::Wet, WET_TICKS);
        } else if fire {
            self.apply(id, StatusKind::Burning, BURN_TICKS);
        }
        let Some(entry) = self.map.get_mut(&id) else { return };
        let mut damage = 0;
        for (kind, left) in entry.iter_mut() {
            // 残りの tick が間隔の倍数のときに当たる (長さによらず、決まった拍で)。
            let interval = match kind {
                StatusKind::Burning => BURN_INTERVAL,
                StatusKind::Poison => POISON_INTERVAL,
                StatusKind::Wet => 0,
            };
            if interval != 0 && *left % interval == 0 {
                damage += 1;
            }
            *left -= 1;
        }
        entry.retain(|_, left| *left > 0);
        if entry.is_empty() {
            self.map.remove(&id);
        }
        vitals.damage(damage);
    }
}
