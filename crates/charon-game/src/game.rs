//! 仕様 10 §6.1: 体力・杖・弾を 1 つの tick の流れにまとめる。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::world::World;
use charon_physics::PhysicsEngine;

use crate::projectile::{Hit, ProjectileWorld};
use crate::vitals::{Vitals, VitalsTable, step_environment};
use crate::wand::{CastError, CastOutcome, SpellTable, Wand};
use crate::{EntityId, EntityWorld};

/// 1 tick のゲーム入力。`aim` は向きのベクトル。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameInput {
    pub fire: bool,
    pub aim: (i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameError {
    /// 存在しない ID、または同じ ID の重複。
    InvalidAction,
    Cast(CastError),
}

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAction => write!(f, "ゲーム入力の ID が存在しないか重複しています"),
            Self::Cast(e) => write!(f, "詠唱に失敗: {e}"),
        }
    }
}

impl std::error::Error for GameError {}

#[derive(Clone, Debug)]
pub struct GameState {
    pub vitals: VitalsTable,
    pub projectiles: ProjectileWorld,
    pub wands: BTreeMap<EntityId, Wand>,
    pub spells: SpellTable,
}

impl GameState {
    pub fn new(spells: SpellTable) -> Self {
        Self {
            vitals: VitalsTable::new(),
            projectiles: ProjectileWorld::new(),
            wands: BTreeMap::new(),
            spells,
        }
    }

    /// 体力と杖を持つキャラクターとして登録する。
    pub fn add_character(&mut self, id: EntityId, vitals: Vitals, wand: Wand) {
        self.vitals.insert(id, vitals);
        self.wands.insert(id, wand);
    }

    /// 仕様 10 §6.1 の順序で 1 tick 進める。
    /// 入力の検査に失敗したときは何も更新しない。
    pub fn step(
        &mut self,
        world: &mut World,
        physics: &mut PhysicsEngine,
        entities: &EntityWorld,
        actions: &[(EntityId, GameInput)],
        tick: u64,
        seed: u64,
    ) -> Result<Vec<Hit>, GameError> {
        let mut by_id = BTreeMap::new();
        for &(id, input) in actions {
            if !self.wands.contains_key(&id) || by_id.insert(id, input).is_some() {
                return Err(GameError::InvalidAction);
            }
        }

        // (1) 環境ダメージ。
        let ids: Vec<EntityId> = self.vitals.iter().map(|(id, _)| id).collect();
        for &id in &ids {
            if let (Some(entity), Some(vitals)) = (entities.get(id), self.vitals.get_mut(id)) {
                step_environment(entity, vitals, world);
            }
        }
        // (2) 杖の回復と待ち時間。
        for wand in self.wands.values_mut() {
            wand.tick();
        }
        // (3) 発射。
        let mut shooters = BTreeSet::new();
        for (&id, input) in &by_id {
            let alive = self.vitals.get(id).is_some_and(Vitals::alive);
            let (Some(entity), Some(wand)) = (entities.get(id), self.wands.get_mut(&id)) else {
                continue;
            };
            if !input.fire || !alive {
                continue;
            }
            let muzzle = (
                entity.x16 + i64::from(entity.width) * 8,
                entity.y16 + i64::from(entity.height) * 8,
            );
            match wand.cast(
                &self.spells,
                &mut self.projectiles,
                world,
                muzzle,
                input.aim,
                tick,
                seed,
            ) {
                Ok(CastOutcome::Fired(pid)) => {
                    shooters.insert((pid, id));
                }
                Ok(CastOutcome::Cooling | CastOutcome::NoMana) | Err(CastError::InvalidAim) => {}
                Err(e) => return Err(GameError::Cast(e)),
            }
        }
        for (pid, id) in shooters {
            self.projectiles.set_grace(pid, id);
        }
        // (4) 弾の移動と着弾。
        Ok(self.projectiles.step(world, physics, entities, &mut self.vitals))
    }

    /// 仕様 10 §6.1: 体力・弾・杖 (ID 昇順) を FNV-1a 64-bit で連結する。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |value: u64| {
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        };
        feed(self.vitals.state_hash());
        feed(self.projectiles.state_hash());
        for (&id, wand) in &self.wands {
            feed(id.0);
            feed(wand.state_hash());
        }
        hash
    }
}
