//! 仕様 20: 戦闘。武器 (近接・遠距離・杖)・防具による軽減・属性・ノックバック。整数だけ。
//!
//! 武器は `Equipment` の `Weapon` 枠のアイテムで決まる。近接 (`Melee`) は前の箱の中の相手を斬り、遠距離 (`Ranged`) は
//! 重力のある矢を撃ち、杖 (`Wand`)・何も持たないときは、キャラクターの杖で撃つ (今までと同じ)。

use charon_core::world::World;

use crate::game::{GameEvent, GameState};
use crate::items::WeaponClass;
use crate::projectile::{Payload, ProjectileId};
use crate::vitals::{Armor, DamageKind};
use crate::{Entity, EntityId, EntityWorld};

/// 近接の攻撃が届く距離 (セル)。2 ブロック半。
pub const MELEE_REACH: i64 = 20;
/// 近接の攻撃の縦の幅の半分 (セル)。
pub const MELEE_HALF_HEIGHT: i64 = 14;
/// 近接の攻撃の間隔 (tick)。
pub const MELEE_COOLDOWN: u16 = 20;
/// 遠距離の攻撃の間隔 (tick)。
pub const RANGED_COOLDOWN: u16 = 24;
/// 矢の速さ (1/16 セル / tick)・重力・寿命 (tick)。
const ARROW_SPEED: i64 = 160;
const ARROW_GRAVITY: i16 = 3;
const ARROW_LIFE: u16 = 90;
/// ノックバックの基本の速さ (1/16 セル / tick)。ダメージが大きいほど強い。
const KNOCKBACK_BASE: i64 = 24;
const KNOCKBACK_LIFT: i16 = 28;

fn center16(e: &Entity) -> (i64, i64) {
    (e.x16 + i64::from(e.width) * 8, e.y16 + i64::from(e.height) * 8)
}

fn unit(ax: i32, ay: i32, speed: i64) -> Option<(i16, i16)> {
    let (ax, ay) = (i64::from(ax), i64::from(ay));
    let len = (ax * ax + ay * ay).isqrt();
    if len == 0 {
        return None;
    }
    let to = |v: i64| i16::try_from(v * speed / len).ok();
    Some((to(ax)?, to(ay)?))
}

impl GameState {
    /// 装備から、防御・耐性を体力の表に写す (毎 tick の初めに呼ぶ)。装備を持たないキャラクターは変えない。
    pub(crate) fn refresh_armor(&mut self) {
        let ids: Vec<EntityId> = self.equipment.keys().copied().collect();
        for id in ids {
            let Some(equipment) = self.equipment.get(&id) else { continue };
            let stats = equipment.stats(|item| match item.class() {
                crate::items::ItemClass::Def(index) => self.item_defs.get(index),
                _ => None,
            });
            if let Some(vitals) = self.vitals.get_mut(id) {
                vitals.armor = Armor {
                    defense: i32::try_from(stats.defense).unwrap_or(i32::MAX),
                    fire: stats.resist.fire,
                    poison: stats.resist.poison,
                    cold: stats.resist.cold,
                };
            }
        }
    }

    /// 装備している武器の種類とダメージ。何も持っていなければ `None` (杖で撃つ)。
    pub fn equipped_weapon(&self, id: EntityId) -> Option<(WeaponClass, i32)> {
        use crate::items::{EquipSlot, ItemClass, ItemKind};
        let item = self.equipment.get(&id)?.get(EquipSlot::Weapon)?;
        let ItemClass::Def(index) = item.class() else { return None };
        match &self.item_defs.get(index)?.kind {
            ItemKind::Weapon { class, damage, .. } => Some((*class, i32::from(*damage))),
            _ => None,
        }
    }

    /// 武器の間隔を 1 tick 進める。
    pub(crate) fn tick_weapon_cooldowns(&mut self) {
        for left in self.weapon_cooldown.values_mut() {
            *left = left.saturating_sub(1);
        }
        self.weapon_cooldown.retain(|_, left| *left > 0);
    }

