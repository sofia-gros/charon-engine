//! 仕様 10 §6.1・§8: 体力・杖・弾・敵を 1 つの tick の流れにまとめる。

use std::collections::{BTreeMap, BTreeSet};

use charon_core::cell::{FLAG_BODY, MaterialId};
use charon_core::coords::CellPos;
use charon_core::material::Phase;
use charon_core::rng;
use charon_core::world::World;
use charon_physics::PhysicsEngine;

use charon_physics::rope::{Rope, RopeEvent, RopeParams, RopePhase, tether_velocity};

use crate::defs::{Definitions, EnemyDef, HookDef};
use crate::dig::{Beam, Dig, Dug};
use crate::inventory::Inventory;
use crate::projectile::{Hit, ProjectileWorld};
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
    pub spells: SpellTable,
    /// 起きた出来事 (仕様 10 §15)。`step` が足し、ゲームが [`GameState::take_events`] で受け取る。
    events: Vec<GameEvent>,
}

/// ゲームに知らせる出来事。シミュレーションの結果を変えず、描画・音・得点などゲーム側の反応に使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameEvent {
    /// 弾が当たった (敵・キャラクター、または地形)。
    Hit(Hit),
    /// 敵を倒して撤去した。
    Defeated(EntityId),
    /// セルを掘った。
    Dug(Dug),
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
            spells,
            events: Vec::new(),
        }
    }

    /// 溜まった出来事を、起きた順に取り出す。
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    fn push_events(&mut self, report: &StepReport) {
        let all = report
            .hits
            .iter()
            .cloned()
            .map(GameEvent::Hit)
            .chain(report.defeated.iter().copied().map(GameEvent::Defeated))
            .chain(report.dug.iter().cloned().map(GameEvent::Dug));
        for event in all {
            if self.events.len() >= EVENTS_MAX {
                break;
            }
            self.events.push(event);
        }
    }

    /// 定義 (仕様 10 §7) の呪文表を取り込んだ、キャラクターのいない状態。
    pub fn from_definitions(definitions: &Definitions) -> Self {
        Self::new(definitions.spells.clone())
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
        // (3) プレイヤーの発射。
        let mut shooters = BTreeSet::new();
        for (&id, input) in &by_id {
            let alive = self.vitals.get(id).is_some_and(Vitals::alive);
            let (Some(entity), Some(wand)) = (entities.get(id), self.wands.get_mut(&id)) else {
                continue;
            };
            if !input.fire || !alive {
                continue;
            }
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
        // (4) 敵。
        self.step_enemies(world, entities, tick, seed);
        // (5) 弾の移動と着弾。
        let hits = self.projectiles.step(world, physics, entities, &mut self.vitals);
        // (6) 倒した敵の撤去。
        let defeated: Vec<EntityId> = self
            .enemies
            .keys()
            .copied()
            .filter(|&id| self.vitals.get(id).is_none_or(|v| !v.alive()))
            .collect();
        for &id in &defeated {
            self.enemies.remove(&id);
            self.vitals.remove(id);
            entities.remove(id);
        }
        let report = StepReport { hits, defeated, dug };
        self.push_events(&report);
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
                // 持ち物のあるキャラクターは、掘った材質を集める (入りきらない分は消える)。
                if let Some(inventory) = self.inventories.get_mut(&id) {
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
                    let horizontal = if def.attack.is_none() {
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
                    vitals.damage(def.contact_damage);
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
