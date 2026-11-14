//! 仕様 23 §1: 死亡と復活。倒れたプレイヤーは、少し待ってから、出発点で体力が満タンで復活する。持ち物はそのまま。

use charon_core::world::World;

use crate::game::{GameEvent, GameState};
use crate::vitals::BREATH_MAX;
use crate::{EntityId, EntityWorld, collides};

/// 倒れてから復活するまでの tick (3 秒)。
pub const RESPAWN_TICKS: u16 = 180;
/// 出発点が埋まっているとき、上へ探す最大の高さ (セル)。
const SEARCH_UP: i32 = 96;

impl GameState {
    /// 倒れたプレイヤー (杖を持つ、敵でない) を数え、`RESPAWN_TICKS` たったら `spawn` (左上のセル) で復活させる。
    /// 毎 tick 呼ぶ。復活したら `GameEvent::Respawned` を足す。復活できる場所がなければ、次の tick にまた試す。
    pub fn step_respawn(&mut self, world: &World, entities: &mut EntityWorld, spawn: [i32; 2]) {
        let dead: Vec<EntityId> = self
            .wands
            .keys()
            .copied()
            .filter(|id| !self.enemies.contains_key(id))
            .filter(|id| self.vitals.get(*id).is_some_and(|v| !v.alive()))
            .collect();
        self.death_timer.retain(|id, _| dead.contains(id));
        for id in dead {
            let timer = self.death_timer.entry(id).or_insert(0);
            *timer = timer.saturating_add(1);
            if *timer < RESPAWN_TICKS {
                continue;
            }
            let Some(entity) = entities.get(id) else { continue };
            let (width, height) = (entity.width, entity.height);
            // 出発点、埋まっていれば少しずつ上へ。
            let place = (0..=SEARCH_UP).find(|&up| {
                !collides(
                    i64::from(spawn[0]) * 16,
                    i64::from(spawn[1] - up) * 16,
                    width,
                    height,
                    world,
                )
            });
            let Some(up) = place else { continue };
            if let Some(entity) = entities.get_mut(id) {
                entity.x16 = i64::from(spawn[0]) * 16;
                entity.y16 = i64::from(spawn[1] - up) * 16;
                entity.vx = 0;
                entity.vy = 0;
                entity.ext_vx = 0;
                entity.grounded = false;
            }
            if let Some(vitals) = self.vitals.get_mut(id) {
                vitals.hp = vitals.max_hp;
                vitals.breath = BREATH_MAX;
                vitals.clear_combat_immunity();
            }
            if let Some(wand) = self.wands.get_mut(&id) {
                wand.mana = wand.mana_max;
            }
            self.statuses.clear(id);
            self.death_timer.remove(&id);
            self.push_event(GameEvent::Respawned(id));
        }
    }

    /// 倒れてから数えている tick (描画用)。倒れていなければ `None`。
    pub fn death_ticks(&self, id: EntityId) -> Option<u16> {
        self.death_timer.get(&id).copied()
    }
}
