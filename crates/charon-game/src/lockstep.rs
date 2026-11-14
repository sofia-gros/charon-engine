//! 仕様 07 §7: 同一プラットフォーム・同一ビルドでだけ進む入力同期。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{FLAG_BODY, FLAG_PARITY, MaterialId};
use charon_core::world::World;
use charon_physics::{BodyError, CELLS_PER_METER, PhysicsEngine};
use charon_world::background::BackgroundWorld;

use crate::defs::Definitions;
use crate::game::{GameError, GameInput, GameState};
use crate::mods::{ModError, ModManifest, ModPack};
use crate::{EntityError, EntityId, EntityWorld, PlayerInput};

pub const PROTOCOL_VERSION: u16 = 4;
pub const TICK_INPUT_BYTES: usize = 30;
/// 狙いのベクトルの各成分の上限 (仕様 10 §6.2)。
const MAX_AIM: i32 = 1_000_000;

/// version(2) + tick(8) + player(8) + horizontal(1) + jump(1) + fire(1) + aim_x(4) + aim_y(4) + tools(1)。すべて LE。`tools` は bit 0 = 道具 (`tool`)、bit 1 = 掘る道具 (`dig`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickInput {
    pub tick: u64,
    pub player: EntityId,
    pub input: PlayerInput,
    pub action: GameInput,
}

impl TickInput {
    pub fn encode(self) -> [u8; TICK_INPUT_BYTES] {
        let mut bytes = [0; TICK_INPUT_BYTES];
        bytes[..2].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes[2..10].copy_from_slice(&self.tick.to_le_bytes());
        bytes[10..18].copy_from_slice(&self.player.0.to_le_bytes());
        bytes[18] = self.input.horizontal as u8;
        bytes[19] = u8::from(self.input.jump);
        bytes[20] = u8::from(self.action.fire);
        bytes[21..25].copy_from_slice(&self.action.aim.0.to_le_bytes());
        bytes[25..29].copy_from_slice(&self.action.aim.1.to_le_bytes());
        bytes[29] = u8::from(self.action.tool) | (u8::from(self.action.dig) << 1);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LockstepError> {
        if bytes.len() != TICK_INPUT_BYTES {
            return Err(LockstepError::InvalidPacket);
        }
        let version = u16::from_le_bytes([bytes[0], bytes[1]]);
        if version != PROTOCOL_VERSION {
            return Err(LockstepError::Mismatch);
        }
        let tick =
            u64::from_le_bytes(bytes[2..10].try_into().map_err(|_| LockstepError::InvalidPacket)?);
        let player = EntityId(u64::from_le_bytes(
            bytes[10..18].try_into().map_err(|_| LockstepError::InvalidPacket)?,
        ));
        let horizontal = bytes[18] as i8;
        let aim = (
            i32::from_le_bytes(bytes[21..25].try_into().map_err(|_| LockstepError::InvalidPacket)?),
            i32::from_le_bytes(bytes[25..29].try_into().map_err(|_| LockstepError::InvalidPacket)?),
        );
        if tick == u64::MAX
            || player.0 == 0
            || !(-1..=1).contains(&horizontal)
            || bytes[19] > 1
            || bytes[20] > 1
            || bytes[29] > 3
            || aim.0.unsigned_abs() > MAX_AIM as u32
            || aim.1.unsigned_abs() > MAX_AIM as u32
        {
            return Err(LockstepError::InvalidPacket);
        }
        Ok(Self {
            tick,
            player,
            input: PlayerInput { horizontal, jump: bytes[19] == 1 },
            action: GameInput {
                fire: bytes[20] == 1,
                tool: bytes[29] & 1 == 1,
                dig: bytes[29] & 2 == 2,
                aim,
            },
        })
    }
}

/// 開始前に全項目を比較する。ビルド ID と feature 構成は起動側が明示して渡す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionInfo {
    pub version: u16,
    pub materials: Vec<String>,
    pub mods: Vec<ModManifest>,
    /// 呪文・杖・敵の定義の中身のハッシュ。定義が違う接続を拒否するため (仕様 10 §7.4)。
    pub definitions: u64,
    pub seed: u64,
    pub binary: String,
    pub features: String,
    pub platform: String,
}

fn sorted_mods(mods: &[ModManifest]) -> Vec<ModManifest> {
    let mut sorted = mods.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    sorted
}

