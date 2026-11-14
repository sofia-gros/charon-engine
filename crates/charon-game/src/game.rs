//! 仕様 10 §6.1・§8: 体力・杖・弾・敵を 1 つの tick の流れにまとめる。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::rng;
use charon_core::world::World;
use charon_physics::PhysicsEngine;

use charon_physics::rope::{Rope, RopeEvent, RopeParams, RopePhase, tether_velocity};

use crate::blocks::{self, BlockPos, BreakError, PlaceError, Placed};
use crate::collapse::Collapse;
use crate::defs::{Definitions, EnemyDef, HookDef};
use crate::dig::{Beam, Dig, Dug};
use crate::drops::{Drops, MAX_DROPS, Pickup};
use crate::inventory::Inventory;
use crate::levitation::{Levitation, LevitationDef};
use crate::projectile::{Hit, ProjectileWorld};
use crate::status::StatusTable;
use crate::vitals::{Vitals, VitalsError, VitalsTable, covered_cells, step_environment};
use crate::wand::{CastError, CastOutcome, SpellTable, Wand, WandError};
use crate::{Entity, EntityError, EntityId, EntityWorld, PlayerInput, collides, step_entity};

/// 接触ダメージの間隔 (tick)。
pub const CONTACT_INTERVAL: u16 = 30;
/// 目標がいないときの徘徊の向きを変える間隔 (tick)。
const WANDER_INTERVAL: u64 = 45;
/// 徘徊の乱数の salt。
pub const SALT_WANDER: u32 = 0x574E_4452;
/// 狙いのベクトルの各成分の上限 (仕様 10 §5.2)。
const MAX_AIM: i64 = 1_000_000;

/// 1 tick のゲーム入力。左手 = 武器 (`fire`)、右手 = 道具 (`tool`)。`aim` は 2 つで共通の向きのベクトル。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameInput {
    /// 武器 (杖) を撃つ。左クリック。
    pub fire: bool,
    /// 道具 (ワイヤーフック) を使う。右クリック。押している間、ワイヤーを巻き取る。
    pub tool: bool,
    /// 掘る道具 (レーザー) を使う。押している間、狙った場所の円を削る (仕様 10 §12)。
    pub dig: bool,
    pub aim: (i32, i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameError {
    /// 存在しない ID、または同じ ID の重複。
    InvalidAction,
    Cast(CastError),
    /// 定義にない敵の種類。
    UnknownEnemy(String),
    /// 敵の攻撃が、呪文表にない呪文を参照している。
    UnknownSpell(String),
    /// 定義にないワイヤーフック。
    UnknownHook(String),
    /// 定義にない掘る道具。
    UnknownDig(String),
    Entity(EntityError),
    Wand(WandError),
    Vitals(VitalsError),
    /// 敵の武器・防具の名前が、アイテムの定義にない。
    UnknownItem(String),
}

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAction => write!(f, "ゲーム入力の ID が存在しないか重複しています"),
            Self::Cast(e) => write!(f, "詠唱に失敗: {e}"),
            Self::UnknownEnemy(name) => write!(f, "敵 \"{name}\" は定義されていません"),
            Self::UnknownSpell(name) => write!(f, "敵の攻撃の呪文 \"{name}\" が表にありません"),
            Self::UnknownHook(name) => write!(f, "ワイヤーフック \"{name}\" は定義されていません"),
            Self::UnknownDig(name) => write!(f, "掘る道具 \"{name}\" は定義されていません"),
            Self::Entity(e) => write!(f, "敵を生成できません: {e}"),
            Self::Wand(e) => write!(f, "{e}"),
            Self::Vitals(e) => write!(f, "{e}"),
            Self::UnknownItem(name) => write!(f, "アイテム \"{name}\" は定義されていません"),
        }
    }
}

impl std::error::Error for GameError {}

/// 動いている敵 1 体。種類の定義の写しを持つ (仕様 10 §8.2)。
#[derive(Clone, Debug)]
pub struct Enemy {
    pub def: EnemyDef,
    wand: Option<Wand>,
    attack_cooldown: u16,
    contact_cooldown: u16,
}

/// 右手に持つワイヤーフック 1 つ (仕様 10 §9.4)。定義の写しを持つ。
#[derive(Clone, Debug)]
pub struct Hook {
    pub def: HookDef,
    rope: Option<Rope>,
    cooldown: u16,
    held: bool,
}

impl Hook {
    pub fn rope(&self) -> Option<&Rope> {
        self.rope.as_ref()
    }

    pub fn cooldown(&self) -> u16 {
        self.cooldown
    }
}

/// `GameState::step` の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepReport {
    pub hits: Vec<Hit>,
    /// この tick に撤去した敵 (ID の昇順)。
    pub defeated: Vec<EntityId>,
    /// この tick に掘ったセル (ID・座標の昇順)。集める・拾うのはゲームの仕事 (仕様 10 §12)。
    pub dug: Vec<Dug>,
}

