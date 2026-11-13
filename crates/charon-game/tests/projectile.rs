#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::projectile::{
    MAX_PROJECTILES, Payload, ProjectileError, ProjectileWorld, explosion_damage,
};
use charon_game::vitals::{MAX_HP, Vitals, VitalsTable};
use charon_game::{EntityId, EntityWorld};
use charon_physics::PhysicsEngine;

fn scene(seed: u64) -> World {
    let mut world =
        World::new(Arc::new(MaterialTable::builtin().expect("組み込み材質を読み込む")), seed);
    world.insert_chunk(ChunkPos::new(0, 0));
    testkit::fill(&mut world, 0, 40, 63, 40, '#');
    world
}

/// 前景の空でないセルの数 (チャンク (0, 0) の中)。
fn solid_count(world: &World) -> usize {
    let mut n = 0;
    for y in 0..64 {
        for x in 0..64 {
            if world.get(CellPos::new(x, y)).material != MaterialId::EMPTY {
                n += 1;
            }
        }
    }
    n
}

struct Stage {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    vitals: VitalsTable,
    projectiles: ProjectileWorld,
}

impl Stage {
    fn new(seed: u64) -> Self {
        Self {
            world: scene(seed),
            physics: PhysicsEngine::new(),
            entities: EntityWorld::new(),
            vitals: VitalsTable::new(),
            projectiles: ProjectileWorld::new(),
        }
    }

    fn step(&mut self) -> Vec<charon_game::projectile::Hit> {
        self.projectiles.step(&mut self.world, &mut self.physics, &self.entities, &mut self.vitals)
    }

