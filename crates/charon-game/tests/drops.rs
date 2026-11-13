//! 仕様 10 §19: 落ちているもの (ドロップ) と拾うこと。

#![allow(clippy::unwrap_used)] // テストの前提が崩れたら直ちに失敗させる。

use std::sync::Arc;

use charon_core::cell::MaterialId;
use charon_core::coords::{CellPos, ChunkPos};
use charon_core::material::MaterialTable;
use charon_core::testkit;
use charon_core::world::World;
use charon_game::defs::Definitions;
use charon_game::drops::{LIFETIME, MAX_DROPS, PICKUP_DELAY};
use charon_game::game::{GameEvent, GameInput, GameState};
use charon_game::inventory::STACK_MAX;
use charon_game::vitals::{MAX_HP, Vitals};
use charon_game::{EntityId, EntityWorld};
use charon_physics::PhysicsEngine;

struct Arena {
    world: World,
    physics: PhysicsEngine,
    entities: EntityWorld,
    game: GameState,
    player: EntityId,
    stone: MaterialId,
    tick: u64,
}

impl Arena {
    /// 床 (y = 60) の上、x = 10 にプレイヤーが立つ。持ち物つき。
    fn new() -> Self {
        let materials = MaterialTable::builtin().unwrap();
        let stone = materials.id("stone").unwrap();
        let defs = Definitions::engine_defaults(&materials).unwrap();
        let mut world = World::new(Arc::new(materials), 1);
        world.insert_chunk(ChunkPos::new(0, 0));
        testkit::fill(&mut world, 0, 60, 63, 60, '#');
        let mut entities = EntityWorld::new();
        let player = entities.spawn_player(10 * 16, 50 * 16, &world).unwrap();
        let mut game = GameState::from_definitions(&defs);
        game.add_character(player, Vitals::new(MAX_HP).unwrap(), defs.wand("basic").unwrap());
        game.give_inventory(player).unwrap();
        let mut arena =
            Self { world, physics: PhysicsEngine::new(), entities, game, player, stone, tick: 0 };
        for _ in 0..30 {
            arena.step();
        }
        arena
    }

    fn step(&mut self) -> Vec<GameEvent> {
        let seed = self.world.seed();
        self.game
            .step(
                &mut self.world,
                &mut self.physics,
                &mut self.entities,
                &[(self.player, GameInput::default())],
                self.tick,
                seed,
            )
            .unwrap();
        self.tick += 1;
        self.game.take_events()
    }

    fn have(&self) -> u32 {
        self.game.inventories[&self.player].count(self.stone)
    }

    /// セル `(x, y)` の中心に落とす。
    fn spawn(&mut self, x: i32, y: i32, count: u32) -> bool {
        self.game.drops.spawn(self.stone, count, i64::from(x) * 16 + 8, i64::from(y) * 16 + 8)
    }
}

/// 落ちて、床の上で止まる。固体に埋まったら、上へ出る。
#[test]
fn drops_fall_and_rest_on_the_floor_and_climb_out_of_solids() {
    let mut arena = Arena::new();
    assert!(arena.spawn(40, 20, 5));
    for _ in 0..120 {
        arena.step();
    }
    let drop = arena.game.drops.iter().next().unwrap();
    assert_eq!(drop.y16.div_euclid(16), 59, "床 (y = 60) の 1 つ上で止まる");
    // 床に埋まった位置から作ると、上へ出る。
    assert!(arena.spawn(50, 60, 1));
    for _ in 0..10 {
        arena.step();
    }
    assert!(arena.game.drops.iter().all(|d| d.y16.div_euclid(16) < 60));
}

/// 近づくと拾える (待ちのあと)。遠いと拾えず、拾うと持ち物に入って出来事になる。
#[test]
fn nearby_drops_are_picked_up_after_the_delay_far_ones_are_not() {
    let mut arena = Arena::new();
    arena.spawn(11, 58, 4);
    arena.spawn(50, 58, 9);
    let mut events = Vec::new();
    for _ in 0..PICKUP_DELAY / 2 {
        events.extend(arena.step());
    }
    assert_eq!(arena.have(), 0, "待ちの間は拾えない");
    for _ in 0..PICKUP_DELAY {
        events.extend(arena.step());
    }
    assert_eq!(arena.have(), 4);
    assert_eq!(arena.game.drops.len(), 1, "遠いものは残る");
    let picked: Vec<_> = events
        .iter()
        .filter_map(|e| if let GameEvent::Pickup(p) = e { Some(*p) } else { None })
        .collect();
    assert_eq!(picked.len(), 1);
    assert_eq!(
        (picked[0].who, picked[0].material, picked[0].count),
        (arena.player, arena.stone, 4)
    );
}

