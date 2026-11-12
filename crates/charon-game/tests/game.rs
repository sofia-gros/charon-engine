#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::game::{GameError, GameInput, GameState};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::wand::{SpellId, SpellTable, Wand};
use charon_game::{EntityId, EntityWorld};
use charon_physics::PhysicsEngine;

const SPELLS: &str = r#"[
    (name: "spark", mana_cost: 5, cast_delay: 6, speed: 128, gravity: 0, life: 60,
     damage: 10, spread: 0, payload: None),
    (name: "slow", mana_cost: 5, cast_delay: 6, speed: 1, gravity: 0, life: 60,
     damage: 10, spread: 0, payload: None),
]"#;

struct Stage {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    game: GameState,
    ids: Vec<EntityId>,
}

impl Stage {
    /// 床のある世界に、8 セル間隔 (x = 8, 40) の 2 人を置く。杖の呪文は `spell`。
    fn new(seed: u64, spell: u16) -> Self {
        let materials = MaterialTable::builtin().expect("組み込み材質を読み込む");
        let table = SpellTable::parse(SPELLS, &materials, None).unwrap();
        let mut world = World::new(Arc::new(materials), seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 40, 63, 40, '#');
        let mut entities = EntityWorld::new();
        let mut game = GameState::new(table.clone());
        let mut ids = Vec::new();
        for x in [8, 40] {
            let id = entities.spawn_player(x * 16, 24 * 16, &world).unwrap();
            let wand = Wand::new(vec![SpellId(spell)], 100, 1, 0, &table).unwrap();
            game.add_character(id, Vitals::new(MAX_HP).unwrap(), wand);
            ids.push(id);
        }
        Self { world, physics: PhysicsEngine::new(), entities, game, ids }
    }

    fn step(
        &mut self,
        actions: &[(EntityId, GameInput)],
    ) -> Result<Vec<charon_game::projectile::Hit>, GameError> {
        let (tick, seed) = (self.world.tick(), self.world.seed());
        self.game
            .step(&mut self.world, &mut self.physics, &mut self.entities, actions, tick, seed)
            .map(|report| report.hits)
    }
}

fn shoot_right() -> GameInput {
    GameInput { fire: true, aim: (1, 0), ..Default::default() }
}

#[test]
fn game_step_order_is_fixed() {
    let mut traces = Vec::new();
    for seed in 1..=8 {
        let mut trace = Vec::new();
        for _ in 0..2 {
            let mut stage = Stage::new(seed, 0);
            let (a, b) = (stage.ids[0], stage.ids[1]);
            let mut hashes = Vec::new();
            for tick in 0..40 {
                let actions = [(a, shoot_right()), (b, GameInput::default())];
                stage.step(&actions).unwrap();
                if tick == 0 {
                    // 発射した tick のうちに弾が 1 歩進む (発射 → 移動の順)。
                    let p = stage.game.projectiles.iter().next().unwrap();
                    assert_eq!(
                        p.x16,
                        (8 * 16 + i64::from(charon_game::PLAYER_WIDTH) * 8) + 128,
                        "seed={seed}"
                    );
                }
                hashes.push(stage.game.state_hash());
            }
            trace.push(hashes);
        }
        assert_eq!(trace[0], trace[1], "seed={seed}");
        traces.push(trace.remove(0));
    }
    assert!(traces.iter().all(|t| t.len() == 40));

    // 環境ダメージ → 発射の順: 同じ tick に死んだキャラクターは撃てない。
    let mut stage = Stage::new(1, 0);
    let a = stage.ids[0];
    testkit::fill(&mut stage.world, 0, 30, 63, 39, 'l');
    stage.game.vitals.get_mut(a).unwrap().hp = 1;
    stage.step(&[(a, shoot_right())]).unwrap();
    assert!(!stage.game.vitals.get(a).unwrap().alive());
    assert!(stage.game.projectiles.is_empty(), "死んだ tick には撃たない");
}

#[test]
fn game_rejects_bad_actions_atomically() {
    let mut stage = Stage::new(1, 0);
    let a = stage.ids[0];
    // 溶岩で体力が減る状況でも、入力の検査に失敗したら何も更新しない。
    testkit::fill(&mut stage.world, 0, 30, 63, 39, 'l');
    let before = stage.game.state_hash();
    let ghost = EntityId(99);
    assert_eq!(stage.step(&[(ghost, shoot_right())]).unwrap_err(), GameError::InvalidAction);
    assert_eq!(
        stage.step(&[(a, shoot_right()), (a, GameInput::default())]).unwrap_err(),
        GameError::InvalidAction
    );
    assert_eq!(stage.game.state_hash(), before);

    // 狙いが (0, 0) なら撃たずに続ける (環境ダメージは進む)。
    stage.step(&[(a, GameInput { fire: true, aim: (0, 0), ..Default::default() })]).unwrap();
    assert!(stage.game.projectiles.is_empty());
    assert!(stage.game.vitals.get(a).unwrap().hp < MAX_HP);
    assert_eq!(stage.game.wands[&a].mana, 100, "撃っていないのでマナは減らない");
}

#[test]
fn shooter_is_not_hit_by_own_shot() {
    let mut stage = Stage::new(1, 1);
    let a = stage.ids[0];
    // 速度 1 (1/16 セル / tick) の弾は、発射後もしばらく自分の中にいる。
    let hits = stage.step(&[(a, shoot_right())]).unwrap();
    assert!(hits.is_empty());
    assert_eq!(stage.game.vitals.get(a).unwrap().hp, MAX_HP, "発射した tick は当たらない");
    let hits = stage.step(&[]).unwrap();
    assert_eq!(hits.len(), 1, "次の tick からは自分にも当たる");
    assert_eq!(hits[0].entity, Some(a));
    assert_eq!(stage.game.vitals.get(a).unwrap().hp, MAX_HP - 10);
}

#[test]
fn dead_characters_do_not_fire() {
    let mut stage = Stage::new(1, 0);
    let a = stage.ids[0];
    stage.game.vitals.get_mut(a).unwrap().damage(1_000);
    testkit::fill(&mut stage.world, 0, 10, 63, 39, 'l');
    for _ in 0..20 {
        stage.step(&[(a, shoot_right())]).unwrap();
    }
    assert!(stage.game.projectiles.is_empty());
    assert_eq!(stage.game.wands[&a].mana, 100);
    let vitals = stage.game.vitals.get(a).unwrap();
    assert_eq!((vitals.hp, vitals.breath), (0, 600), "死んだあとは環境の影響を受けない");
}