impl ConnectionInfo {
    /// 定義の中身 (`Definitions::content_hash`) を接続情報に加える。
    #[must_use]
    pub fn with_definitions(mut self, definitions: &Definitions) -> Self {
        self.definitions = definitions.content_hash();
        self
    }

    pub fn new(world: &World, mods: &[ModManifest], binary: String, features: String) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            materials: charon_world::material_names(world.materials()),
            mods: sorted_mods(mods),
            definitions: 0,
            seed: world.seed(),
            binary,
            features: format!("core.parallel={};{features}", charon_core::PARALLEL_ENABLED),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        }
    }
}

#[derive(Debug)]
pub enum LockstepError {
    InvalidPacket,
    InvalidParticipant,
    NotConnected,
    WrongTick,
    DuplicateInput,
    Mismatch,
    Script(ModError),
    Entity(EntityError),
    Game(GameError),
    Body(BodyError),
    MissingBody,
}

impl std::fmt::Display for LockstepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPacket => write!(f, "入力パケットの長さまたは値が無効です"),
            Self::InvalidParticipant => write!(f, "参加者 ID が無効です"),
            Self::NotConnected => write!(f, "全参加者の接続確認が済んでいません"),
            Self::WrongTick => write!(f, "入力の tick が現在の tick と一致しません"),
            Self::DuplicateInput => write!(f, "同じ tick と参加者の入力が重複しました"),
            Self::Mismatch => write!(f, "プロトコル・材質・MOD・seed・ビルド構成が一致しません"),
            Self::Script(err) => write!(f, "規則の適用に失敗: {err}"),
            Self::Entity(err) => write!(f, "プレイヤーの更新に失敗: {err}"),
            Self::Game(err) => write!(f, "ゲーム状態の更新に失敗: {err}"),
            Self::Body(err) => write!(f, "剛体の更新に失敗: {err}"),
            Self::MissingBody => write!(f, "剛体の保存可能な状態を読めません"),
        }
    }
}

impl std::error::Error for LockstepError {}

/// 接続を検証済みの ID からのみ入力を受け取り、全員の入力が揃うまで停止する。
#[derive(Debug)]
pub struct Lockstep<'a> {
    participants: Vec<EntityId>,
    connected: BTreeSet<EntityId>,
    pending: BTreeMap<EntityId, (PlayerInput, GameInput)>,
    identity: ConnectionInfo,
    mods: Option<&'a ModPack>,
    tick: u64,
}

impl<'a> Lockstep<'a> {
    pub fn new(
        participants: &[EntityId],
        world: &World,
        entities: &EntityWorld,
        mods: Option<&'a ModPack>,
        identity: ConnectionInfo,
    ) -> Result<Self, LockstepError> {
        if identity.version != PROTOCOL_VERSION
            || identity.binary.is_empty()
            || world.tick() == u64::MAX
            || identity.seed != world.seed()
            || identity.materials != charon_world::material_names(world.materials())
            || mods.map_or(!identity.mods.is_empty(), |pack| {
                sorted_mods(&pack.manifests) != identity.mods
            })
        {
            return Err(LockstepError::Mismatch);
        }
        let mut sorted = participants.to_vec();
        sorted.sort_unstable();
        if sorted.iter().any(|id| id.0 == 0 || entities.get(*id).is_none())
            || sorted.windows(2).any(|ids| ids[0] == ids[1])
        {
            return Err(LockstepError::InvalidParticipant);
        }
        Ok(Self {
            participants: sorted,
            connected: BTreeSet::new(),
            pending: BTreeMap::new(),
            identity,
            mods,
            tick: world.tick(),
        })
    }

    /// 材質名、MOD、seed、バイナリ、feature、プラットフォームを開始前に照合する。
    pub fn connect(
        &mut self,
        player: EntityId,
        peer: &ConnectionInfo,
    ) -> Result<(), LockstepError> {
        if self.identity != *peer {
            return Err(LockstepError::Mismatch);
        }
        if self.participants.binary_search(&player).is_err() {
            return Err(LockstepError::InvalidParticipant);
        }
        self.connected.insert(player);
        Ok(())
    }

