//! 仕様 10 §4: 飛び道具と爆発ダメージ。

use std::collections::BTreeMap;

use charon_core::cell::{Cell, FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;
use charon_physics::PhysicsEngine;

use crate::vitals::{VitalsTable, covered_cells};
use crate::{Entity, EntityId, EntityWorld};

/// 同時に存在できる弾の数。
pub const MAX_PROJECTILES: usize = 1024;
const MAX_SPEED: i16 = 512;
const MAX_LIFE: u16 = 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectileId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    None,
    /// 半径 1..=16 の爆発 (仕様 07 §5)。
    Explode {
        radius: u8,
    },
    /// 着弾の直前の空きセルに 1 セル置く。
    Deposit {
        material: MaterialId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Projectile {
    pub id: ProjectileId,
    pub x16: i64,
    pub y16: i64,
    pub vx: i16,
    pub vy: i16,
    pub gravity: i16,
    pub life: u16,
    pub damage: i32,
    pub payload: Payload,
    /// 発射した tick に限り、このエンティティには当たらない (仕様 10 §6.1)。
    pub grace: Option<EntityId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectileError {
    InvalidLife,
    InvalidSpeed,
    InvalidRadius,
    UnknownMaterial,
    TooMany,
    ExhaustedIds,
}

impl std::fmt::Display for ProjectileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLife => write!(f, "弾の寿命は 1..=3600 tick です"),
            Self::InvalidSpeed => write!(f, "弾の速度の各成分は絶対値 512 以下です"),
            Self::InvalidRadius => write!(f, "爆発半径は 1..=16 セルです"),
            Self::UnknownMaterial => write!(f, "弾が置く材質が定義されていません"),
            Self::TooMany => write!(f, "弾は同時に {MAX_PROJECTILES} 個までです"),
            Self::ExhaustedIds => write!(f, "弾 ID の上限に達しました"),
        }
    }
}

impl std::error::Error for ProjectileError {}

/// 1 つの弾が何かに当たった記録。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub projectile: ProjectileId,
    pub entity: Option<EntityId>,
    pub cell: CellPos,
}

#[derive(Clone, Debug)]
pub struct ProjectileWorld {
    projectiles: BTreeMap<ProjectileId, Projectile>,
    next_id: u64,
}

impl Default for ProjectileWorld {
    fn default() -> Self {
        Self::new()
    }
}

/// 爆風が中心から `dist2` (セル距離の 2 乗) 離れた対象に与えるダメージ。遮蔽は考えない。
pub fn explosion_damage(radius: u8, dist2: i64, max_damage: i32) -> i32 {
    let r2 = i64::from(radius) * i64::from(radius);
    if dist2 >= r2 || max_damage <= 0 {
        return 0;
    }
    // r2 <= 256、max_damage は i32 なので i64 で収まる。結果は max_damage 以下。
    i32::try_from(i64::from(max_damage) * (r2 - dist2) / r2).unwrap_or(0)
}

impl ProjectileWorld {
    pub fn new() -> Self {
        Self { projectiles: BTreeMap::new(), next_id: 1 }
    }

    pub fn len(&self) -> usize {
        self.projectiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.projectiles.is_empty()
    }