    /// 体力を持つ 8×16 のエンティティを `(x, 24)` に置く。
    fn add_target(&mut self, x: i64) -> EntityId {
        let id = self.entities.spawn_player(x * 16, 10 * 16, &self.world).unwrap();
        self.vitals.insert(id, Vitals::new(MAX_HP).unwrap());
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn shoot(
        &mut self,
        x: i64,
        y: i64,
        vx: i16,
        vy: i16,
        gravity: i16,
        life: u16,
        damage: i32,
        payload: Payload,
    ) -> Result<charon_game::projectile::ProjectileId, ProjectileError> {
        self.projectiles.spawn(&self.world, x * 16, y * 16, vx, vy, gravity, life, damage, payload)
    }
}

#[test]
fn projectile_flies_and_falls() {
    let mut finals = Vec::new();
    for seed in 1..=8 {
        let mut stage = Stage::new(seed);
        // 直進: 毎 tick 4 セル進み、高さは変わらない。
        let straight = stage.shoot(2, 20, 64, 0, 0, 100, 0, Payload::None).unwrap();
        // 放物線: 3 tick で 4 + 8 + 12 = 24 (1/16 セル) 下がる。
        let arc = stage.shoot(2, 10, 64, 0, 4, 100, 0, Payload::None).unwrap();
        for _ in 0..3 {
            assert!(stage.step().is_empty());
        }
        let p = stage.projectiles.get(straight).unwrap();
        assert_eq!((p.x16, p.y16), (2 * 16 + 3 * 64, 20 * 16), "seed={seed}");
        let p = stage.projectiles.get(arc).unwrap();
        assert_eq!((p.x16, p.y16, p.vy), (2 * 16 + 3 * 64, 10 * 16 + 24, 12), "seed={seed}");

        // 放物線の弾はやがて床に当たる。
        let mut hit = None;
        for _ in 0..200 {
            if let Some(h) = stage.step().into_iter().find(|h| h.projectile == arc) {
                hit = Some(h);
                break;
            }
        }
        let hit = hit.expect("床に当たる");
        assert_eq!(hit.cell.y, 40, "seed={seed}");
        assert!(hit.entity.is_none());
        assert!(stage.projectiles.get(arc).is_none());
        finals.push((hit.cell, stage.projectiles.state_hash()));
    }
    assert!(finals.windows(2).all(|w| w[0] == w[1]), "seed に依らない");
}

#[test]
fn projectile_explodes_on_solid() {
    for seed in 1..=8 {
        let mut stage = Stage::new(seed);
        testkit::fill(&mut stage.world, 30, 0, 40, 39, '#');
        let before = solid_count(&stage.world);
        stage.shoot(20, 25, 100, 0, 0, 100, 10, Payload::Explode { radius: 4 }).unwrap();
        let mut hits = Vec::new();
        for _ in 0..5 {
            hits.extend(stage.step());
        }
        assert_eq!(hits.len(), 1, "seed={seed}");
        assert_eq!((hits[0].cell.x, hits[0].cell.y), (30, 25), "seed={seed}");
        let particles = stage.physics.particles().len();
        assert!(particles > 0, "seed={seed}");
        let after = solid_count(&stage.world);
        assert!(after < before, "壁が崩れる seed={seed}");
        assert_eq!(after + particles, before, "セルは消えず粒子になる seed={seed}");
        assert_eq!(stage.world.get(CellPos::new(30, 25)).material, MaterialId::EMPTY);
        assert!(stage.projectiles.is_empty());
    }
}

#[test]
fn projectile_hits_entity() {
    for seed in 1..=8 {
        let mut stage = Stage::new(seed);
        let first = stage.add_target(8);
        // 同じ位置に重ねる。ID の小さい方だけが当たる。
        let second = stage.add_target(8);
        // 体力を持たないエンティティは当たらない。
        let ghost = stage.entities.spawn_player(8 * 16, 10 * 16, &stage.world).unwrap();
        assert!(first < second && second < ghost);
        stage.shoot(2, 30, 64, 0, 0, 100, 7, Payload::None).unwrap();
        let mut hits = Vec::new();
        for _ in 0..4 {
            hits.extend(stage.step());
        }
        assert_eq!(hits.len(), 1, "seed={seed}");
        assert_eq!(hits[0].entity, Some(first));
        assert_eq!(stage.vitals.get(first).unwrap().hp, MAX_HP - 7);
        assert_eq!(stage.vitals.get(second).unwrap().hp, MAX_HP);
        assert!(stage.vitals.get(ghost).is_none());
    }
}

#[test]
fn explosion_damage_falls_off() {
    assert_eq!(explosion_damage(4, 0, 100), 100);
    assert_eq!(explosion_damage(4, 4, 100), 75);
    assert_eq!(explosion_damage(4, 15, 100), 6);
    assert_eq!(explosion_damage(4, 16, 100), 0);
    assert_eq!(explosion_damage(4, 99, 100), 0);
    assert_eq!(explosion_damage(4, 0, 0), 0);
    assert_eq!(explosion_damage(4, 0, -5), 0);
    assert_eq!(explosion_damage(16, 0, i32::MAX), i32::MAX);

    let mut stage = Stage::new(1);
    // 壁の向こうにいても、爆発の中心との距離だけで決まる。
    testkit::fill(&mut stage.world, 30, 0, 40, 39, '#');
    // 箱の右端のセルが x = 29 になる位置 (プレイヤーの幅に合わせる)。
    let near = stage.add_target(30 - i64::from(charon_game::PLAYER_WIDTH));
    let far = stage.add_target(48);
    stage.shoot(8, 20, 100, 0, 0, 100, 50, Payload::Explode { radius: 8 }).unwrap();
    for _ in 0..5 {
        stage.step();
    }
    // 爆発の中心は (30, 20)。近い方は x の距離 1、y の距離 4。
    assert!(stage.vitals.get(near).unwrap().hp < MAX_HP, "近い方はダメージを受ける");
    let _ = explosion_damage;
    assert_eq!(stage.vitals.get(far).unwrap().hp, MAX_HP);
}

#[test]
fn projectile_expires_and_deposits() {
    let mut stage = Stage::new(1);
    let before = solid_count(&stage.world);
    let fuse = stage.shoot(2, 20, 16, 0, 0, 2, 5, Payload::Explode { radius: 4 }).unwrap();
    stage.step();
    assert!(stage.projectiles.get(fuse).is_some());
    assert!(stage.step().is_empty());
    assert!(stage.projectiles.get(fuse).is_none(), "寿命が尽きたら payload なしで消える");
    assert_eq!(solid_count(&stage.world), before);
    assert!(stage.physics.particles().is_empty());

    // 床に当たる直前の空きセルに 1 個だけ置く。
    let sand = testkit::material_of(&stage.world, 's');
    stage.shoot(20, 30, 0, 64, 0, 100, 0, Payload::Deposit { material: sand }).unwrap();
    for _ in 0..4 {
        stage.step();
    }
    assert_eq!(stage.world.get(CellPos::new(20, 39)).material, sand);
    assert_eq!(solid_count(&stage.world), before + 1);

    // 出発点が塞がっていれば何も置かない。
    stage.shoot(30, 40, 0, 64, 0, 100, 0, Payload::Deposit { material: sand }).unwrap();
    let hits = stage.step();
    assert_eq!(hits.len(), 1);
    assert_eq!(solid_count(&stage.world), before + 1);
    assert!(stage.projectiles.is_empty());
}

#[test]
fn projectile_validation_and_limit() {
    let mut stage = Stage::new(1);
    let hash = stage.projectiles.state_hash();
    let sand = testkit::material_of(&stage.world, 's');
    let none = Payload::None;
    assert_eq!(stage.shoot(1, 1, 0, 0, 0, 0, 0, none), Err(ProjectileError::InvalidLife));
    assert_eq!(stage.shoot(1, 1, 0, 0, 0, 3601, 0, none), Err(ProjectileError::InvalidLife));
    assert_eq!(stage.shoot(1, 1, 513, 0, 0, 10, 0, none), Err(ProjectileError::InvalidSpeed));
    assert_eq!(stage.shoot(1, 1, 0, -513, 0, 10, 0, none), Err(ProjectileError::InvalidSpeed));
    assert_eq!(
        stage.shoot(1, 1, 0, 0, 0, 10, 0, Payload::Explode { radius: 0 }),
        Err(ProjectileError::InvalidRadius)
    );
    assert_eq!(
        stage.shoot(1, 1, 0, 0, 0, 10, 0, Payload::Explode { radius: 17 }),
        Err(ProjectileError::InvalidRadius)
    );
    assert_eq!(
        stage.shoot(1, 1, 0, 0, 0, 10, 0, Payload::Deposit { material: MaterialId(60_000) }),
        Err(ProjectileError::UnknownMaterial)
    );
    assert_eq!(stage.projectiles.state_hash(), hash, "失敗しても状態は変わらない");
    assert!(stage.projectiles.is_empty());

    // 失敗した生成は ID を消費しない。
    let first = stage.shoot(1, 1, 0, 0, 0, 10, 0, Payload::Deposit { material: sand }).unwrap();
    assert_eq!(first.0, 1);
    for _ in 1..MAX_PROJECTILES {
        stage.shoot(1, 1, 0, 0, 0, 10, 0, none).unwrap();
    }
    let full = stage.projectiles.state_hash();
    assert_eq!(stage.shoot(1, 1, 0, 0, 0, 10, 0, none), Err(ProjectileError::TooMany));
    assert_eq!(stage.projectiles.state_hash(), full);
    assert_eq!(stage.projectiles.len(), MAX_PROJECTILES);
}
