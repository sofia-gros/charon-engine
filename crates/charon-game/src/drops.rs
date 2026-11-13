//! 落ちている材質のかたまり (ドロップ、仕様 10 §19)。落ちて、近づいたキャラクターが拾う。整数だけで決定的。

use std::collections::BTreeMap;

use charon_core::cell::MaterialId;
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

use crate::EntityId;

/// 同時に存在できるドロップの数。超える分は作らない。
pub const MAX_DROPS: usize = 256;
/// 拾える距離 (セル)。
pub const PICKUP_RADIUS: i64 = 8;
/// 作られてから拾えるまでの待ち (tick)。投げた直後に拾い直さないため。
pub const PICKUP_DELAY: u32 = 30;
/// 拾われないまま消えるまでの時間 (tick)。5 分。
pub const LIFETIME: u32 = 18_000;
/// 落ちる速さの上限 (1/16 セル / tick)。
const FALL_MAX: i16 = 48;

/// 落ちているもの。位置は 1/16 セル単位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drop {
    pub x16: i64,
    pub y16: i64,
    vy: i16,
    pub material: MaterialId,
    pub count: u32,
    age: u32,
}

/// 落ちているものの集まり。
#[derive(Clone, Debug, Default)]
pub struct Drops {
    items: BTreeMap<u32, Drop>,
    next_id: u32,
}

/// 拾われたもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pickup {
    pub who: EntityId,
    pub material: MaterialId,
    pub count: u32,
}

fn blocks(world: &World, x: i64, y: i64) -> bool {
    let (Ok(cx), Ok(cy)) = (i32::try_from(x), i32::try_from(y)) else { return true };
    let cell = world.get(CellPos::new(cx, cy));
    cell.material == MaterialId::BOUNDARY
        || matches!(world.materials().get(cell.material).phase, Phase::Solid | Phase::Powder)
}

impl Drops {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Drop> {
        self.items.values()
    }

    /// 材質 `material` を `count` 個、位置 (1/16 セル) に落とす。空・境界・0 個、または上限を超えるときは作らない。
    pub fn spawn(&mut self, material: MaterialId, count: u32, x16: i64, y16: i64) -> bool {
        if material == MaterialId::EMPTY
            || material == MaterialId::BOUNDARY
            || count == 0
            || self.items.len() >= MAX_DROPS
        {
            return false;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.items.insert(self.next_id, Drop { x16, y16, vy: 0, material, count, age: 0 });
        true
    }

    /// 1 tick 進める: 落ちる・年を取る・拾われる。`collectors` は拾える者 (ID と、中心の位置 1/16 セル)。
    /// `accept(who, material, count)` は、持ち物に入れて、入った個数を返す (満杯なら少ない)。
    /// 全部入ればドロップは消え、残れば残りが落ちたまま。拾われたものを返す。ID の昇順で、先に近づいた者が先。
    pub fn step(
        &mut self,
        world: &World,
        collectors: &[(EntityId, i64, i64)],
        mut accept: impl FnMut(EntityId, MaterialId, u32) -> u32,
    ) -> Vec<Pickup> {
        let mut picked = Vec::new();
        let reach = (PICKUP_RADIUS * 16) * (PICKUP_RADIUS * 16);
        self.items.retain(|_, drop| {
            drop.age += 1;
            if drop.age > LIFETIME {
                return false;
            }
            // 固体の中にいたら、1 セルずつ上へ出る (埋まって見えなくならないように)。
            if blocks(world, drop.x16.div_euclid(16), drop.y16.div_euclid(16)) {
                drop.y16 -= 16;
                drop.vy = 0;
            } else {
                drop.vy = (drop.vy + 4).min(FALL_MAX);
                for _ in 0..drop.vy {
                    if blocks(world, drop.x16.div_euclid(16), (drop.y16 + 1).div_euclid(16)) {
                        drop.vy = 0;
                        break;
                    }
                    drop.y16 += 1;
                }
            }
            if drop.age < PICKUP_DELAY {
                return true;
            }
            for &(who, cx, cy) in collectors {
                let (dx, dy) = (cx - drop.x16, cy - drop.y16);
                if dx * dx + dy * dy > reach {
                    continue;
                }
                let taken = accept(who, drop.material, drop.count).min(drop.count);
                if taken > 0 {
                    picked.push(Pickup { who, material: drop.material, count: taken });
                    drop.count -= taken;
                }
                if drop.count == 0 {
                    return false;
                }
            }
            true
        });
        picked
    }
}