/// 持ち物が満杯なら、入った分だけ拾って、残りは落ちたまま。
#[test]
fn a_full_inventory_leaves_the_rest_on_the_ground() {
    let mut arena = Arena::new();
    arena.game.inventories.get_mut(&arena.player).unwrap().add(arena.stone, STACK_MAX - 2);
    arena.spawn(11, 58, 10);
    for _ in 0..PICKUP_DELAY + 5 {
        arena.step();
    }
    assert_eq!(arena.have(), STACK_MAX);
    assert_eq!(arena.game.drops.iter().next().unwrap().count, 8);
}

/// 持ち物から落とす。足りなければ何もしない。落としたものは、待ちのあとに拾い直せる。
#[test]
fn dropping_from_the_inventory_and_picking_it_up_again() {
    let mut arena = Arena::new();
    arena.game.inventories.get_mut(&arena.player).unwrap().add(arena.stone, 7);
    assert!(!arena.game.drop_item(&arena.entities, arena.player, arena.stone, 8), "足りない");
    assert!(!arena.game.drop_item(&arena.entities, arena.player, arena.stone, 0));
    assert!(arena.game.drop_item(&arena.entities, arena.player, arena.stone, 5));
    assert_eq!(arena.have(), 2);
    assert_eq!(arena.game.drops.len(), 1);
    for _ in 0..PICKUP_DELAY + 5 {
        arena.step();
    }
    assert_eq!(arena.have(), 7, "拾い直した");
    assert!(arena.game.drops.is_empty());
}

/// 倒れたキャラクターは拾えない。数・寿命・不正な材質に上限がある。
#[test]
fn dead_players_do_not_pick_up_and_limits_hold() {
    let mut arena = Arena::new();
    arena.spawn(11, 58, 3);
    arena.game.vitals.get_mut(arena.player).unwrap().damage(MAX_HP);
    for _ in 0..PICKUP_DELAY + 5 {
        arena.step();
    }
    assert_eq!(arena.have(), 0);
    assert_eq!(arena.game.drops.len(), 1);

    assert!(!arena.game.drops.spawn(MaterialId::EMPTY, 1, 0, 0));
    assert!(!arena.game.drops.spawn(MaterialId::BOUNDARY, 1, 0, 0));
    assert!(!arena.game.drops.spawn(arena.stone, 0, 0, 0));
    for _ in 0..MAX_DROPS {
        arena.game.drops.spawn(arena.stone, 1, 40 * 16, 20 * 16);
    }
    assert_eq!(arena.game.drops.len(), MAX_DROPS);
    assert!(!arena.spawn(41, 20, 1), "上限");

    // 拾われなければ、寿命で消える。
    let mut lone = Arena::new();
    lone.game.vitals.get_mut(lone.player).unwrap().damage(MAX_HP);
    lone.spawn(40, 20, 1);
    for _ in 0..=LIFETIME {
        lone.step();
    }
    assert!(lone.game.drops.is_empty());
}

/// 掘ったものを、持ち物に直接ではなくドロップにする設定。材質ごとにまとまり、拾うと持ち物に入る。
#[test]
fn dig_to_drops_makes_pickups_instead_of_direct_collection() {
    use charon_game::defs::DefinitionFiles;
    let materials = MaterialTable::builtin().unwrap();
    let mut defs = Definitions::engine_defaults(&materials).unwrap();
    defs.layer(DefinitionFiles::default(), &materials, None).unwrap();
    let mut arena = Arena::new();
    testkit::fill(&mut arena.world, 30, 40, 40, 59, '#');
    arena.game.equip_dig(arena.player, &defs, "laser").unwrap();
    arena.game.dig_to_drops = true;
    let mut dug = 0;
    for _ in 0..140 {
        let seed = arena.world.seed();
        let report = arena
            .game
            .step(
                &mut arena.world,
                &mut arena.physics,
                &mut arena.entities,
                &[(arena.player, GameInput { dig: true, aim: (1000, 0), ..Default::default() })],
                arena.tick,
                seed,
            )
            .unwrap();
        arena.tick += 1;
        dug += report.dug.len();
    }
    assert!(dug > 0);
    assert_eq!(arena.have(), 0, "直接は入らない");
    let on_ground: u32 = arena.game.drops.iter().map(|d| d.count).sum();
    assert_eq!(on_ground as usize, dug, "掘った数がドロップになる");
    assert!(arena.world.get(CellPos::new(30, 55)).material != MaterialId::BOUNDARY);
}

/// 同じ手順なら、同じ結果。
#[test]
fn drops_are_deterministic() {
    let run = || {
        let mut arena = Arena::new();
        arena.spawn(14, 20, 3);
        arena.spawn(30, 10, 2);
        (0..200)
            .map(|_| {
                arena.step();
                (
                    arena.have(),
                    arena.game.drops.iter().map(|d| (d.x16, d.y16, d.count)).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
