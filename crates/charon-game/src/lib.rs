//! 仕様 07 §2.1: セルと独立した、決定的なエンティティの格納。

pub mod lockstep;
pub mod mods;
pub mod projectile;
pub mod vitals;

use std::collections::BTreeMap;

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::world::World;

/// 生成ごとに単調増加する識別子。0 は無効。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u64);

/// 左上位置と速度を 1/16 セル単位の整数で保持する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub id: EntityId,
    pub x16: i64,
    pub y16: i64,
    pub vx: i16,
    pub vy: i16,
    pub width: u8,
    pub height: u8,
    pub grounded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityError {
    InvalidDimensions,
    OutOfBounds,
    ExhaustedIds,
    Occupied,
    InvalidInput,
}

impl std::fmt::Display for EntityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDimensions => write!(f, "エンティティの幅・高さは 1..=32 セルです"),
            Self::OutOfBounds => write!(f, "エンティティの座標がワールド範囲外です"),
            Self::ExhaustedIds => write!(f, "エンティティ ID の上限に達しました"),
            Self::Occupied => write!(f, "プレイヤーの初期位置が固体と重なっています"),
            Self::InvalidInput => write!(f, "プレイヤーの入力が無効です"),
        }
    }
}

impl std::error::Error for EntityError {}

/// ID 順で格納・反復し、削除した ID は再利用しない。
#[derive(Clone, Debug)]
pub struct EntityWorld {
    entities: BTreeMap<EntityId, Entity>,
    next_id: u64,
}

impl Default for EntityWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityWorld {
    pub fn new() -> Self {
        Self { entities: BTreeMap::new(), next_id: 1 }
    }

    pub fn spawn(
        &mut self,
        x16: i64,
        y16: i64,
        width: u8,
        height: u8,
    ) -> Result<EntityId, EntityError> {
        if !(1..=32).contains(&width) || !(1..=32).contains(&height) {
            return Err(EntityError::InvalidDimensions);
        }
        if i32::try_from(x16.div_euclid(16)).is_err() || i32::try_from(y16.div_euclid(16)).is_err()
        {
            return Err(EntityError::OutOfBounds);
        }
        let next_id = self.next_id.checked_add(1).ok_or(EntityError::ExhaustedIds)?;
        let id = EntityId(self.next_id);
        self.entities
            .insert(id, Entity { id, x16, y16, vx: 0, vy: 0, width, height, grounded: false });
        self.next_id = next_id;
        Ok(id)
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    pub fn remove(&mut self, id: EntityId) -> Option<Entity> {
        self.entities.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// 仕様 07 §2.1: フィールドをリトルエンディアンで FNV-1a 64-bit に流す。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        for entity in self.iter() {
            for byte in entity
                .id
                .0
                .to_le_bytes()
                .into_iter()
                .chain(entity.x16.to_le_bytes())
                .chain(entity.y16.to_le_bytes())
                .chain(entity.vx.to_le_bytes())
                .chain(entity.vy.to_le_bytes())
                .chain([entity.width, entity.height, u8::from(entity.grounded)])
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        }
        for byte in self.next_id.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
        }
        hash
    }

    /// 8×16 セルのプレイヤーを、固体のない位置に作る。
    pub fn spawn_player(
        &mut self,
        x16: i64,
        y16: i64,
        world: &World,
    ) -> Result<EntityId, EntityError> {
        let id = self.spawn(x16, y16, 8, 16)?;
        if collides(x16, y16, 8, 16, world) {
            self.remove(id);
            // 失敗した生成で ID を消費しない。前の状態へ戻す。
            self.next_id -= 1;
            return Err(EntityError::Occupied);
        }
        Ok(id)
    }

    /// 全入力を検証してから ID 順に更新する。入力のない ID は中立入力。
    pub fn step_players(
        &mut self,
        inputs: &[(EntityId, PlayerInput)],
        world: &World,
    ) -> Result<(), EntityError> {
        let mut by_id = BTreeMap::new();
        for &(id, input) in inputs {
            if !self.entities.contains_key(&id)
                || !(-1..=1).contains(&input.horizontal)
                || by_id.insert(id, input).is_some()
            {
                return Err(EntityError::InvalidInput);
            }
        }
        for (&id, entity) in &mut self.entities {
            step_player(entity, by_id.get(&id).copied().unwrap_or_default(), world);
        }
        Ok(())
    }
}

/// 1 tick の整数入力。horizontal は -1, 0, 1 のみ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayerInput {
    pub horizontal: i8,
    pub jump: bool,
}

/// x → y の順に 1/16 セルずつ移動し、衝突する直前で止める。
pub fn step_player(entity: &mut Entity, input: PlayerInput, world: &World) {
    debug_assert!((-1..=1).contains(&input.horizontal));
    entity.vx = i16::from(input.horizontal.clamp(-1, 1)) * 32;
    if entity.grounded && input.jump {
        entity.vy = -96;
        entity.grounded = false;
    }
    entity.vy = (entity.vy + 4).min(96);

    let step_x = entity.vx.signum() as i64;
    for _ in 0..entity.vx.unsigned_abs() {
        let next_x = entity.x16 + step_x;
        if collides(next_x, entity.y16, entity.width, entity.height, world) {
            entity.vx = 0;
            break;
        }
        entity.x16 = next_x;
    }
    let step_y = entity.vy.signum() as i64;
    entity.grounded = false;
    for _ in 0..entity.vy.unsigned_abs() {
        let next_y = entity.y16 + step_y;
        if collides(entity.x16, next_y, entity.width, entity.height, world) {
            entity.vy = 0;
            entity.grounded = step_y > 0;
            break;
        }
        entity.y16 = next_y;
    }
    if entity.vy == 0 && !entity.grounded {
        entity.grounded = collides(entity.x16, entity.y16 + 1, entity.width, entity.height, world);
    }
}

/// 半開区間の端を含むセルだけを検査し、未読み込み/範囲外も壁とする。
fn collides(x16: i64, y16: i64, width: u8, height: u8, world: &World) -> bool {
    let left = x16.div_euclid(16);
    let top = y16.div_euclid(16);
    let right = (x16 + i64::from(width) * 16 - 1).div_euclid(16);
    let bottom = (y16 + i64::from(height) * 16 - 1).div_euclid(16);
    for y in top..=bottom {
        for x in left..=right {
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                return true;
            };
            let cell = world.get(CellPos::new(x, y));
            if cell.material == MaterialId::BOUNDARY
                || cell.has(FLAG_BODY)
                || world.materials().get(cell.material).phase == Phase::Solid
            {
                return true;
            }
        }
    }
    false
}