    pub fn submit(&mut self, input: TickInput) -> Result<(), LockstepError> {
        if input.tick != self.tick || input.tick == u64::MAX {
            return Err(LockstepError::WrongTick);
        }
        if input.player.0 == 0 {
            return Err(LockstepError::InvalidParticipant);
        }
        if !self.connected.contains(&input.player) {
            return Err(LockstepError::NotConnected);
        }
        if !(-1..=1).contains(&input.input.horizontal)
            || input.action.aim.0.unsigned_abs() > MAX_AIM as u32
            || input.action.aim.1.unsigned_abs() > MAX_AIM as u32
        {
            return Err(LockstepError::InvalidPacket);
        }
        match self.pending.entry(input.player) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert((input.input, input.action));
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(_) => Err(LockstepError::DuplicateInput),
        }
    }

    pub fn submit_bytes(&mut self, packet: &[u8]) -> Result<(), LockstepError> {
        self.submit(TickInput::decode(packet)?)
    }

    /// 欠損入力なら何も更新せず None。規則 → エンティティ → ゲーム状態 → 物理/セルの順に 1 回進める。
    pub fn advance(
        &mut self,
        world: &mut World,
        entities: &mut EntityWorld,
        physics: &mut PhysicsEngine,
        background: &BackgroundWorld,
        game: &mut GameState,
    ) -> Result<Option<u64>, LockstepError> {
        if self.tick != world.tick() {
            return Err(LockstepError::WrongTick);
        }
        if self.connected.len() != self.participants.len()
            || self.pending.len() != self.participants.len()
        {
            return Ok(None);
        }
        // 体力と杖を持たない参加者が撃とうとしたら、何も更新せずに拒否する。
        if self.pending.iter().any(|(id, (_, action))| {
            !game.wands.contains_key(id) && *action != GameInput::default()
        }) {
            return Err(LockstepError::Game(GameError::InvalidAction));
        }
        if let Some(mods) = self.mods {
            mods.apply_rules(world).map_err(LockstepError::Script)?;
        }
        let inputs: Vec<_> = self.pending.iter().map(|(&id, &(input, _))| (id, input)).collect();
        // 杖を持つキャラクターの入力だけをゲーム状態へ渡す。
        let actions: Vec<_> = self
            .pending
            .iter()
            .filter(|(id, _)| game.wands.contains_key(id))
            .map(|(&id, &(_, action))| (id, action))
            .collect();
        game.step_players(entities, &inputs, world).map_err(LockstepError::Entity)?;
        game.step(world, physics, entities, &actions, world.tick(), world.seed())
            .map_err(LockstepError::Game)?;
        physics.step(world).map_err(LockstepError::Body)?;
        let hash = synchronization_hash(world, entities, physics, background, game)?;
        self.pending.clear();
        self.tick = world.tick();
        Ok(Some(hash))
    }
}

struct Fnv64(u64);
impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
}

/// 前景・エンティティ・剛体の保存可能な状態・背景・ゲーム状態を順に FNV-1a に流す。
pub fn synchronization_hash(
    world: &World,
    entities: &EntityWorld,
    physics: &PhysicsEngine,
    background: &BackgroundWorld,
    game: &GameState,
) -> Result<u64, LockstepError> {
    let mut hash = Fnv64::new();
    hash.bytes(&world.state_hash().to_le_bytes());
    hash.bytes(&entities.state_hash().to_le_bytes());
    for (index, body) in physics.bodies.iter().enumerate() {
        let rigid = physics.world.bodies.get(body.handle).ok_or(LockstepError::MissingBody)?;
        hash.bytes(&(index as u64).to_le_bytes());
        hash.bytes(&body.width.to_le_bytes());
        hash.bytes(&body.height.to_le_bytes());
        for value in [
            rigid.translation().x * CELLS_PER_METER,
            rigid.translation().y * CELLS_PER_METER,
            rigid.rotation().angle(),
            rigid.linvel().x,
            rigid.linvel().y,
            rigid.angvel(),
        ] {
            hash.bytes(&value.to_le_bytes());
        }
        for cell in &body.cells {
            hash.bytes(&cell.material.0.to_le_bytes());
            hash.bytes(&[cell.flags & !(FLAG_BODY | FLAG_PARITY), cell.shade]);
            hash.bytes(&cell.temp.to_le_bytes());
            hash.bytes(&[cell.vy as u8, cell.life]);
        }
    }
    for (pos, chunk) in background.iter() {
        hash.bytes(&pos.x.to_le_bytes());
        hash.bytes(&pos.y.to_le_bytes());
        for &MaterialId(id) in chunk.cells.iter() {
            hash.bytes(&id.to_le_bytes());
        }
    }
    hash.bytes(&game.state_hash().to_le_bytes());
    Ok(hash.0)
}