    /// 近接の攻撃: `aim` の向き (左右) の前の箱にいる相手を斬る。間隔の間は何もしない。当たった相手の数を返す。
    pub(crate) fn try_melee(
        &mut self,
        entities: &mut EntityWorld,
        id: EntityId,
        damage: i32,
        aim: (i32, i32),
    ) -> usize {
        if self.weapon_cooldown.contains_key(&id) {
            return 0;
        }
        let Some(me) = entities.get(id).cloned() else { return 0 };
        self.weapon_cooldown.insert(id, MELEE_COOLDOWN);
        let facing: i64 = if aim.0 < 0 { -1 } else { 1 };
        let (cx, cy) = center16(&me);
        // 攻撃の箱 (1/16 セル): 体の端から前へ `MELEE_REACH`。
        let (x0, x1) = if facing > 0 {
            (cx, cx + (i64::from(me.width) * 8 + MELEE_REACH * 16))
        } else {
            (cx - (i64::from(me.width) * 8 + MELEE_REACH * 16), cx)
        };
        let (y0, y1) = (cy - MELEE_HALF_HEIGHT * 16, cy + MELEE_HALF_HEIGHT * 16);
        let attacker_is_enemy = self.enemies.contains_key(&id);
        let targets: Vec<EntityId> = entities
            .iter()
            .filter(|e| e.id != id)
            .filter(|e| self.enemies.contains_key(&e.id) != attacker_is_enemy)
            .filter(|e| self.vitals.get(e.id).is_some_and(|v| v.alive()))
            .filter(|e| {
                e.x16 < x1
                    && e.x16 + i64::from(e.width) * 16 > x0
                    && e.y16 < y1
                    && e.y16 + i64::from(e.height) * 16 > y0
            })
            .map(|e| e.id)
            .collect();
        for &target in &targets {
            let applied = self
                .vitals
                .get_mut(target)
                .map_or(0, |v| v.damage_combat(damage, DamageKind::Physical));
            if applied > 0 {
                self.knockback(entities, target, facing, applied);
            }
            self.push_event(GameEvent::Melee { attacker: id, target, damage: applied });
        }
        targets.len()
    }

    /// 遠距離の攻撃: 重力のある矢を撃つ。間隔の間・向きが (0, 0) のときは `None`。
    pub(crate) fn try_shoot_ranged(
        &mut self,
        world: &World,
        entities: &EntityWorld,
        id: EntityId,
        damage: i32,
        aim: (i32, i32),
    ) -> Option<ProjectileId> {
        if self.weapon_cooldown.contains_key(&id) {
            return None;
        }
        let me = entities.get(id)?;
        let (vx, vy) = unit(aim.0, aim.1, ARROW_SPEED)?;
        let (x, y) = center16(me);
        let pid = self
            .projectiles
            .spawn(world, x, y, vx, vy, ARROW_GRAVITY, ARROW_LIFE, damage, Payload::None)
            .ok()?;
        self.weapon_cooldown.insert(id, RANGED_COOLDOWN);
        Some(pid)
    }

    /// 向き `dir` (-1 か 1) へ弾き飛ばす。ダメージが大きいほど強い。
    pub(crate) fn knockback(
        &mut self,
        entities: &mut EntityWorld,
        target: EntityId,
        dir: i64,
        damage: i32,
    ) {
        let Some(e) = entities.get_mut(target) else { return };
        let push = KNOCKBACK_BASE + i64::from(damage.clamp(0, 40));
        let v = (i64::from(e.ext_vx) + dir.signum() * push).clamp(-512, 512);
        e.ext_vx = i16::try_from(v).unwrap_or(0);
        e.vy = e.vy.min(-KNOCKBACK_LIFT);
        e.grounded = false;
    }

    pub(crate) fn push_event(&mut self, event: GameEvent) {
        if self.events.len() < crate::game::EVENTS_MAX {
            self.events.push(event);
        }
    }
}