    pub fn get(&self, id: ProjectileId) -> Option<&Projectile> {
        self.projectiles.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Projectile> {
        self.projectiles.values()
    }

    /// 弾を追加する。エラーのときは何も変わらない。
    #[allow(clippy::too_many_arguments)] // 仕様 10 §4 の弾のフィールドをそのまま並べる。
    pub fn spawn(
        &mut self,
        world: &World,
        x16: i64,
        y16: i64,
        vx: i16,
        vy: i16,
        gravity: i16,
        life: u16,
        damage: i32,
        payload: Payload,
    ) -> Result<ProjectileId, ProjectileError> {
        if !(1..=MAX_LIFE).contains(&life) {
            return Err(ProjectileError::InvalidLife);
        }
        if vx.unsigned_abs() > MAX_SPEED.unsigned_abs()
            || vy.unsigned_abs() > MAX_SPEED.unsigned_abs()
        {
            return Err(ProjectileError::InvalidSpeed);
        }
        match payload {
            Payload::Explode { radius } if !(1..=16).contains(&radius) => {
                return Err(ProjectileError::InvalidRadius);
            }
            Payload::Deposit { material } if usize::from(material.0) >= world.materials().len() => {
                return Err(ProjectileError::UnknownMaterial);
            }
            _ => {}
        }
        if self.projectiles.len() >= MAX_PROJECTILES {
            return Err(ProjectileError::TooMany);
        }
        let next_id = self.next_id.checked_add(1).ok_or(ProjectileError::ExhaustedIds)?;
        let id = ProjectileId(self.next_id);
        self.projectiles.insert(
            id,
            Projectile { id, x16, y16, vx, vy, gravity, life, damage, payload, grace: None },
        );
        self.next_id = next_id;
        Ok(id)
    }

    /// 弾 `id` が次の 1 tick だけ `entity` に当たらないようにする。弾がなければ何もしない。
    pub fn set_grace(&mut self, id: ProjectileId, entity: EntityId) {
        if let Some(p) = self.projectiles.get_mut(&id) {
            p.grace = Some(entity);
        }
    }

    /// 仕様 10 §4: 弾を ID 昇順に 1 tick 進める。
    pub fn step(
        &mut self,
        world: &mut World,
        physics: &mut PhysicsEngine,
        entities: &EntityWorld,
        vitals: &mut VitalsTable,
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        let ids: Vec<ProjectileId> = self.projectiles.keys().copied().collect();
        for id in ids {
            let Some(mut p) = self.projectiles.get(&id).copied() else { continue };
            p.vy = (i32::from(p.vy) + i32::from(p.gravity))
                .clamp(-i32::from(MAX_SPEED), i32::from(MAX_SPEED)) as i16;
            let (start_x, start_y) = (p.x16, p.y16);
            let steps = i64::from(p.vx.unsigned_abs().max(p.vy.unsigned_abs()));
            let mut last_free = (start_x, start_y);
            let mut collision: Option<(CellPos, Option<EntityId>)> = None;
            for k in 1..=steps {
                // 0 方向へ丸める整数除算。
                let x = start_x + i64::from(p.vx) * k / steps;
                let y = start_y + i64::from(p.vy) * k / steps;
                let cell_pos = cell_at(x, y);
                if let Some(entity) = entity_at(entities, vitals, x, y, p.grace) {
                    collision = Some((cell_pos.unwrap_or(CellPos::new(0, 0)), Some(entity)));
                    break;
                }
                match cell_pos {
                    Some(pos) if !blocks(world, pos) => last_free = (x, y),
                    Some(pos) => {
                        collision = Some((pos, None));
                        break;
                    }
                    // 座標が i32 のセル範囲を外れたら壁として扱い、直前の位置で止める。
                    None => {
                        collision = Some((CellPos::new(0, 0), None));
                        break;
                    }
                }
            }

            let Some((cell, entity)) = collision else {
                p.x16 = start_x + i64::from(p.vx);
                p.y16 = start_y + i64::from(p.vy);
                p.life -= 1;
                // 撃った本人の体から出るまでは、当たらない (体が大きいと、1 tick では出られない)。
                if let Some(owner) = p.grace
                    && !inside_entity(entities, owner, p.x16, p.y16)
                {
                    p.grace = None;
                }
                if p.life == 0 {
                    self.projectiles.remove(&id);
                } else {
                    self.projectiles.insert(id, p);
                }
                continue;
            };

            self.projectiles.remove(&id);
            if let Some(target) = entity
                && let Some(v) = vitals.get_mut(target)
            {
                v.damage(p.damage);
            }
            match p.payload {
                Payload::None => {}
                Payload::Explode { radius } => {
                    // 半径は spawn で検査済み。起こりえない失敗は爆発なしとして扱う。
                    let center = if entity.is_some() {
                        cell_at(last_free.0, last_free.1).unwrap_or(cell)
                    } else {
                        cell
                    };
                    if charon_physics::explode(world, physics, center, radius).is_ok() {
                        apply_explosion(
                            entities,
                            vitals,
                            center,
                            radius,
                            p.damage.saturating_mul(2),
                        );
                    }
                }
                Payload::Deposit { material } => {
                    if let Some(pos) = cell_at(last_free.0, last_free.1)
                        && world.get(pos).material == MaterialId::EMPTY
                        && !world.get(pos).has(FLAG_BODY)
                    {
                        let cell = world.make_cell(material, pos);
                        world.set(pos, cell);
                    }
                }
            }
            hits.push(Hit { projectile: id, entity, cell });
        }
        hits
    }

    /// 仕様 10 §4: FNV-1a 64-bit。整数はリトルエンディアン。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        };
        for p in self.projectiles.values() {
            feed(&p.id.0.to_le_bytes());
            feed(&p.x16.to_le_bytes());
            feed(&p.y16.to_le_bytes());
            feed(&p.vx.to_le_bytes());
            feed(&p.vy.to_le_bytes());
            feed(&p.gravity.to_le_bytes());
            feed(&p.life.to_le_bytes());
            feed(&p.damage.to_le_bytes());
            feed(&p.grace.map_or(0, |id| id.0).to_le_bytes());
            match p.payload {
                Payload::None => feed(&[0]),
                Payload::Explode { radius } => feed(&[1, radius]),
                Payload::Deposit { material } => {
                    feed(&[2]);
                    feed(&material.0.to_le_bytes());
                }
            }
        }
        feed(&self.next_id.to_le_bytes());
        hash
    }
}

