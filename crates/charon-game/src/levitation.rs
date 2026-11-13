//! 浮遊 (仕様 10 §18): ジャンプのボタンを押している間、燃料を使って上へ進む。接地すると燃料が戻る。整数だけ。

use crate::Entity;

/// 浮遊の性能。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevitationDef {
    /// 燃料の最大 (tick)。押している間、1 tick に 1 減る。
    pub fuel_max: u16,
    /// 1 tick に上向きへ足す速さ (1/16 セル / tick)。重力 (4) より大きいと上がる。
    pub thrust: i16,
    /// 上向きの速さの上限 (1/16 セル / tick、正の値)。
    pub rise_max: i16,
    /// 接地している間、1 tick に戻る燃料。
    pub recharge: u16,
}

impl Default for LevitationDef {
    /// 約 3 秒ぶんの燃料。
    fn default() -> Self {
        Self { fuel_max: 180, thrust: 8, rise_max: 40, recharge: 3 }
    }
}

/// 浮遊の状態 (キャラクターごと)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levitation {
    pub def: LevitationDef,
    pub fuel: u16,
}

/// 速さの値の上限 (`step_entity` の `vy` の範囲に合わせる)。
const SPEED_LIMIT: i16 = 96;

impl Levitation {
    /// 燃料は満タンで始まる。値は範囲に収める。
    pub fn new(def: LevitationDef) -> Self {
        let def = LevitationDef {
            fuel_max: def.fuel_max,
            thrust: def.thrust.clamp(0, SPEED_LIMIT),
            rise_max: def.rise_max.clamp(0, SPEED_LIMIT),
            recharge: def.recharge,
        };
        Self { def, fuel: def.fuel_max }
    }

    /// 1 tick 進める。`held` はジャンプのボタンを押しているか。移動 (`step_entity`) のあと、次の tick の前に呼ぶ。
    /// 空中で押していて燃料があれば、上向きの速さを足して燃料を減らす。接地していれば燃料が戻る。
    /// 上向きに進んだら真。
    pub fn step(&mut self, entity: &mut Entity, held: bool) -> bool {
        if entity.grounded {
            self.fuel = self.fuel.saturating_add(self.def.recharge).min(self.def.fuel_max);
            return false;
        }
        if !held || self.fuel == 0 {
            return false;
        }
        self.fuel -= 1;
        entity.vy = (entity.vy - self.def.thrust).max(-self.def.rise_max);
        true
    }
}
