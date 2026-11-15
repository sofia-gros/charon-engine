//! 仕様 28 §8: ロックオン。狙う敵を選び、追い、解除する。整数だけ。

use charon_core::coords::CellPos;
use charon_core::query::Stop;
use charon_core::world::World;

use crate::cursor::{REACH_CELLS, center_cell};
use crate::game::GameState;
use crate::{EntityId, EntityWorld};

/// ロックできる距離 (セル)。届く距離の 2 倍。
pub const LOCK_RANGE: i32 = 2 * REACH_CELLS;
/// 見通しが切れてから、解除するまでの tick (`ClearestLine` のとき)。
pub const LOCK_LOST_TICKS: u32 = 60;

/// 候補の優先 (設定 `Lock On Priority`)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LockPriority {
    /// プレイヤーから見通せる敵のうち、カーソルに最も近いもの。
    #[default]
    ClearestLine,
    /// 壁を無視して、プレイヤーに最も近いもの。
    TargetClosest,
}

/// 範囲内の生きている敵とその中心のセル。`EntityId` の昇順。
fn enemies_in_range(
    entities: &EntityWorld,
    game: &GameState,
    center: CellPos,
) -> Vec<(EntityId, CellPos)> {
    let range = i64::from(LOCK_RANGE);
    game.enemies
        .keys()
        .filter_map(|&id| {
            let entity = entities.get(id)?;
            if !game.vitals.get(id).is_some_and(|v| v.alive()) {
                return None;
            }
            let cell = center_cell(entity);
            let (dx, dy) = (i64::from(cell.x - center.x), i64::from(cell.y - center.y));
            (dx * dx + dy * dy <= range * range).then_some((id, cell))
        })
        .collect()
}

fn distance_squared(a: CellPos, b: CellPos) -> i64 {
    let (dx, dy) = (i64::from(a.x) - i64::from(b.x), i64::from(a.y) - i64::from(b.y));
    dx * dx + dy * dy
}

/// 優先に従って並べた候補 (先頭がいちばん優先)。`ClearestLine` は見通せない敵を含めない。
/// `cursor` は `ClearestLine` の「カーソルに最も近い」の基準 (なければプレイヤー)。同じ近さなら `EntityId` の小さいほう。
pub fn ranked_targets(
    world: &World,
    entities: &EntityWorld,
    game: &GameState,
    center: CellPos,
    cursor: Option<CellPos>,
    priority: LockPriority,
) -> Vec<EntityId> {
    let mut found: Vec<(i64, EntityId)> = enemies_in_range(entities, game, center)
        .into_iter()
        .filter_map(|(id, cell)| match priority {
            LockPriority::TargetClosest => Some((distance_squared(cell, center), id)),
            LockPriority::ClearestLine => world
                .line_of_sight(center, cell, Stop::Solid)
                .then(|| (distance_squared(cell, cursor.unwrap_or(center)), id)),
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, id)| id).collect()
}

/// いまのロックの状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockOn {
    pub target: Option<EntityId>,
    /// 見通しが切れている tick の数。
    lost: u32,
}

impl LockOn {
    /// ロックのボタン: ロックしていなければ最優先の敵をロックし、ロック中なら次の候補へ (ほかにいなければ解除)。
    pub fn press(
        &mut self,
        world: &World,
        entities: &EntityWorld,
        game: &GameState,
        center: CellPos,
        cursor: Option<CellPos>,
        priority: LockPriority,
    ) {
        let ranked = ranked_targets(world, entities, game, center, cursor, priority);
        self.lost = 0;
        self.target = match self.target {
            None => ranked.first().copied(),
            Some(current) => {
                let others: Vec<EntityId> =
                    ranked.iter().copied().filter(|&id| id != current).collect();
                // 今の対象より優先の低いもののうち最初、なければ最優先 (一巡)。
                let position = ranked.iter().position(|&id| id == current);
                position
                    .and_then(|at| ranked[at + 1..].iter().copied().find(|&id| id != current))
                    .or_else(|| others.first().copied())
            }
        };
    }

    /// 毎 tick の更新: 対象が死ぬ・範囲を出る・(`ClearestLine` で) 見通しが 60 tick 切れると解除する。
    pub fn update(
        &mut self,
        world: &World,
        entities: &EntityWorld,
        game: &GameState,
        center: CellPos,
        priority: LockPriority,
    ) {
        let Some(target) = self.target else { return };
        let alive = entities.get(target).filter(|_| {
            game.enemies.contains_key(&target) && game.vitals.get(target).is_some_and(|v| v.alive())
        });
        let Some(entity) = alive else {
            *self = Self::default();
            return;
        };
        let cell = center_cell(entity);
        let range = i64::from(LOCK_RANGE);
        if distance_squared(cell, center) > range * range {
            *self = Self::default();
            return;
        }
        if priority == LockPriority::ClearestLine && !world.line_of_sight(center, cell, Stop::Solid)
        {
            self.lost += 1;
            if self.lost >= LOCK_LOST_TICKS {
                *self = Self::default();
            }
        } else {
            self.lost = 0;
        }
    }

    /// 解除する。
    pub fn release(&mut self) {
        *self = Self::default();
    }

    /// ロックしている敵の中心のセル。
    pub fn target_cell(&self, entities: &EntityWorld) -> Option<CellPos> {
        self.target.and_then(|id| entities.get(id)).map(center_cell)
    }
}