fn cell_at(x16: i64, y16: i64) -> Option<CellPos> {
    Some(CellPos::new(
        i32::try_from(x16.div_euclid(16)).ok()?,
        i32::try_from(y16.div_euclid(16)).ok()?,
    ))
}

/// 弾が通れないセルか: 境界・剛体・固体・粉体・液体。
fn blocks(world: &World, pos: CellPos) -> bool {
    let cell: Cell = world.get(pos);
    cell.material == MaterialId::BOUNDARY
        || cell.has(FLAG_BODY)
        || matches!(
            world.materials().get(cell.material).phase,
            Phase::Solid | Phase::Powder | Phase::Liquid
        )
}

/// 点 `(x16, y16)` を当たり判定に含む、体力を持つエンティティのうち ID が最小のもの。
/// 点 (x16, y16) が、エンティティ id の箱の中にあるか。
fn inside_entity(entities: &EntityWorld, id: EntityId, x16: i64, y16: i64) -> bool {
    entities.get(id).is_some_and(|e| {
        x16 >= e.x16
            && x16 < e.x16 + i64::from(e.width) * 16
            && y16 >= e.y16
            && y16 < e.y16 + i64::from(e.height) * 16
    })
}

fn entity_at(
    entities: &EntityWorld,
    vitals: &VitalsTable,
    x16: i64,
    y16: i64,
    grace: Option<EntityId>,
) -> Option<EntityId> {
    entities.iter().find_map(|e: &Entity| {
        if grace == Some(e.id) {
            return None;
        }
        let inside = x16 >= e.x16
            && x16 < e.x16 + i64::from(e.width) * 16
            && y16 >= e.y16
            && y16 < e.y16 + i64::from(e.height) * 16;
        (inside && vitals.get(e.id).is_some()).then_some(e.id)
    })
}

/// 爆風の円と当たり判定が交わる全エンティティ (ID 昇順) にダメージを与える。
fn apply_explosion(
    entities: &EntityWorld,
    vitals: &mut VitalsTable,
    center: CellPos,
    radius: u8,
    max_damage: i32,
) {
    for e in entities.iter() {
        let Some(v) = vitals.get_mut(e.id) else { continue };
        let (left, top, right, bottom) = covered_cells(e);
        let (cx, cy) = (i64::from(center.x), i64::from(center.y));
        let dx = if cx < left {
            left - cx
        } else if cx > right {
            cx - right
        } else {
            0
        };
        let dy = if cy < top {
            top - cy
        } else if cy > bottom {
            cy - bottom
        } else {
            0
        };
        v.damage(explosion_damage(radius, dx * dx + dy * dy, max_damage));
    }
}