#[derive(Clone, Debug)]
pub struct GameState {
    pub vitals: VitalsTable,
    pub projectiles: ProjectileWorld,
    pub wands: BTreeMap<EntityId, Wand>,
    pub enemies: BTreeMap<EntityId, Enemy>,
    /// 右手のワイヤーフック。
    pub hooks: BTreeMap<EntityId, Hook>,
    /// 掘る道具 (レーザー)。
    pub digs: BTreeMap<EntityId, Dig>,
    /// 持ち物。持つキャラクターが掘ったセルは、材質ごとに数えて入る (仕様 10 §13)。
    pub inventories: BTreeMap<EntityId, Inventory>,
    /// 完全なブロックの採掘進捗 (固定 tick ごと、仕様 24 §3)。
    pub(crate) mining: BTreeMap<EntityId, MiningProgress>,
    /// チャンクの退避に関係なく残る設備の記録 (仕様 24 §4)。
    pub fixtures: crate::fixtures::FixtureMap,
    /// 装備 (仕様 19 §5)。
    pub equipment: BTreeMap<EntityId, crate::items::Equipment>,
    pub spells: SpellTable,
    /// 起きた出来事 (仕様 10 §15)。`step` が足し、ゲームが [`GameState::take_events`] で受け取る。
    pub(crate) events: Vec<GameEvent>,
    /// 地形の崩落 (仕様 10 §16)。`Some` のとき、掘って支えを失った固体が落ちる。
    pub collapse: Option<Collapse>,
    /// 状態異常 (仕様 10 §17)。
    pub statuses: StatusTable,
    /// 浮遊 (仕様 10 §18)。
    pub levitation: BTreeMap<EntityId, Levitation>,
    /// 落ちているもの (仕様 10 §19)。
    pub drops: Drops,
    /// 真なら、掘ったセルは持ち物に直接ではなく、材質ごとのドロップとして落ちる (仕様 10 §19)。
    pub dig_to_drops: bool,
    /// アイテムの定義の写し (防具の軽減・武器の種類を引く、仕様 20)。
    pub(crate) item_defs: Vec<crate::items::ItemDef>,
    /// 武器の次の攻撃までの tick (近接・遠距離)。
    pub(crate) weapon_cooldown: BTreeMap<EntityId, u16>,
    /// 敵の自動のスポーンと削除 (仕様 22 §3)。`None` なら、ゲームが `spawn_enemy` で出す。
    pub spawner: Option<crate::spawner::Spawner>,
    /// 倒れてから数えている tick (復活までの待ち、仕様 23 §1)。
    pub(crate) death_timer: BTreeMap<EntityId, u16>,
}

/// プレイヤーごとの完全ブロック採掘の途中経過 (仕様 24 §3)。保存には含めない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MiningProgress {
    pub at: BlockPos,
    pub tool: crate::items::ItemId,
    pub accumulated: u32,
}

/// ゲームに知らせる出来事。シミュレーションの結果を変えず、描画・音・得点などゲーム側の反応に使う。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameEvent {
    /// 弾が当たった (敵・キャラクター、または地形)。
    Hit(Hit),
    /// 敵を倒して撤去した (ID と、倒れた位置の中心のセル)。
    Defeated(EntityId, CellPos),
    /// セルを掘った。
    Dug(Dug),
    /// 落ちているものを拾った (仕様 10 §19)。
    Pickup(Pickup),
    /// ブロックを置いた (仕様 17 §3)。
    BlockPlaced(BlockPos, MaterialId),
    /// ブロックを壊した (仕様 17 §4)。
    BlockBroken(BlockPos, MaterialId),
    /// 近接の攻撃が当たった (仕様 20)。`damage` は防具で軽減したあとの量。
    Melee { attacker: EntityId, target: EntityId, damage: i32 },
    /// ボスを倒した (ID と、倒れた位置の中心のセル、仕様 22)。`Defeated` も同時に起きる。
    BossDefeated(EntityId, CellPos),
    /// プレイヤーが復活した (仕様 23 §1)。
    Respawned(EntityId),
}

/// ブロックの設置・破壊の失敗。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    /// 持ち物がない・ブロックのアイテムがない。
    NoBlock,
    /// ブロックを持ち切れない (上限)。
    InventoryFull,
    /// 設備がある位置は専用の撤去操作のみ許可する。
    FixtureAnchor,
    /// 置けない (仕様 17 §3)。
    Place(PlaceError),
    /// 壊せない (仕様 17 §4)。
    Break(BreakError),
}

/// 受け取られないまま溜まる出来事の上限。超えた分は捨てる (ゲームが受け取らなくても増え続けない)。
pub const EVENTS_MAX: usize = 4096;

/// 当たり判定の中心のセル (切り捨て)。
fn center_cell(e: &Entity) -> (i64, i64) {
    (e.x16.div_euclid(16) + i64::from(e.width) / 2, e.y16.div_euclid(16) + i64::from(e.height) / 2)
}

/// 当たり判定の中心 (1/16 セル)。
fn center16(e: &Entity) -> (i64, i64) {
    (e.x16 + i64::from(e.width) * 8, e.y16 + i64::from(e.height) * 8)
}

fn overlaps(a: &Entity, b: &Entity) -> bool {
    let (al, at, ar, ab) = covered_cells(a);
    let (bl, bt, br, bb) = covered_cells(b);
    al <= br && bl <= ar && at <= bb && bt <= ab
}

/// 視線を遮るセルか: 境界・剛体・固体・粉体 (仕様 10 §8.3)。
fn blocks_sight(world: &World, x: i64, y: i64) -> bool {
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else { return true };
    let cell = world.get(CellPos::new(x, y));
    cell.material == MaterialId::BOUNDARY
        || cell.has(FLAG_BODY)
        || matches!(world.materials().get(cell.material).phase, Phase::Solid | Phase::Powder)
}

