//! 仕様 22 §3: 敵の自動のスポーンと削除。プレイヤーのまわりの、画面の外に敵を出し、遠く・チャンクの外の敵を消す。整数だけで決定的。

use charon_core::coords::CellPos;
use charon_core::rng::hash;
use charon_core::world::World;

use crate::defs::Definitions;
use crate::game::GameState;
use crate::{EntityId, EntityWorld, collides};

const SALT_SPAWN_GATE: u32 = 0x5350_0001;
const SALT_SPAWN_SIDE: u32 = 0x5350_0002;
const SALT_SPAWN_DIST: u32 = 0x5350_0003;
const SALT_SPAWN_PICK: u32 = 0x5350_0004;
const SALT_SPAWN_WHO: u32 = 0x5350_0005;
const SALT_BOSS: u32 = 0x5350_0006;

/// 1 つの出現の規則。`min_y..max_y` (セルの y) の範囲に出る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRule {
    /// 敵の種類の名前。
    pub enemy: String,
    pub min_y: i32,
    pub max_y: i32,
    /// 選ばれやすさ。
    pub weight: u32,
    /// ボス。同時に 1 体まで、まれに出る。
    pub boss: bool,
}

impl SpawnRule {
    pub fn new(enemy: &str, min_y: i32, max_y: i32, weight: u32) -> Self {
        Self { enemy: enemy.to_owned(), min_y, max_y, weight, boss: false }
    }
}

/// 敵の出現の設定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawner {
    pub rules: Vec<SpawnRule>,
    /// ボスでない敵の同時の数の上限。
    pub max_enemies: usize,
    /// 出そうとする間隔 (tick)。
    pub interval: u64,
    /// プレイヤーからの、出る距離の範囲 (セル、横)。画面の外 (近い方より外)。
    pub near: i32,
    pub far: i32,
    /// これより遠い (どのプレイヤーからも) 敵は消す (セル)。
    pub despawn: i32,
}

impl Default for Spawner {
    fn default() -> Self {
        Self { rules: Vec::new(), max_enemies: 8, interval: 90, near: 120, far: 200, despawn: 320 }
    }
}

impl Spawner {
    /// 冒険の世界 (仕様 26 §4、表面 `y = 200`) の規則: 地表→石炭→鉄/金→深い金とボス。
    pub fn adventure() -> Self {
        let mut boss = SpawnRule::new("golem", 920, 1400, 1);
        boss.boss = true;
        Self {
            rules: vec![
                SpawnRule::new("walker", 150, 260, 4),
                SpawnRule::new("archer", 150, 260, 2),
                SpawnRule::new("skeleton", 260, 920, 3),
                SpawnRule::new("shooter", 260, 560, 2),
                SpawnRule::new("mage", 560, 1400, 2),
                boss,
            ],
            ..Self::default()
        }
    }
}

/// 箱 (左上のセルと大きさ) を含むチャンクが、全部読み込まれているか (足元の 1 行を含む)。
fn loaded(world: &World, x: i32, y: i32, width: u8, height: u8) -> bool {
    let (x1, y1) = (x + i32::from(width) - 1, y + i32::from(height));
    [(x, y), (x1, y), (x, y1), (x1, y1)]
        .iter()
        .all(|&(cx, cy)| world.chunk(CellPos::new(cx, cy).chunk()).is_some())
}

