//! 浮遊 (ホバー、仕様 10 §18): ジャンプのボタンを**長押し**すると、燃料を使って空中にとどまる。
//! 短く押すだけなら、ふつうのジャンプ。接地すると燃料が戻る。整数だけ。

use crate::Entity;

/// 浮遊の性能。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevitationDef {
    /// 燃料の最大 (tick)。ホバーしている間、1 tick に 1 減る。
    pub fuel_max: u16,
    /// ボタンを押し始めてから、ホバーが始まるまでの tick。これより短い押し方は、ふつうのジャンプ。
    pub delay: u16,
    /// 1 tick に上向きへ足す速さ (1/16 セル / tick)。重力 (4) より大きいと上がる。
    pub thrust: i16,
    /// 上向きの速さの上限 (1/16 セル / tick、正の値)。ホバー中は、これより速く上がらない (上がっていたら、この速さまで減る)。
    pub rise_max: i16,
    /// 接地している間、1 tick に戻る燃料。
    pub recharge: u16,
}

impl Default for LevitationDef {
    /// 約 3 秒ぶんの燃料。0.2 秒 (12 tick) 押し続けるとホバーが始まり、ゆっくり (約 7 セル / 秒) 上がる。
    fn default() -> Self {
        Self { fuel_max: 180, delay: 12, thrust: 8, rise_max: 6, recharge: 3 }
    }
}

/// 浮遊の状態 (キャラクターごと)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levitation {
    pub def: LevitationDef,
    pub fuel: u16,
    /// ボタンを押し続けている tick (離すと 0)。
    held_ticks: u16,
}

/// 速さの値の上限 (`step_entity` の `vy` の範囲に合わせる)。
const SPEED_LIMIT: i16 = 96;

impl Levitation {
    /// 燃料は満タンで始まる。値は範囲に収める。
    pub fn new(def: LevitationDef) -> Self {
        let def = LevitationDef {
            fuel_max: def.fuel_max,
            delay: def.delay,
            thrust: def.thrust.clamp(0, SPEED_LIMIT),
            rise_max: def.rise_max.clamp(0, SPEED_LIMIT),
            recharge: def.recharge,
        };
        Self { def, fuel: def.fuel_max, held_ticks: 0 }
    }

    /// ボタンを押し続けている tick。
    pub fn held_ticks(&self) -> u16 {
        self.held_ticks
    }

    /// 1 tick 進める。`held` はジャンプのボタンを押しているか。移動 (`step_entity`) のあと、次の tick の前に呼ぶ。
    /// 押し続けた tick が `delay` を超え、空中で、燃料があれば、ホバーする: 上向きの速さが `rise_max` に届いていなければ、
    /// `thrust` を足して (`rise_max` まで) 燃料を 1 減らす。すでに `rise_max` より速く上がっているとき (ジャンプの最中) は
    /// 何もせず、燃料も減らさない。ジャンプが頂点に近づいて遅くなると、落ちずにとどまり、ゆっくり上がる。
    /// 短く押しただけなら何もしない (ふつうのジャンプ)。接地していれば燃料が戻る。推力を足したら真。
    pub fn step(&mut self, entity: &mut Entity, held: bool) -> bool {
        self.held_ticks = if held { self.held_ticks.saturating_add(1) } else { 0 };
        if entity.grounded {
            self.fuel = self.fuel.saturating_add(self.def.recharge).min(self.def.fuel_max);
            return false;
        }
        if self.held_ticks <= self.def.delay || self.fuel == 0 || entity.vy <= -self.def.rise_max {
            return false;
        }
        self.fuel -= 1;
        entity.vy = (entity.vy - self.def.thrust).max(-self.def.rise_max);
        true
    }
}
