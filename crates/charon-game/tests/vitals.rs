#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::coords::ChunkPos;
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::vitals::{
    BREATH_MAX, DROWN_INTERVAL, MAX_HP, Vitals, VitalsError, VitalsTable, step_environment,
};
use charon_game::{EntityId, EntityWorld};

fn scene(seed: u64) -> World {
    let mut world =
        World::new(Arc::new(MaterialTable::builtin().expect("組み込み材質を読み込む")), seed);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 40, 63, 40, '#');
    world
}

/// 8×16 のエンティティを `(8, top)` に置く。
fn spawn(world: &World, top: i32) -> (EntityWorld, EntityId) {
    let mut entities = EntityWorld::new();
    let id = entities.spawn_player(8 * 16, i64::from(top) * 16, world).unwrap();
    (entities, id)
}

#[test]
fn hot_cells_damage_entity() {
    for seed in 1..=8 {
        let mut world = scene(seed);
        testkit::fill(&mut world, 0, 30, 63, 39, 'l');
        let (entities, id) = spawn(&world, 16);
        let mut hot = Vitals::new(MAX_HP).unwrap();
        // 足元だけが溶岩に触れる。頭は空気なので溺れない。
        step_environment(entities.get(id).unwrap(), &mut hot, &world);
        assert!(hot.hp < MAX_HP, "seed={seed}");
        let first = hot.hp;
        step_environment(entities.get(id).unwrap(), &mut hot, &world);
        assert_eq!(hot.hp, first - (MAX_HP - first), "毎 tick 同じだけ減る seed={seed}");

        // 溶岩のない場所では減らない。
        let (entities, id) = spawn(&scene(seed), 16);
        let mut safe = Vitals::new(MAX_HP).unwrap();
        for _ in 0..10 {
            step_environment(entities.get(id).unwrap(), &mut safe, &scene(seed));
        }
        assert_eq!(safe.hp, MAX_HP, "seed={seed}");
    }
}

#[test]
fn drowning_needs_submerged_head() {
    for seed in 1..=8 {
        // 足だけ水に浸かる (頭の行は空気)。
        let mut world = scene(seed);
        testkit::fill(&mut world, 0, 33, 63, 39, 'w');
        let (entities, id) = spawn(&world, 16);
        let mut feet = Vitals::new(MAX_HP).unwrap();
        for _ in 0..(BREATH_MAX as usize + 100) {
            step_environment(entities.get(id).unwrap(), &mut feet, &world);
        }
        assert_eq!((feet.breath, feet.hp), (BREATH_MAX, MAX_HP), "seed={seed}");

        // 頭まで水没する。
        let mut world = scene(seed);
        testkit::fill(&mut world, 0, 10, 63, 39, 'w');
        let (entities, id) = spawn(&world, 16);
        let mut deep = Vitals::new(MAX_HP).unwrap();
        let entity = entities.get(id).unwrap();
        for _ in 0..(BREATH_MAX - 1) {
            step_environment(entity, &mut deep, &world);
        }
        assert_eq!((deep.breath, deep.hp), (1, MAX_HP), "seed={seed}");
        // 息が尽きた tick から数えて DROWN_INTERVAL 回目に最初のダメージ。
        for _ in 0..(DROWN_INTERVAL - 1) {
            step_environment(entity, &mut deep, &world);
        }
        assert_eq!((deep.breath, deep.hp), (0, MAX_HP), "seed={seed}");
        step_environment(entity, &mut deep, &world);
        assert_eq!(deep.hp, MAX_HP - 1, "seed={seed}");
        for _ in 0..DROWN_INTERVAL {
            step_environment(entity, &mut deep, &world);
        }
        assert_eq!(deep.hp, MAX_HP - 2, "seed={seed}");
    }
}

#[test]
fn breath_recovers_in_air() {
    let world = scene(1);
    let (entities, id) = spawn(&world, 16);
    let mut vitals = Vitals::new(MAX_HP).unwrap();
    vitals.breath = 10;
    let entity = entities.get(id).unwrap();
    step_environment(entity, &mut vitals, &world);
    assert_eq!(vitals.breath, 14);
    for _ in 0..1000 {
        step_environment(entity, &mut vitals, &world);
    }
    assert_eq!(vitals.breath, BREATH_MAX);

    // 溺れかけてから空気に出ると、ダメージの周期が最初からになる。
    let mut deep = scene(1);
    testkit::fill(&mut deep, 0, 10, 63, 39, 'w');
    let mut v = Vitals::new(MAX_HP).unwrap();
    v.breath = 0;
    for _ in 0..(DROWN_INTERVAL - 1) {
        step_environment(entities.get(id).unwrap(), &mut v, &deep);
    }
    step_environment(entity, &mut v, &world); // 空気
    for _ in 0..(DROWN_INTERVAL - 1) {
        step_environment(entities.get(id).unwrap(), &mut v, &deep);
    }
    assert_eq!(v.hp, MAX_HP, "周期が 0 に戻っているので、まだダメージはない");
}

#[test]
fn damage_and_heal_saturate() {
    assert_eq!(Vitals::new(0).unwrap_err(), VitalsError::InvalidMaxHp);
    assert_eq!(Vitals::new(10_001).unwrap_err(), VitalsError::InvalidMaxHp);
    let mut v = Vitals::new(50).unwrap();
    v.damage(0);
    v.damage(-5);
    assert_eq!(v.hp, 50);
    v.damage(30);
    v.heal(-9);
    assert_eq!(v.hp, 20);
    v.heal(1_000);
    assert_eq!(v.hp, 50);
    v.damage(i32::MAX);
    assert_eq!(v.hp, 0);
    assert!(!v.alive());
    // 死んだエンティティは環境の影響を受けない。
    let mut world = scene(1);
    testkit::fill(&mut world, 0, 10, 63, 39, 'l');
    let (entities, id) = spawn(&world, 16);
    step_environment(entities.get(id).unwrap(), &mut v, &world);
    assert_eq!(v.hp, 0);
}

#[test]
fn vitals_hash_is_stable() {
    let build = |hp_loss: i32, breath: u16| {
        let mut table = VitalsTable::new();
        let mut a = Vitals::new(100).unwrap();
        a.damage(hp_loss);
        a.breath = breath;
        table.insert(EntityId(2), a);
        table.insert(EntityId(1), Vitals::new(80).unwrap());
        table
    };
    let base = build(10, 500);
    assert_eq!(base.state_hash(), build(10, 500).state_hash());
    assert_ne!(base.state_hash(), build(11, 500).state_hash());
    assert_ne!(base.state_hash(), build(10, 499).state_hash());
    let ids: Vec<u64> = base.iter().map(|(id, _)| id.0).collect();
    assert_eq!(ids, vec![1, 2], "ID 昇順");
    let mut removed = build(10, 500);
    assert!(removed.remove(EntityId(2)).is_some());
    assert_ne!(base.state_hash(), removed.state_hash());
}