impl GameState {
    /// 自動のスポーンと削除を 1 tick 進める (`spawner` があるときだけ)。プレイヤー (杖を持つ、敵でないもの) がいなければ何もしない。
    pub fn step_spawner(
        &mut self,
        world: &World,
        entities: &mut EntityWorld,
        defs: &Definitions,
        tick: u64,
        seed: u64,
    ) {
        let Some(spawner) = self.spawner.clone() else { return };
        let players: Vec<(EntityId, i64, i64)> = self
            .wands
            .keys()
            .filter(|id| !self.enemies.contains_key(id))
            .filter(|id| self.vitals.get(**id).is_some_and(|v| v.alive()))
            .filter_map(|&id| {
                entities.get(id).map(|e| {
                    let (x, y) = (
                        (e.x16 + i64::from(e.width) * 8).div_euclid(16),
                        (e.y16 + i64::from(e.height) * 8).div_euclid(16),
                    );
                    (id, x, y)
                })
            })
            .collect();
        if players.is_empty() {
            return;
        }
        // (1) 削除: どのプレイヤーからも遠い敵、またはチャンクの外の敵。
        let far = i64::from(spawner.despawn);
        let gone: Vec<EntityId> = self
            .enemies
            .keys()
            .copied()
            .filter(|id| {
                let Some(e) = entities.get(*id) else { return true };
                let (x, y) = (
                    (e.x16 + i64::from(e.width) * 8).div_euclid(16),
                    (e.y16 + i64::from(e.height) * 8).div_euclid(16),
                );
                let near_someone = players
                    .iter()
                    .any(|&(_, px, py)| (px - x).abs() <= far && (py - y).abs() <= far);
                let in_loaded = i32::try_from(x)
                    .ok()
                    .zip(i32::try_from(y).ok())
                    .is_some_and(|(cx, cy)| world.chunk(CellPos::new(cx, cy).chunk()).is_some());
                !(near_someone && in_loaded)
            })
            .collect();
        for id in gone {
            self.remove_enemy(entities, id);
        }
        // (2) スポーン。
        if spawner.interval == 0 || !tick.is_multiple_of(spawner.interval) {
            return;
        }
        let normal = self.enemies.values().filter(|e| !e.def.boss).count();
        let boss_alive = self.enemies.values().any(|e| e.def.boss);
        let (who, px, py) = {
            let pick = hash(seed, tick, 0, 0, SALT_SPAWN_WHO) as usize % players.len();
            let p = players[pick];
            (p.0, p.1, p.2)
        };
        let _ = who;
        let span = (spawner.far - spawner.near).max(1) as u32;
        let side: i64 = if hash(seed, tick, 0, 0, SALT_SPAWN_SIDE) & 1 == 0 { -1 } else { 1 };
        let dist =
            i64::from(spawner.near) + i64::from(hash(seed, tick, 0, 0, SALT_SPAWN_DIST) % span);
        let Ok(x) = i32::try_from(px + side * dist) else { return };
        let Some(def_probe) = defs.enemy("walker") else { return };
        let (w, h) = (def_probe.width, def_probe.height);
        // 地面を探す: 足元に固体があり、体の入る空きのある、いちばん上の場所。
        let Ok(py) = i32::try_from(py) else { return };
        let ground = (py - 40..py + 60).find(|&y| {
            loaded(world, x, y, w, h)
                && !collides(i64::from(x) * 16, i64::from(y) * 16, w, h, world)
                && collides(i64::from(x) * 16, i64::from(y) * 16 + 16, w, h, world)
        });
        let Some(y) = ground else { return };
        let boss_gate = !boss_alive && hash(seed, tick, 0, 0, SALT_BOSS).is_multiple_of(12);
        let candidates: Vec<&SpawnRule> = spawner
            .rules
            .iter()
            .filter(|r| (r.min_y..r.max_y).contains(&y) && r.weight > 0)
            .filter(|r| if r.boss { boss_gate } else { normal < spawner.max_enemies })
            .collect();
        let total: u32 = candidates.iter().map(|r| r.weight).sum();
        if total == 0 {
            return;
        }
        let mut roll = hash(seed, tick, 0, 0, SALT_SPAWN_PICK) % total;
        let mut chosen = None;
        for rule in &candidates {
            if roll < rule.weight {
                chosen = Some(*rule);
                break;
            }
            roll -= rule.weight;
        }
        // 出す気になるまでの門 (毎回は出さない)。
        if hash(seed, tick, 1, 0, SALT_SPAWN_GATE).is_multiple_of(3) {
            return;
        }
        if let Some(rule) = chosen {
            // 失敗 (名前がない・場所が埋まっている) は、何も変えず次の機会を待つ。
            let _ = self.spawn_enemy(defs, entities, world, &rule.enemy, (x, y));
        }
    }

    /// 敵を、倒さずに消す (遠い・チャンクの外)。戦利品も出ない。
    pub(crate) fn remove_enemy(&mut self, entities: &mut EntityWorld, id: EntityId) {
        self.enemies.remove(&id);
        self.vitals.remove(id);
        self.equipment.remove(&id);
        self.weapon_cooldown.remove(&id);
        self.statuses.clear(id);
        entities.remove(id);
    }
}