/// 2 つのセルの間 (両端を除く) に遮るものがなければ真。整数の線分 (Bresenham)。
fn line_clear(world: &World, from: (i64, i64), to: (i64, i64)) -> bool {
    let (dx, dy) = ((to.0 - from.0).abs(), -(to.1 - from.1).abs());
    let (sx, sy) = ((to.0 - from.0).signum(), (to.1 - from.1).signum());
    let (mut x, mut y, mut err) = (from.0, from.1, dx + dy);
    loop {
        if (x, y) == to {
            return true;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        if (x, y) == to {
            return true;
        }
        if blocks_sight(world, x, y) {
            return false;
        }
    }
}

/// 定義から物理の設定へ (仕様 10 §9.3)。長さはセル、速さは 1/16 セル / tick を割る。
fn rope_params(def: &HookDef) -> RopeParams {
    RopeParams {
        segments: usize::from(def.segments),
        max_length: f32::from(def.max_length),
        min_length: f32::from(def.min_length),
        gravity: f32::from(def.gravity) / 16.0,
        iterations: usize::from(def.iterations),
        reel_speed: f32::from(def.reel_speed) / 16.0,
    }
}

impl GameState {
    pub fn new(spells: SpellTable) -> Self {
        Self {
            vitals: VitalsTable::new(),
            projectiles: ProjectileWorld::new(),
            wands: BTreeMap::new(),
            enemies: BTreeMap::new(),
            hooks: BTreeMap::new(),
            digs: BTreeMap::new(),
            inventories: BTreeMap::new(),
            mining: BTreeMap::new(),
            fixtures: crate::fixtures::FixtureMap::default(),
            equipment: BTreeMap::new(),
            spells,
            events: Vec::new(),
            collapse: None,
            statuses: StatusTable::new(),
            levitation: BTreeMap::new(),
            drops: Drops::new(),
            dig_to_drops: false,
            item_defs: Vec::new(),
            weapon_cooldown: BTreeMap::new(),
            spawner: None,
            death_timer: BTreeMap::new(),
        }
    }

    /// 持ち物から `material` を `count` 個出して、キャラクターの頭の高さに落とす (仕様 10 §19)。
    /// 足りない・エンティティがない・ドロップが上限なら何もせず `false`。
    pub fn drop_item(
        &mut self,
        entities: &EntityWorld,
        id: EntityId,
        material: MaterialId,
        count: u32,
    ) -> bool {
        let (Some(entity), Some(inventory)) = (entities.get(id), self.inventories.get_mut(&id))
        else {
            return false;
        };
        if count == 0 || inventory.count(material) < count || self.drops.len() >= MAX_DROPS {
            return false;
        }
        let (x, y) = center16(entity);
        if !inventory.take(material, count) {
            return false;
        }
        self.drops.spawn(material, count, x, y)
    }

    /// 浮遊の装備を渡す (燃料は満タン)。
    pub fn equip_levitation(&mut self, id: EntityId, def: LevitationDef) {
        self.levitation.insert(id, Levitation::new(def));
    }

    /// 浮遊を 1 tick 進める (仕様 10 §18)。`held` はジャンプのボタン。倒れている・装備がない・エンティティがないときは何もしない。
    /// 上向きに進んだら真。移動 (`step_players`) のあとに呼ぶ。
    pub fn step_levitation(
        &mut self,
        entities: &mut EntityWorld,
        id: EntityId,
        held: bool,
    ) -> bool {
        let alive = self.vitals.get(id).is_none_or(Vitals::alive);
        match (self.levitation.get_mut(&id), entities.get_mut(id)) {
            (Some(lev), Some(entity)) => lev.step(entity, held && alive),
            _ => false,
        }
    }

    /// 持ち物のブロックを 1 個使って、`at` に置く (仕様 17 §3)。置けなければ持ち物は変わらない。
    /// 届く範囲 (距離) の確認は、呼ぶ側が決める。
    pub fn place_block_item(
        &mut self,
        world: &mut World,
        entities: &mut EntityWorld,
        id: EntityId,
        at: BlockPos,
        material: MaterialId,
    ) -> Result<Placed, BuildError> {
        if self.fixtures.get(at).is_some() {
            return Err(BuildError::FixtureAnchor);
        }
        let has = self.inventories.get(&id).is_some_and(|inv| inv.block_count(material) > 0);
        if !has {
            return Err(BuildError::NoBlock);
        }
        let placed =
            blocks::place_block(world, entities, at, material).map_err(BuildError::Place)?;
        if let Some(inventory) = self.inventories.get_mut(&id) {
            inventory.take_block(material, 1);
        }
        if self.events.len() < EVENTS_MAX {
            self.events.push(GameEvent::BlockPlaced(at, material));
        }
        Ok(placed)
    }

    /// `at` の完全なブロックを壊して、持ち物にブロックのアイテムを 1 個加える (仕様 17 §4)。
    /// 持ち切れない (上限) ときは、壊さずに失敗する。
    pub fn break_block_item(
        &mut self,
        world: &mut World,
        id: EntityId,
        at: BlockPos,
    ) -> Result<MaterialId, BuildError> {
        if self.fixtures.get(at).is_some() {
            return Err(BuildError::FixtureAnchor);
        }
        let cells = at.cells();
        let material = cells.first().map(|&pos| world.get(pos).material);
        if let (Some(material), Some(inventory)) = (material, self.inventories.get(&id))
            && inventory.block_count(material) >= crate::inventory::STACK_MAX
        {
            return Err(BuildError::InventoryFull);
        }
        let material = blocks::break_block(world, at).map_err(BuildError::Break)?;
        if let Some(inventory) = self.inventories.get_mut(&id) {
            inventory.add_block(material, 1);
        }
        if self.events.len() < EVENTS_MAX {
            self.events.push(GameEvent::BlockBroken(at, material));
        }
        Ok(material)
    }

    /// 材質のセル 64 個で、その材質のブロックを 1 個作る (仕様 17 §4)。`Solid` と `Powder` の材質だけ。
    pub fn craft_block(&mut self, world: &World, id: EntityId, material: MaterialId) -> bool {
        if !matches!(world.materials().get(material).phase, Phase::Solid | Phase::Powder)
            || material == MaterialId::EMPTY
            || material == MaterialId::BOUNDARY
        {
            return false;
        }
        self.inventories.get_mut(&id).is_some_and(|inventory| inventory.craft_block(material))
    }

    /// 溜まっている出来事の数。
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// `start` 番目以降の出来事を、取り出さずに見る (`event_count` を `start` に使うと、それ以降に起きた分)。
    pub fn events_since(&self, start: usize) -> &[GameEvent] {
        self.events.get(start..).unwrap_or(&[])
    }

    /// 溜まった出来事を、起きた順に取り出す。
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    fn push_events(
        &mut self,
        report: &StepReport,
        defeated_at: &[(EntityId, CellPos)],
        picked: &[Pickup],
    ) {
        let all = report
            .hits
            .iter()
            .cloned()
            .map(GameEvent::Hit)
            .chain(defeated_at.iter().map(|&(id, pos)| GameEvent::Defeated(id, pos)))
            .chain(report.dug.iter().cloned().map(GameEvent::Dug))
            .chain(picked.iter().copied().map(GameEvent::Pickup));
        for event in all {
            if self.events.len() >= EVENTS_MAX {
                break;
            }
            self.events.push(event);
        }
    }

    /// 定義 (仕様 10 §7) の呪文表を取り込んだ、キャラクターのいない状態。
    pub fn from_definitions(definitions: &Definitions) -> Self {
        let mut game = Self::new(definitions.spells.clone());
        game.item_defs = definitions.items.clone();
        game
    }

    /// 体力と杖を持つキャラクターとして登録する。
    pub fn add_character(&mut self, id: EntityId, vitals: Vitals, wand: Wand) {
        self.vitals.insert(id, vitals);
        self.wands.insert(id, wand);
    }

    /// 仕様 10 §9.4: 杖を持つキャラクターの右手にワイヤーフック `name` を持たせる。
    pub fn equip_hook(
        &mut self,
        id: EntityId,
        defs: &Definitions,
        name: &str,
    ) -> Result<(), GameError> {
        let def = defs.hook(name).ok_or_else(|| GameError::UnknownHook(name.to_owned()))?;
        if !self.wands.contains_key(&id) {
            return Err(GameError::InvalidAction);
        }
        self.hooks.insert(id, Hook { def: def.clone(), rope: None, cooldown: 0, held: false });
        Ok(())
    }

    /// 仕様 10 §12: 杖を持つキャラクターに、掘る道具 `name` を持たせる。
    pub fn equip_dig(
        &mut self,
        id: EntityId,
        defs: &Definitions,
        name: &str,
    ) -> Result<(), GameError> {
        let def = defs.dig(name).ok_or_else(|| GameError::UnknownDig(name.to_owned()))?;
        if !self.wands.contains_key(&id) {
            return Err(GameError::InvalidAction);
        }
        self.digs.insert(id, Dig::new(def.clone()));
        Ok(())
    }

    /// 杖を持つキャラクターに、空の持ち物を持たせる。すでにあるときは中身を残す。
    pub fn give_inventory(&mut self, id: EntityId) -> Result<(), GameError> {
        if !self.wands.contains_key(&id) {
            return Err(GameError::InvalidAction);
        }
        self.inventories.entry(id).or_default();
        Ok(())
    }

    /// 出ている掘る道具の光線 (描画用)。
    pub fn dig_beam(&self, id: EntityId) -> Option<Beam> {
        self.digs.get(&id)?.beam()
    }

    /// 出ているワイヤー (描画用)。
    pub fn rope(&self, id: EntityId) -> Option<&Rope> {
        self.hooks.get(&id)?.rope.as_ref()
    }

    /// 仕様 10 §8.2: 定義 `name` の敵を、左上のセル `cell` に作る。失敗したときは何も変えない。
    pub fn spawn_enemy(
        &mut self,
        defs: &Definitions,
        entities: &mut EntityWorld,
        world: &World,
        name: &str,
        cell: (i32, i32),
    ) -> Result<EntityId, GameError> {
        let def = defs.enemy(name).ok_or_else(|| GameError::UnknownEnemy(name.to_owned()))?;
        let wand = match &def.attack {
            Some(spell) => {
                let id =
                    self.spells.id(spell).ok_or_else(|| GameError::UnknownSpell(spell.clone()))?;
                // 常にマナが満タンの、呪文 1 つの杖。撃つ間隔は `attack_interval` で決める。
                Some(
                    Wand::new(vec![id], 10_000, 10_000, 0, &self.spells)
                        .map_err(GameError::Wand)?,
                )
            }
            None => None,
        };
        // 持っている武器・防具を、自動装備する (仕様 22 §1)。名前が不正なら、何も作らずに失敗する (ID も使わない)。
        let equipment = if def.weapon.is_some() || !def.armor.is_empty() {
            let mut carried = Inventory::new();
            for name in def.weapon.iter().chain(def.armor.iter()) {
                let item = defs
                    .item_id(name, world.materials())
                    .ok_or_else(|| GameError::UnknownItem(name.clone()))?;
                carried.add_item(item, 1);
            }
            let mut equipment = crate::items::Equipment::new();
            crate::loadout::auto_equip_best(&mut carried, &mut equipment, defs);
            Some(equipment)
        } else {
            None
        };
        let vitals = Vitals::new(def.max_hp).map_err(GameError::Vitals)?;
        let id = entities
            .spawn_checked(
                i64::from(cell.0) * 16,
                i64::from(cell.1) * 16,
                def.width,
                def.height,
                world,
            )
            .map_err(GameError::Entity)?;
        self.vitals.insert(id, vitals);
        self.enemies
            .insert(id, Enemy { def: def.clone(), wand, attack_cooldown: 0, contact_cooldown: 0 });
        if let Some(equipment) = equipment {
            self.equipment.insert(id, equipment);
        }
        Ok(id)
    }

    /// 仕様 10 §6.1・§8.3 の順序で 1 tick 進める。
    /// 入力の検査に失敗したときは何も更新しない。
    pub fn step(
        &mut self,
        world: &mut World,
        physics: &mut PhysicsEngine,
        entities: &mut EntityWorld,
        actions: &[(EntityId, GameInput)],
        tick: u64,
        seed: u64,
    ) -> Result<StepReport, GameError> {
        let mut by_id = BTreeMap::new();
        for &(id, input) in actions {
            if !self.wands.contains_key(&id) || by_id.insert(id, input).is_some() {
                return Err(GameError::InvalidAction);
            }
        }

        for (_, vitals) in self.vitals.iter_mut() {
            vitals.tick_combat_immunity();
        }
        self.refresh_armor();
        self.tick_weapon_cooldowns();
        // (1) 環境ダメージ。
        let ids: Vec<EntityId> = self.vitals.iter().map(|(id, _)| id).collect();
        for &id in &ids {
            if let (Some(entity), Some(vitals)) = (entities.get(id), self.vitals.get_mut(id)) {
                step_environment(entity, vitals, world);
                self.statuses.step(id, entity, vitals, world);
            }
        }
        // (2) 杖の回復と待ち時間。
        for wand in self.wands.values_mut() {
            wand.tick();
        }
        // (3) プレイヤーの発射。
        let mut shooters = BTreeSet::new();
        for (&id, input) in &by_id {
            let alive = self.vitals.get(id).is_some_and(Vitals::alive);
            if !input.fire || !alive {
                continue;
            }
            // 装備している武器 (仕様 20): 近接は斬る、遠距離は矢を撃つ。杖・何も持たないときは、杖で撃つ。
            match self.equipped_weapon(id) {
                Some((crate::items::WeaponClass::Melee, damage)) => {
                    self.try_melee(entities, id, damage, input.aim);
                    continue;
                }
                Some((crate::items::WeaponClass::Ranged, damage)) => {
                    if let Some(pid) = self.try_shoot_ranged(world, entities, id, damage, input.aim)
                    {
                        shooters.insert((pid, id));
                    }
                    continue;
                }
                _ => {}
            }
            let (Some(entity), Some(wand)) = (entities.get(id), self.wands.get_mut(&id)) else {
                continue;
            };
            let muzzle = center16(entity);
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
        // (3') 道具 (ワイヤーフック)。
        self.step_hooks(world, entities, &by_id);
        // (3'') 掘る道具 (レーザー)。
        let dug = self.step_digs(world, entities, &by_id);
        // (3''') 地形の崩落 (仕様 10 §16): 掘って支えを失ったかたまりを落とす。
        if let Some(collapse) = self.collapse.as_mut() {
            let cells: Vec<CellPos> = dug.iter().map(|d| d.pos).collect();
            collapse.notice(world, &cells);
            collapse.step(world);
        }
        // (4) 敵。
        self.step_enemies(world, entities, tick, seed);
        // (5) 弾の移動と着弾。
        let (hits, applied) =
            self.projectiles.step_with_damage(world, physics, entities, &mut self.vitals);
        // 実際に直撃の体力が減ったときだけ弾き飛ばす。Hit.damage は基本ダメージのまま。
        for (hit, dealt) in hits.iter().zip(applied) {
            if let Some(target) = hit.entity
                && hit.vx != 0
                && dealt > 0
            {
                self.knockback(entities, target, i64::from(hit.vx.signum()), dealt);
            }
        }
        // (6) 倒した敵の撤去。
        let defeated: Vec<EntityId> = self
            .enemies
            .keys()
            .copied()
            .filter(|&id| self.vitals.get(id).is_none_or(|v| !v.alive()))
            .collect();
        let mut defeated_at = Vec::new();
        let mut boss_down: Vec<(EntityId, CellPos)> = Vec::new();
        for &id in &defeated {
            if let Some(entity) = entities.get(id) {
                let (cx, cy) = center_cell(entity);
                let to_i32 = |v: i64| i32::try_from(v).unwrap_or(0);
                defeated_at.push((id, CellPos::new(to_i32(cx), to_i32(cy))));
            }
            // 戦利品を落とす。ボスなら出来事を足す (仕様 22)。
            if let (Some(enemy), Some(entity)) = (self.enemies.get(&id), entities.get(id)) {
                let (x, y) = center16(entity);
                for (name, count) in &enemy.def.loot {
                    let name = name.strip_prefix("cells:").unwrap_or(name);
                    if let Some(material) = world.materials().id(name) {
                        self.drops.spawn(material, *count, x, y);
                    }
                }
                if enemy.def.boss {
                    let (cx, cy) = center_cell(entity);
                    let to_i32 = |v: i64| i32::try_from(v).unwrap_or(0);
                    boss_down.push((id, CellPos::new(to_i32(cx), to_i32(cy))));
                }
            }
            self.enemies.remove(&id);
            self.equipment.remove(&id);
            self.weapon_cooldown.remove(&id);
            self.statuses.clear(id);
            self.vitals.remove(id);
            entities.remove(id);
        }
        // (7) 落ちているものの移動と、持ち物のあるキャラクターが拾うこと (仕様 10 §19)。
        let collectors: Vec<(EntityId, i64, i64)> = self
            .inventories
            .keys()
            .filter(|id| self.vitals.get(**id).is_none_or(Vitals::alive))
            .filter_map(|&id| entities.get(id).map(|e| (id, center16(e))))
            .map(|(id, (x, y))| (id, x, y))
            .collect();
        let inventories = &mut self.inventories;
        let picked = self.drops.step(world, &collectors, |who, material, count| {
            inventories.get_mut(&who).map_or(0, |inventory| inventory.add(material, count))
        });
        let report = StepReport { hits, defeated, dug };
        self.push_events(&report, &defeated_at, &picked);
        for (id, pos) in boss_down {
            self.push_event(GameEvent::BossDefeated(id, pos));
        }
        Ok(report)
    }

    /// 仕様 10 §12: 掘る道具を ID 昇順に更新する。倒れたキャラクターは使えない。
    fn step_digs(
        &mut self,
        world: &mut World,
        entities: &EntityWorld,
        inputs: &BTreeMap<EntityId, GameInput>,
    ) -> Vec<Dug> {
        let mut dug = Vec::new();
        let ids: Vec<EntityId> = self.digs.keys().copied().collect();
        for id in ids {
            let input = inputs.get(&id).copied().unwrap_or_default();
            let alive = self.vitals.get(id).is_some_and(Vitals::alive);
            let Some(entity) = entities.get(id) else { continue };
            let hand = center16(entity);
            if let Some(tool) = self.digs.get_mut(&id) {
                let mine = tool.step(world, hand, input.aim, input.dig && alive);
                if self.dig_to_drops {
                    // 材質ごとにまとめて、最初に掘ったセルの位置に落とす (仕様 10 §19)。
                    let mut groups: BTreeMap<u16, (u32, CellPos)> = BTreeMap::new();
                    for cell in &mine {
                        groups.entry(cell.material.0).or_insert((0, cell.pos)).0 += 1;
                    }
                    for (material, (count, pos)) in groups {
                        self.drops.spawn(
                            MaterialId(material),
                            count,
                            i64::from(pos.x) * 16 + 8,
                            i64::from(pos.y) * 16 + 8,
                        );
                    }
                } else if let Some(inventory) = self.inventories.get_mut(&id) {
                    // 持ち物のあるキャラクターは、掘った材質を集める (入りきらない分は消える)。
                    for cell in &mine {
                        inventory.add(cell.material, 1);
                    }
                }
                dug.extend(mine);
            }
        }
        dug
    }

    /// 仕様 10 §9.4: ワイヤーフックを ID 昇順に更新し、張ったワイヤーでプレイヤーを引く。
    fn step_hooks(
        &mut self,
        world: &World,
        entities: &mut EntityWorld,
        inputs: &BTreeMap<EntityId, GameInput>,
    ) {
        let ids: Vec<EntityId> = self.hooks.keys().copied().collect();
        for id in ids {
            let input = inputs.get(&id).copied().unwrap_or_default();
            let alive = self.vitals.get(id).is_some_and(Vitals::alive);
            let Some(entity) = entities.get(id).cloned() else { continue };
            let (cx, cy) = center16(&entity);
            let hand = [cx as f32 / 16.0, cy as f32 / 16.0];
            let Some(hook) = self.hooks.get_mut(&id) else { continue };
            hook.cooldown = hook.cooldown.saturating_sub(1);
            let def = hook.def.clone();
            let params = rope_params(&def);
            let tool = input.tool && alive;
            // 押した tick に撃つ (出ていなくて、待ち時間が明けているとき)。
            if tool && !hook.held && hook.rope.is_none() && hook.cooldown == 0 {
                let direction = [input.aim.0 as f32, input.aim.1 as f32];
                hook.rope = Rope::launch(hand, direction, f32::from(def.speed) / 16.0, &params);
            }
            hook.held = tool;
            // 離した、または倒れたら外す。
            if hook.rope.is_some() && !tool {
                hook.rope = None;
                hook.cooldown = def.cooldown;
                continue;
            }
            let Some(rope) = hook.rope.as_mut() else { continue };
            let reel = tool && rope.phase() == RopePhase::Anchored;
            let mut solid =
                |p: [f32; 2]| blocks_sight(world, p[0].floor() as i64, p[1].floor() as i64);
            if rope.step(hand, reel, &params, &mut solid) == RopeEvent::Lost {
                hook.rope = None;
                hook.cooldown = def.cooldown;
                continue;
            }
            if rope.phase() != RopePhase::Anchored {
                continue;
            }
            // 張ったワイヤーでプレイヤーの速度を直す (振り子と巻き取り)。
            let Some(body) = entities.get_mut(id) else { continue };
            // 次の tick の重力 (`step_entity` が vy に 4 足す) を見込んで補正する。
            let velocity = [f32::from(body.vx) / 16.0, (f32::from(body.vy) + 4.0) / 16.0];
            let next = tether_velocity(
                rope,
                hand,
                velocity,
                reel,
                f32::from(def.pull_accel) / 16.0,
                f32::from(def.pull_max) / 16.0,
            );
            if !next.iter().all(|v| v.is_finite()) {
                continue;
            }
            let control = i32::from(body.vx) - i32::from(body.ext_vx);
            body.vy = ((next[1] * 16.0).round() - 4.0).clamp(-256.0, 256.0) as i16;
            body.ext_vx = ((next[0] * 16.0).round() as i32 - control).clamp(-512, 512) as i16;
        }
    }

    /// 最も近く、視界と視線の通る生存中のキャラクター (仕様 10 §8.3 の 2)。
    fn pick_target(
        &self,
        me: &Entity,
        sight: u16,
        entities: &EntityWorld,
        world: &World,
    ) -> Option<Entity> {
        let (mx, my) = center_cell(me);
        let sight = i64::from(sight);
        let mut best: Option<(i64, &Entity)> = None;
        // `wands` は ID の昇順なので、同じ距離なら ID が小さいものが残る。
        for &id in self.wands.keys() {
            if !self.vitals.get(id).is_some_and(Vitals::alive) {
                continue;
            }
            let Some(target) = entities.get(id) else { continue };
            let (tx, ty) = center_cell(target);
            let dist2 = (tx - mx) * (tx - mx) + (ty - my) * (ty - my);
            if dist2 > sight * sight || !line_clear(world, (mx, my), (tx, ty)) {
                continue;
            }
            if best.is_none_or(|(d, _)| dist2 < d) {
                best = Some((dist2, target));
            }
        }
        best.map(|(_, target)| target.clone())
    }

    /// 敵を ID 昇順に動かし、撃ち、接触ダメージを与える (仕様 10 §8.3 の 1〜6)。
    fn step_enemies(&mut self, world: &World, entities: &mut EntityWorld, tick: u64, seed: u64) {
        let ids: Vec<EntityId> = self.enemies.keys().copied().collect();
        for id in ids {
            if !self.vitals.get(id).is_some_and(Vitals::alive) {
                continue;
            }
            let Some(me) = entities.get(id).cloned() else { continue };
            let Some(def) = self.enemies.get(&id).map(|e| e.def.clone()) else { continue };
            if let Some(enemy) = self.enemies.get_mut(&id) {
                enemy.attack_cooldown = enemy.attack_cooldown.saturating_sub(1);
                enemy.contact_cooldown = enemy.contact_cooldown.saturating_sub(1);
                if let Some(wand) = enemy.wand.as_mut() {
                    wand.tick();
                }
            }

            let target = self.pick_target(&me, def.sight, entities, world);
            let (mx, _) = center_cell(&me);
            let (horizontal, jump) = match &target {
                None => {
                    let h =
                        rng::hash(seed, tick / WANDER_INTERVAL, id.0 as i32, 0, SALT_WANDER) % 3;
                    (h as i8 - 1, false)
                }
                Some(target) => {
                    let (tx, _) = center_cell(target);
                    let dx = tx - mx;
                    let toward = dx.signum() as i8;
                    let ranged = matches!(
                        self.equipped_weapon(id),
                        Some((crate::items::WeaponClass::Ranged, _))
                    );
                    let horizontal = if def.attack.is_none() && !ranged {
                        // 当たり判定が重なるまで近づく (仕様 10 §8.3)。
                        let reach =
                            ((i64::from(me.width) + i64::from(target.width)) / 2 - 2).max(0);
                        if dx.abs() > reach { toward } else { 0 }
                    } else {
                        let stand_off = i64::from(def.sight) / 2;
                        if dx.abs() > stand_off {
                            toward
                        } else if dx.abs() < stand_off / 2 {
                            -toward
                        } else {
                            0
                        }
                    };
                    let blocked_ahead = horizontal != 0
                        && collides(
                            me.x16 + i64::from(horizontal) * 16,
                            me.y16,
                            me.width,
                            me.height,
                            world,
                        );
                    // 目標の上端が自分の上端より 4 セル以上上にあり、ほぼ真上にいる。
                    let target_above =
                        target.y16.div_euclid(16) + 4 <= me.y16.div_euclid(16) && dx.abs() <= 4;
                    (horizontal, me.grounded && (blocked_ahead || target_above))
                }
            };

            if let Some(entity) = entities.get_mut(id) {
                step_entity(
                    entity,
                    PlayerInput { horizontal, jump },
                    def.walk_speed,
                    def.jump_speed,
                    world,
                );
            }
            let Some(me) = entities.get(id).cloned() else { continue };

            // 攻撃。
            if let (Some(target), Some(enemy)) = (&target, self.enemies.get_mut(&id))
                && enemy.attack_cooldown == 0
                && let Some(wand) = enemy.wand.as_mut()
            {
                let (mcx, mcy) = center16(&me);
                let (tcx, tcy) = center16(target);
                let aim = (
                    (tcx - mcx).clamp(-MAX_AIM, MAX_AIM) as i32,
                    (tcy - mcy).clamp(-MAX_AIM, MAX_AIM) as i32,
                );
                if let Ok(CastOutcome::Fired(pid)) = wand.cast(
                    &self.spells,
                    &mut self.projectiles,
                    world,
                    (mcx, mcy),
                    aim,
                    tick,
                    seed,
                ) {
                    self.projectiles.set_grace(pid, id);
                    enemy.attack_cooldown = def.attack_interval;
                }
            }

            // 武器 (仕様 22 §2): 近接は目標が箱に入ったら斬る、遠距離は目標へ矢を撃つ。
            if let Some(target) = &target {
                let (mcx, mcy) = center16(&me);
                let (tcx, tcy) = center16(target);
                let (dx, dy) = ((tcx - mcx) / 16, (tcy - mcy) / 16);
                match self.equipped_weapon(id) {
                    Some((crate::items::WeaponClass::Melee, damage))
                        if dx.abs() <= i64::from(me.width) / 2 + crate::combat::MELEE_REACH - 2
                            && dy.abs() <= crate::combat::MELEE_HALF_HEIGHT =>
                    {
                        let aim = (i32::try_from(dx).unwrap_or(0), 0);
                        self.try_melee(entities, id, damage, aim);
                    }
                    Some((crate::items::WeaponClass::Ranged, damage)) => {
                        // 重力で落ちる分、遠いほど上を狙う。
                        let aim = (
                            i32::try_from(dx * 16).unwrap_or(0),
                            i32::try_from((dy - dx.abs() / 3) * 16).unwrap_or(0),
                        );
                        if let Some(pid) = self.try_shoot_ranged(world, entities, id, damage, aim) {
                            self.projectiles.set_grace(pid, id);
                        }
                    }
                    _ => {}
                }
            }

            // 接触ダメージ。
            if def.contact_damage > 0
                && self.enemies.get(&id).is_some_and(|e| e.contact_cooldown == 0)
            {
                let victim = self.wands.keys().copied().find(|&cid| {
                    self.vitals.get(cid).is_some_and(Vitals::alive)
                        && entities.get(cid).is_some_and(|c| overlaps(&me, c))
                });
                if let Some(victim) = victim
                    && let Some(vitals) = self.vitals.get_mut(victim)
                {
                    vitals.damage_combat(def.contact_damage, crate::vitals::DamageKind::Physical);
                    if let Some(enemy) = self.enemies.get_mut(&id) {
                        enemy.contact_cooldown = CONTACT_INTERVAL;
                    }
                }
            }
        }
    }

    /// 仕様 10 §6.1・§8: 体力・弾・杖・敵 (ID 昇順) を FNV-1a 64-bit で連結する。
    pub fn state_hash(&self) -> u64 {
        const PRIME: u64 = 0x100000001b3;
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
            }
        };
        feed(&self.vitals.state_hash().to_le_bytes());
        feed(&self.projectiles.state_hash().to_le_bytes());
        for (&id, wand) in &self.wands {
            feed(&id.0.to_le_bytes());
            feed(&wand.state_hash().to_le_bytes());
        }
        for (&id, hook) in &self.hooks {
            feed(&id.0.to_le_bytes());
            feed(&hook.cooldown.to_le_bytes());
            feed(&[u8::from(hook.held)]);
            feed(&hook.rope.as_ref().map_or(0, Rope::state_hash).to_le_bytes());
        }
        for (&id, dig) in &self.digs {
            feed(&id.0.to_le_bytes());
            feed(&dig.state_hash().to_le_bytes());
        }
        for (&id, inventory) in &self.inventories {
            feed(&id.0.to_le_bytes());
            feed(&inventory.state_hash().to_le_bytes());
        }
        // 途中経過がなければ印を流さず、既存の状態ハッシュを維持する。
        for (&id, progress) in &self.mining {
            feed(&[0x4D, 0x49]);
            feed(&id.0.to_le_bytes());
            feed(&progress.at.bx.to_le_bytes());
            feed(&progress.at.by.to_le_bytes());
            feed(&progress.tool.0.to_le_bytes());
            feed(&progress.accumulated.to_le_bytes());
        }
        if !self.fixtures.is_empty() {
            feed(&[0x46, 0x58]);
            feed(&self.fixtures.state_hash().to_le_bytes());
        }
        // 装備がなければ、これまでと同じ値 (印も流さない)。
        for (&id, equipment) in self.equipment.iter().filter(|(_, e)| e.iter().next().is_some()) {
            feed(&[0xE9, 0x01]);
            feed(&id.0.to_le_bytes());
            feed(&equipment.state_hash().to_le_bytes());
        }
        // 復活の待ちがなければ、これまでと同じ値 (印も流さない)。
        for (&id, ticks) in &self.death_timer {
            feed(&[0xDE, 0xAD]);
            feed(&id.0.to_le_bytes());
            feed(&ticks.to_le_bytes());
        }
        // 武器の間隔がなければ、これまでと同じ値 (印も流さない)。
        for (&id, left) in &self.weapon_cooldown {
            feed(&[0xCD, 0x01]);
            feed(&id.0.to_le_bytes());
            feed(&left.to_le_bytes());
        }
        for (&id, enemy) in &self.enemies {
            feed(&id.0.to_le_bytes());
            feed(enemy.def.name.as_bytes());
            feed(&enemy.attack_cooldown.to_le_bytes());
            feed(&enemy.contact_cooldown.to_le_bytes());
            feed(&enemy.wand.as_ref().map_or(0, Wand::state_hash).to_le_bytes());
        }
        hash
    }
}
