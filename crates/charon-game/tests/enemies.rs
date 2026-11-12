#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::collections::BTreeSet;
use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::rng;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::{DefinitionFiles, Definitions};
use charon_game::game::{
    CONTACT_INTERVAL, GameError, GameInput, GameState, SALT_WANDER, StepReport,
};
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{
    Entity, EntityError, EntityId, EntityWorld, PlayerInput, step_entity, step_player,
};
use charon_physics::PhysicsEngine;

const TEST_ENEMIES: &str = r#"[
    (name: "chaser", width: 8, height: 16, max_hp: 30, walk_speed: 16, jump_speed: 40,
     sight: 60, attack: None, contact_damage: 5),
    (name: "gunner", width: 8, height: 16, max_hp: 30, walk_speed: 16, jump_speed: 40,
     sight: 60, attack: Some("spark"), attack_interval: 20),
    (name: "blind", width: 8, height: 16, max_hp: 30, walk_speed: 16, jump_speed: 40,
     sight: 20, attack: None),
]"#;

struct Arena {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    game: GameState,
    defs: Definitions,
    tick: u64,
}

impl Arena {
    /// 幅 64・高さ 41 の床のある世界。床は y = 40。
    fn new(seed: u64) -> Self {
        let materials = MaterialTable::builtin().expect("組み込み材質を読み込む");
        let mut defs = Definitions::engine_defaults(&materials).unwrap();
        defs.layer(
            DefinitionFiles { enemies: Some(TEST_ENEMIES), ..Default::default() },
            &materials,
            None,
        )
        .unwrap();
        let mut world = World::new(Arc::new(materials), seed);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 40, 63, 40, '#');
        let game = GameState::from_definitions(&defs);
        Self {
            world,
            physics: PhysicsEngine::new(),
            entities: EntityWorld::new(),
            game,
            defs,
            tick: 0,
        }
    }

    /// 体力 100・杖 (`basic`) を持つプレイヤー (8×16) を `(x, 24)` に置く。
    fn add_player(&mut self, x: i64) -> EntityId {
        let id = self.entities.spawn_player(x * 16, 24 * 16, &self.world).unwrap();
        self.game.add_character(id, Vitals::new(MAX_HP).unwrap(), self.defs.wand("basic").unwrap());
        id
    }

    fn spawn(&mut self, name: &str, x: i32) -> EntityId {
        self.game.spawn_enemy(&self.defs, &mut self.entities, &self.world, name, (x, 24)).unwrap()
    }

    fn step_with(&mut self, actions: &[(EntityId, GameInput)]) -> StepReport {
        let seed = self.world.seed();
        let report = self
            .game
            .step(&mut self.world, &mut self.physics, &mut self.entities, actions, self.tick, seed)
            .unwrap();
        self.tick += 1;
        report
    }

    fn step(&mut self) -> StepReport {
        self.step_with(&[])
    }

    fn entity(&self, id: EntityId) -> &Entity {
        self.entities.get(id).unwrap()
    }

    fn hp(&self, id: EntityId) -> i32 {
        self.game.vitals.get(id).unwrap().hp
    }
}

#[test]
fn enemy_spawns_from_definition() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let first = arena.spawn("walker", 30);
        let second = arena.spawn("shooter", 40);
        assert!(first < second, "ID は単調増加");
        let e = arena.entity(first);
        assert_eq!((e.x16, e.y16, e.width, e.height), (30 * 16, 24 * 16, 8, 16));
        assert_eq!(arena.hp(first), 30);
        assert_eq!(arena.game.enemies.len(), 2);
        assert_eq!(arena.game.enemies[&second].def.attack.as_deref(), Some("spark"));

        // 失敗は何も変えず、ID も消費しない。
        let before = (arena.game.state_hash(), arena.entities.state_hash());
        let mut fail = |name: &str, y: i32| {
            let error = arena
                .game
                .spawn_enemy(&arena.defs, &mut arena.entities, &arena.world, name, (10, y))
                .unwrap_err();
            assert_eq!((arena.game.state_hash(), arena.entities.state_hash()), before);
            error
        };
        assert_eq!(fail("nothing", 24), GameError::UnknownEnemy("nothing".to_owned()));
        // 床 (y = 40) と重なる。
        assert_eq!(fail("walker", 30), GameError::Entity(EntityError::Occupied));
        let third = arena.spawn("walker", 50);
        assert_eq!(third.0, second.0 + 1, "失敗した生成で ID を消費しない seed={seed}");
    }

    // 敵の攻撃が、ゲームの呪文表にない呪文を参照している。
    let mut arena = Arena::new(1);
    let materials = arena.world.materials().as_ref().clone();
    let mut other = arena.defs.clone();
    other
        .layer(
            DefinitionFiles {
                spells: Some(
                    r#"[(name: "extra", mana_cost: 1, cast_delay: 1, speed: 10, gravity: 0,
                         life: 10, damage: 1, spread: 0, payload: None)]"#,
                ),
                enemies: Some(
                    r#"[(name: "caster", width: 8, height: 16, max_hp: 5, walk_speed: 1,
                         jump_speed: 1, sight: 10, attack: Some("extra"), attack_interval: 5)]"#,
                ),
                ..Default::default()
            },
            &materials,
            None,
        )
        .unwrap();
    let error = arena
        .game
        .spawn_enemy(&other, &mut arena.entities, &arena.world, "caster", (10, 24))
        .unwrap_err();
    assert_eq!(error, GameError::UnknownSpell("extra".to_owned()));
    assert!(arena.game.enemies.is_empty() && arena.entities.iter().count() == 0);
}

