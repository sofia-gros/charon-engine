//! 仕様 07 §2.1: セルと独立した、決定的なエンティティの格納。

pub mod collapse;
pub mod defs;
pub mod dig;
pub mod drops;
pub mod game;
pub mod inventory;
pub mod levitation;
pub mod lockstep;
pub mod mods;
pub mod net;
pub mod projectile;
pub mod save;
pub mod status;
pub mod vitals;
pub mod wand;

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
    /// 外力による横向きの速度 (1/16 セル / tick)。ワイヤーの引っ張りなど (仕様 10 §9.2)。
    pub ext_vx: i16,
    /// 足場を離れてから、まだジャンプできる残りの tick (コヨーテタイム、仕様 10 §8.4)。接地中は `COYOTE_TICKS`。
    pub coyote: u8,
    /// ジャンプを押してから、着地したら跳ぶ残りの tick (ジャンプの先行入力、仕様 10 §8.4)。
    pub jump_buffer: u8,
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
        self.entities.insert(
            id,
            Entity {
                id,
                x16,
                y16,
                vx: 0,
                vy: 0,
                width,
                height,
                grounded: false,
                ext_vx: 0,
                coyote: 0,
                jump_buffer: 0,
            },
        );
        self.next_id = next_id;
        Ok(id)
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    /// 位置と速度を更新するための可変参照。ID・寸法を変えてはならない。
    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.get_mut(&id)
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
                .chain(entity.ext_vx.to_le_bytes())
                .chain([entity.coyote, entity.jump_buffer])
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        }
        for byte in self.next_id.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
        }
        hash
    }

    /// 固体のない位置にだけ作る (仕様 10 §8.2)。固体と重なるなら `Occupied` で、ID も消費しない。
    pub fn spawn_checked(
        &mut self,
        x16: i64,
        y16: i64,
        width: u8,
        height: u8,
        world: &World,
    ) -> Result<EntityId, EntityError> {
        let id = self.spawn(x16, y16, width, height)?;
        if collides(x16, y16, width, height, world) {
            self.remove(id);
            // 失敗した生成で ID を消費しない。前の状態へ戻す。
            self.next_id -= 1;
            return Err(EntityError::Occupied);
        }
        Ok(id)
    }

    /// `PLAYER_WIDTH`×`PLAYER_HEIGHT` セルのプレイヤーを、固体のない位置に作る。
    pub fn spawn_player(
        &mut self,
        x16: i64,
        y16: i64,
        world: &World,
    ) -> Result<EntityId, EntityError> {
        self.spawn_checked(x16, y16, PLAYER_WIDTH, PLAYER_HEIGHT, world)
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

/// プレイヤーの大きさ (セル)。当たり判定は、この箱に内接する楕円。
pub const PLAYER_WIDTH: u8 = 5;
pub const PLAYER_HEIGHT: u8 = 10;

/// プレイヤーの速さ: 歩く速さ (1/16 セル / tick) とジャンプの初速。
pub const PLAYER_WALK_SPEED: i16 = 32;
pub const PLAYER_JUMP_SPEED: i16 = 96;

/// x → y の順に 1/16 セルずつ移動し、衝突する直前で止める。
pub fn step_player(entity: &mut Entity, input: PlayerInput, world: &World) {
    step_entity(entity, input, PLAYER_WALK_SPEED, PLAYER_JUMP_SPEED, world);
}

/// 接地中の自動の段差乗り越えで、持ち上げてよい高さ (セル)。プレイヤー (高さ 10) の 4 割。
pub const STEP_UP_CELLS: i64 = 4;
/// 足場を離れてからも、ジャンプできる tick (コヨーテタイム、0.1 秒)。
pub const COYOTE_TICKS: u8 = 6;
/// ジャンプを押してから、着地したら跳ぶ tick (ジャンプの先行入力、約 0.08 秒)。
pub const JUMP_BUFFER_TICKS: u8 = 5;

/// `x16` へ進んだとき、楕円にぶつかる固体の**壁の高さ** (セル): ぶつかる各セルから真上へ続く固体の列の天辺から、足の行まで。
/// ぶつからなければ 0。楕円の下は丸いので、1/16 セルずつ進むたびに少しだけ持ち上げることを繰り返すと、高い壁も登れてしまう。
/// それを防ぐため、乗り越えてよいかは、持ち上げる量ではなく、壁そのものの高さで決める。
fn obstacle_height(entity: &Entity, x16: i64, world: &World) -> i64 {
    let (width, height, y16) = (entity.width, entity.height, entity.y16);
    let left = x16.div_euclid(16);
    let top = y16.div_euclid(16);
    let right = (x16 + i64::from(width) * 16 - 1).div_euclid(16);
    let feet = (y16 + i64::from(height) * 16 - 1).div_euclid(16);
    let solid = |x: i64, y: i64| -> bool {
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else { return true };
        let cell = world.get(CellPos::new(x, y));
        cell.material == MaterialId::BOUNDARY
            || cell.has(FLAG_BODY)
            || world.materials().get(cell.material).phase == Phase::Solid
    };
    let mut wall_top = feet + 1;
    for y in top..=feet {
        for x in left..=right {
            if !in_ellipse(x16, y16, width, height, x, y) || !solid(x, y) {
                continue;
            }
            let mut t = y;
            // 乗り越えられる高さを超えたと分かればよいので、少し上までだけ調べる。
            for _ in 0..=STEP_UP_CELLS + 1 {
                if solid(x, t - 1) {
                    t -= 1;
                } else {
                    break;
                }
            }
            wall_top = wall_top.min(t);
        }
    }
    feet - wall_top + 1
}

/// `next_x` へ進むとき、固体にぶつからないために持ち上げる最小の高さ (1/16 セル)。`STEP_UP_CELLS` より高い段、
/// または持ち上げた先に頭が入る空間がなければ `None`。
fn step_lift(entity: &Entity, next_x: i64, world: &World) -> Option<i64> {
    if obstacle_height(entity, next_x, world) > STEP_UP_CELLS {
        return None;
    }
    (1..=STEP_UP_CELLS * 16)
        .find(|&lift| !collides(next_x, entity.y16 - lift, entity.width, entity.height, world))
}

/// `step_player` の移動を、速さを引数にしたもの (仕様 10 §8.4)。`walk_speed`・`jump_speed` は 0..=96。
///
/// 操作性のための 3 つの助け (仕様 10 §8.4、どれも整数だけで、同期 (Lockstep) に影響しない):
/// 1. **段差の自動乗り越え**: 接地中に横へ進んで固体にぶつかったとき、`STEP_UP_CELLS` までの段なら、ぶつからない最小の
///    高さだけ持ち上げて進む (持ち上げた先で足場にぴったり着くので、接地は保たれる)。
/// 2. **コヨーテタイム**: 接地をやめてから `COYOTE_TICKS` の間は、ジャンプできる (ジャンプした直後は 0)。
/// 3. **ジャンプの先行入力**: 空中で押したジャンプは `JUMP_BUFFER_TICKS` の間覚えておき、着地したら跳ぶ。
pub fn step_entity(
    entity: &mut Entity,
    input: PlayerInput,
    walk_speed: i16,
    jump_speed: i16,
    world: &World,
) {
    debug_assert!((-1..=1).contains(&input.horizontal));
    entity.vx = (i16::from(input.horizontal.clamp(-1, 1)) * walk_speed.clamp(0, 96)
        + entity.ext_vx)
        .clamp(-512, 512);
    let wants_jump = input.jump || entity.jump_buffer > 0;
    if wants_jump && (entity.grounded || entity.coyote > 0) {
        entity.vy = -jump_speed.clamp(0, 96);
        entity.grounded = false;
        entity.coyote = 0;
        entity.jump_buffer = 0;
    } else if input.jump {
        entity.jump_buffer = JUMP_BUFFER_TICKS;
    } else {
        entity.jump_buffer = entity.jump_buffer.saturating_sub(1);
    }
    entity.vy = (entity.vy + 4).min(96);

    // 段差の乗り越えは、この tick の初めに接地していて、跳ばなかったときだけ。
    let can_step_up = entity.grounded;
    let step_x = entity.vx.signum() as i64;
    for _ in 0..entity.vx.unsigned_abs() {
        let next_x = entity.x16 + step_x;
        if collides(next_x, entity.y16, entity.width, entity.height, world) {
            if can_step_up && let Some(lift) = step_lift(entity, next_x, world) {
                entity.x16 = next_x;
                entity.y16 -= lift;
                continue;
            }
            entity.vx = 0;
            entity.ext_vx = 0;
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
    entity.coyote = if entity.grounded { COYOTE_TICKS } else { entity.coyote.saturating_sub(1) };
    // 外力の速度は、接地中は速く、空中はゆっくり減る (0 方向へ丸める)。
    entity.ext_vx = if entity.grounded { entity.ext_vx * 3 / 4 } else { entity.ext_vx * 15 / 16 };
}

/// セル `(x, y)` の正方形が、`x16`・`y16`・`width`・`height` の箱に内接する楕円と重なるか (整数だけで判定する)。
/// 楕円の中心は箱の中心、半径は幅・高さの半分。縁がちょうど触れるだけなら重ならない
/// (床や壁にぴったり立てる)。セルの中心だけで判定すると、床に少し沈んでぎくしゃくするため。
pub(crate) fn in_ellipse(x16: i64, y16: i64, width: u8, height: u8, x: i64, y: i64) -> bool {
    let (w, h) = (i64::from(width) * 16, i64::from(height) * 16);
    // 1/32 セル単位。楕円の中心 (x16 * 2 + w, y16 * 2 + h)、半径 (w, h)。
    let (cx, cy) = (x16 * 2 + w, y16 * 2 + h);
    let nearest_x = cx.clamp(x * 32, x * 32 + 32);
    let nearest_y = cy.clamp(y * 32, y * 32 + 32);
    let (a, b) = ((nearest_x - cx) * h, (nearest_y - cy) * w);
    a * a + b * b < (w * h) * (w * h)
}

/// 楕円の内側のセルが `Phase::Powder` なら、近くの空きセルへ押し出す (仕様 07 §2.3)。
/// エンティティが砂に勝つ: 砂に埋まらず、砂のほうがよける。セルの数は変えず、`World::swap` だけで動かす。
impl EntityWorld {
    /// 全エンティティの楕円に重なる粉体を、ID 順に押し出す。動かしたセルの数を返す。
    pub fn push_out_powder(&self, world: &mut World) -> usize {
        const REACH: i32 = 10;
        let mut moved = 0;
        for entity in self.entities.values() {
            let left = entity.x16.div_euclid(16);
            let top = entity.y16.div_euclid(16);
            let right = (entity.x16 + i64::from(entity.width) * 16 - 1).div_euclid(16);
            let bottom = (entity.y16 + i64::from(entity.height) * 16 - 1).div_euclid(16);
            for y in top..=bottom {
                for x in left..=right {
                    let (Ok(cx), Ok(cy)) = (i32::try_from(x), i32::try_from(y)) else { continue };
                    if !in_ellipse(entity.x16, entity.y16, entity.width, entity.height, x, y) {
                        continue;
                    }
                    let from = CellPos::new(cx, cy);
                    let cell = world.get(from);
                    if cell.has(FLAG_BODY)
                        || world.materials().get(cell.material).phase != Phase::Powder
                    {
                        continue;
                    }
                    // 近い順 (同じ距離なら上、次に左) の、どのエンティティの楕円の外でもある空きセル。
                    let mut best: Option<((i32, i32, i32), CellPos)> = None;
                    for dy in -REACH..=REACH {
                        for dx in -REACH..=REACH {
                            let to = CellPos::new(cx.saturating_add(dx), cy.saturating_add(dy));
                            let target = world.get(to);
                            if target.material != MaterialId::EMPTY || target.has(FLAG_BODY) {
                                continue;
                            }
                            let key = (dx * dx + dy * dy, dy, dx);
                            if best.is_some_and(|(k, _)| k <= key) {
                                continue;
                            }
                            let inside = self.entities.values().any(|e| {
                                in_ellipse(
                                    e.x16,
                                    e.y16,
                                    e.width,
                                    e.height,
                                    i64::from(to.x),
                                    i64::from(to.y),
                                )
                            });
                            if !inside {
                                best = Some((key, to));
                            }
                        }
                    }
                    if let Some((_, to)) = best {
                        world.swap(from, to);
                        moved += 1;
                    }
                }
            }
        }
        moved
    }
}

/// 楕円の内側のセルだけを検査し、未読み込み/範囲外も壁とする。
pub(crate) fn collides(x16: i64, y16: i64, width: u8, height: u8, world: &World) -> bool {
    let left = x16.div_euclid(16);
    let top = y16.div_euclid(16);
    let right = (x16 + i64::from(width) * 16 - 1).div_euclid(16);
    let bottom = (y16 + i64::from(height) * 16 - 1).div_euclid(16);
    for y in top..=bottom {
        for x in left..=right {
            if !in_ellipse(x16, y16, width, height, x, y) {
                continue;
            }
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