#[test]
fn enemy_chases_target_in_sight() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let player = arena.add_player(8);
        let chaser = arena.spawn("chaser", 50);
        let start = arena.entity(chaser).x16;
        for _ in 0..200 {
            arena.step();
        }
        let (p, c) = (arena.entity(player).x16, arena.entity(chaser).x16);
        assert!(c < start, "近づく seed={seed}");
        assert!((c - p).abs() <= 8 * 16, "接触するまで近づく seed={seed}: {p} {c}");
        assert!(arena.hp(player) < MAX_HP, "接触ダメージを受ける seed={seed}");
    }

    // 視界 (20 セル) の外にいる敵は、目標に向かわず、徘徊の向きに動く。
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let _player = arena.add_player(8);
        let blind = arena.spawn("blind", 40);
        arena.step();
        let expected = rng::hash(seed, 0, blind.0 as i32, 0, SALT_WANDER) % 3;
        let horizontal = expected as i16 - 1;
        assert_eq!(arena.entity(blind).vx, horizontal * 16, "seed={seed}");
    }
}

#[test]
fn enemy_needs_line_of_sight() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let player = arena.add_player(8);
        let gunner = arena.spawn("gunner", 50);
        testkit::fill(&mut arena.world, 30, 0, 30, 39, '#');
        for _ in 0..150 {
            let report = arena.step();
            assert!(report.hits.is_empty() && arena.game.projectiles.is_empty(), "seed={seed}");
        }
        assert_eq!(arena.hp(player), MAX_HP);
        assert_eq!(arena.hp(gunner), 30);

        testkit::fill(&mut arena.world, 30, 0, 30, 39, '.');
        let mut fired = false;
        for _ in 0..60 {
            arena.step();
            fired |= !arena.game.projectiles.is_empty();
        }
        assert!(fired, "壁を外すと撃つ seed={seed}");
    }
}

#[test]
fn enemy_jumps_over_step() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let player = arena.add_player(8);
        let chaser = arena.spawn("chaser", 50);
        // 高さ 4 セルの段差。
        testkit::fill(&mut arena.world, 30, 36, 31, 39, '#');
        let mut crossed = false;
        for _ in 0..300 {
            arena.step();
            crossed |= arena.entity(chaser).x16 < 30 * 16;
        }
        assert!(crossed, "段差を越える seed={seed}");
        assert!(arena.hp(player) < MAX_HP, "越えて目標に届く seed={seed}");
    }
}

#[test]
fn shooter_fires_and_hits_target() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let player = arena.add_player(8);
        let gunner = arena.spawn("gunner", 50);
        let mut on_player = 0;
        for _ in 0..130 {
            let report = arena.step();
            on_player += report.hits.iter().filter(|h| h.entity == Some(player)).count();
            assert!(
                report.hits.iter().all(|h| h.entity != Some(gunner)),
                "撃った敵自身には当たらない seed={seed}"
            );
        }
        // 20 tick ごとに撃つので、130 tick で 7 発。到達までの時間を見て 5 発以上が当たる。
        assert!((5..=7).contains(&on_player), "seed={seed}: {on_player} 発");
        assert_eq!(arena.hp(player), MAX_HP - 8 * on_player as i32, "seed={seed}");
        assert_eq!(arena.hp(gunner), 30);
    }
}

#[test]
fn enemy_contact_damage_repeats() {
    for seed in 1..=8 {
        let mut arena = Arena::new(seed);
        let player = arena.add_player(8);
        let _chaser = arena.spawn("chaser", 12);
        let mut series = Vec::new();
        for _ in 0..100 {
            arena.step();
            series.push(arena.hp(player));
        }
        // tick 0、30、60、90 に 5 ずつ入る。
        let drops: Vec<usize> = (1..series.len()).filter(|&i| series[i] < series[i - 1]).collect();
        assert_eq!(series[0], MAX_HP - 5, "seed={seed}");
        assert_eq!(drops, vec![30, 60, 90], "seed={seed}");
        assert_eq!(series[99], MAX_HP - 20);
        assert_eq!(CONTACT_INTERVAL, 30);
    }
}

#[test]
fn defeated_enemies_are_removed() {
    let mut arena = Arena::new(1);
    let player = arena.add_player(8);
    let first = arena.spawn("chaser", 40);
    let second = arena.spawn("chaser", 50);
    arena.game.vitals.get_mut(first).unwrap().damage(1_000);
    let report = arena.step();
    assert_eq!(report.defeated, vec![first]);
    assert!(arena.entities.get(first).is_none());
    assert!(arena.game.vitals.get(first).is_none());
    assert!(!arena.game.enemies.contains_key(&first));
    assert!(arena.entities.get(second).is_some() && arena.game.enemies.contains_key(&second));

    // ID は再利用しない。
    let third = arena.spawn("walker", 20);
    assert!(third > second);

    // 倒れたプレイヤーは撤去されない。
    arena.game.vitals.get_mut(player).unwrap().damage(1_000);
    let report = arena.step();
    assert!(report.defeated.is_empty());
    assert!(arena.entities.get(player).is_some());
    assert_eq!(arena.hp(player), 0);
    // 倒れたプレイヤーは目標にならない: 敵は徘徊に戻る。
    let before: Vec<_> = arena.entities.iter().map(|e| (e.id, e.x16)).collect();
    for _ in 0..90 {
        arena.step();
    }
    let after: Vec<_> = arena.entities.iter().map(|e| (e.id, e.x16)).collect();
    assert_ne!(before, after, "敵は目標を失っても動き続ける (徘徊)");
    assert_eq!(arena.hp(player), 0, "倒れたプレイヤーへの接触ダメージは入らない");
}

fn trace(seed: u64) -> Vec<(u64, u64)> {
    let mut arena = Arena::new(seed);
    let player = arena.add_player(8);
    arena.spawn("chaser", 50);
    arena.spawn("gunner", 56);
    arena.spawn("blind", 36);
    let mut hashes = Vec::new();
    for tick in 0..250 {
        let fire = tick % 7 < 3;
        let action = GameInput { fire, aim: (1, 0), ..Default::default() };
        let input =
            PlayerInput { horizontal: if tick % 90 < 45 { 1 } else { -1 }, jump: tick % 53 == 0 };
        arena.entities.step_players(&[(player, input)], &arena.world).unwrap();
        arena.step_with(&[(player, action)]);
        hashes.push((arena.game.state_hash(), arena.entities.state_hash()));
    }
    hashes
}

#[test]
fn enemy_ai_is_deterministic() {
    for seed in 1..=8 {
        let (first, second) = (trace(seed), trace(seed));
        assert_eq!(first, second, "seed={seed}");
        let distinct: BTreeSet<_> = first.iter().collect();
        assert!(distinct.len() > 100, "状態が動いている seed={seed}: {}", distinct.len());
    }
    assert_ne!(trace(1), trace(2), "seed が違えば徘徊が変わる");
}

#[test]
fn step_entity_uses_given_speeds() {
    let materials = MaterialTable::builtin().unwrap();
    let mut world = World::new(Arc::new(materials), 1);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 40, 63, 40, '#');
    let spawn = || {
        let mut entities = EntityWorld::new();
        let id = entities.spawn_player(8 * 16, 24 * 16, &world).unwrap();
        (entities, id)
    };
    // 速さは引数どおり。範囲外は 96 に収める。
    let (mut entities, id) = spawn();
    let input = PlayerInput { horizontal: 1, jump: false };
    for _ in 0..3 {
        let entity = entities.get_mut(id).unwrap();
        step_entity(entity, input, 10, 40, &world);
    }
    assert_eq!(entities.get(id).unwrap().x16, 8 * 16 + 30);
    step_entity(entities.get_mut(id).unwrap(), input, 500, 40, &world);
    assert_eq!(entities.get(id).unwrap().vx, 96);
    // 接地したあとのジャンプの初速。
    for _ in 0..40 {
        step_entity(entities.get_mut(id).unwrap(), PlayerInput::default(), 10, 40, &world);
    }
    assert!(entities.get(id).unwrap().grounded);
    step_entity(
        entities.get_mut(id).unwrap(),
        PlayerInput { horizontal: 0, jump: true },
        10,
        40,
        &world,
    );
    assert_eq!(entities.get(id).unwrap().vy, -40 + 4);

    // step_player は (32, 96) の step_entity と同じ。
    let (mut a, ida) = spawn();
    let (mut b, idb) = spawn();
    for tick in 0..120 {
        let input = PlayerInput { horizontal: [1, 0, -1][tick % 3], jump: tick % 40 == 0 };
        step_player(a.get_mut(ida).unwrap(), input, &world);
        step_entity(b.get_mut(idb).unwrap(), input, 32, 96, &world);
        assert_eq!(a.get(ida), b.get(idb), "tick {tick}");
    }
}
